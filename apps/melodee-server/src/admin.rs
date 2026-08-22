//! New Rust-native Admin API (PRD-API-ADMIN-001/002/003, SPEC section 19.4).
//!
//! This is the operator/curation API, intentionally separate from the legacy
//! Melodee API v1, OpenSubsonic, and Jellyfin compatibility surfaces. It uses
//! Rust-native vocabulary (`Release` work items, not `Album`) and is
//! independently versioned under `/api/admin/v1`.
//!
//! scope: durable-work-item **diagnostics only** — state counts,
//! bounded/paged listing, work-item detail, append-only event history, and
//! active-lease evidence. It does not expose secrets, arbitrary filesystem
//! contents, or raw SQLx rows; DTOs are explicit and bounded. No compatibility
//! route is invented here.
//!
//! (ADR-0139): every route in this module is gated behind
//! [`AdminPrincipal`] — anonymous requests receive 401, authenticated
//! non-administrators 403, both with the stable error envelope. `/healthz`
//! and `/readyz` remain anonymous infrastructure probes and are not routed
//! here.
//!
//! Error/correlation conventions: errors use a stable JSON envelope with a
//! machine-readable `code` and a request `correlation_id` (sourced from the
//! `x-request-id` header when present, otherwise generated). This establishes
//! the admin API error/correlation shape required by SPEC for later phases.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_domain::{IngestionState, ReleaseWorkItem, ReleaseWorkItemEvent, WorkEventType};
use melodee_persistence::{ActiveLease, StateCount, WorkItemSort, WorkItemStore};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};
use crate::system::SystemInfo;

/// Maximum rows returned by list endpoints. The query layer clamps to this
/// even when a client requests more, so a misbehaving caller cannot pull the
/// whole queue.
const MAX_LIST_LIMIT: u32 = 500;

/// One durable work item as exposed to operators (PRD-INGEST-010). Contains
/// only operator-safe fields: paths (read-only source paths, never credentials),
/// state, attempts, lease evidence, last error code/message, timestamps, and
/// the correlation id. Never includes secrets — the store never receives them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemDto {
    pub id: Uuid,
    pub api_key: Uuid,
    pub source_library_id: Option<Uuid>,
    pub target_library_id: Option<Uuid>,
    pub source_path: String,
    pub current_path: String,
    pub target_path: Option<String>,
    pub source_fingerprint: Option<String>,
    pub manifest_hash: Option<String>,
    pub state: IngestionState,
    pub state_reason: Option<String>,
    pub attempt_count: u32,
    pub next_attempt_at: Option<OffsetDateTime>,
    pub lease_owner: Option<String>,
    pub lease_acquired_at: Option<OffsetDateTime>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub correlation_id: Uuid,
    pub discovered_at: OffsetDateTime,
    pub first_started_at: Option<OffsetDateTime>,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

impl From<ReleaseWorkItem> for WorkItemDto {
    fn from(item: ReleaseWorkItem) -> Self {
        Self {
            id: item.id,
            api_key: item.api_key,
            source_library_id: item.source_library_id,
            target_library_id: item.target_library_id,
            source_path: item.source_path,
            current_path: item.current_path,
            target_path: item.target_path,
            source_fingerprint: item.source_fingerprint,
            manifest_hash: item.manifest_hash,
            state: item.state,
            state_reason: item.state_reason,
            attempt_count: item.attempt_count,
            next_attempt_at: item.next_attempt_at,
            lease_owner: item.lease_owner,
            lease_acquired_at: item.lease_acquired_at,
            lease_expires_at: item.lease_expires_at,
            last_error_code: item.last_error_code.map(|c| c.as_str().to_string()),
            last_error_message: item.last_error_message,
            correlation_id: item.correlation_id,
            discovered_at: item.discovered_at,
            first_started_at: item.first_started_at,
            last_attempt_at: item.last_attempt_at,
            updated_at: item.updated_at,
            created_at: item.created_at,
            completed_at: item.completed_at,
        }
    }
}

/// One append-only history event (PRD-INGEST-010: "what happened and why").
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemEventDto {
    pub id: i64,
    pub work_item_id: Uuid,
    pub event_type: WorkEventType,
    pub from_state: Option<IngestionState>,
    pub to_state: Option<IngestionState>,
    pub reason_code: Option<String>,
    pub worker_id: Option<String>,
    pub correlation_id: Uuid,
    pub created_at: OffsetDateTime,
    /// Structured, operator-safe extras only. Never secrets or stack traces.
    pub metadata: Option<serde_json::Value>,
}

impl From<ReleaseWorkItemEvent> for WorkItemEventDto {
    fn from(event: ReleaseWorkItemEvent) -> Self {
        Self {
            id: event.id,
            work_item_id: event.work_item_id,
            event_type: event.event_type,
            from_state: event.from_state,
            to_state: event.to_state,
            reason_code: event.reason_code,
            worker_id: event.worker_id,
            correlation_id: event.correlation_id,
            created_at: event.created_at,
            metadata: event.metadata,
        }
    }
}

/// Active-lease evidence (PRD-WORK-002). This is what PostgreSQL can prove: a
/// row currently holds an unexpired lease. It is **not** a live-worker liveness
/// claim (no heartbeat store exists yet).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveLeaseDto {
    pub work_item_id: Uuid,
    pub api_key: Uuid,
    pub state: IngestionState,
    pub lease_owner: String,
    pub lease_acquired_at: OffsetDateTime,
    pub lease_expires_at: OffsetDateTime,
    pub last_activity_at: OffsetDateTime,
}

impl From<ActiveLease> for ActiveLeaseDto {
    fn from(lease: ActiveLease) -> Self {
        Self {
            work_item_id: lease.work_item_id,
            api_key: lease.api_key,
            state: lease.state,
            lease_owner: lease.lease_owner,
            lease_acquired_at: lease.lease_acquired_at,
            lease_expires_at: lease.lease_expires_at,
            last_activity_at: lease.last_activity_at,
        }
    }
}

/// Per-state counts for the queue overview (PRD-WORK-002).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateCountDto {
    pub state: IngestionState,
    pub count: i64,
}

impl From<StateCount> for StateCountDto {
    fn from(count: StateCount) -> Self {
        Self {
            state: count.state,
            count: count.count,
        }
    }
}

/// Query parameters for the bounded work-item listing. All optional; omitted
/// filters return items in any state. Pagination is server-side and bounded.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListWorkItemsParams {
    /// Filter by state (e.g. `ready`, `failed`, `needs_review`). Unknown values
    /// produce a 400 rather than silently returning an empty list.
    pub state: Option<String>,
    /// Maximum rows to return (clamped to MAX_LIST_LIMIT). Default 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    /// Zero-based offset for paging.
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort expression `key:asc|desc`: `addedAt`,
    /// `updatedAt`, `state`, or `attemptCount`. Omitted keeps the historical
    /// newest-first ordering. Unknown keys or directions produce a 400.
    pub sort: Option<String>,
}

fn default_list_limit() -> u32 {
    50
}

/// Query parameters for the active-lease listing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListActiveLeasesParams {
    /// Maximum leases to return (clamped to MAX_LIST_LIMIT). Default 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
}

/// Stable admin API error envelope (SPEC: "error envelope and correlation
/// conventions for the new admin API"). The `code` is machine-readable and
/// stable; `message` is operator-safe and never contains secrets.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminApiErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

/// Typed admin API errors mapping to status codes.
#[derive(Debug)]
pub enum AdminApiErrorKind {
    /// Malformed request (bad state filter, bad pagination, etc.).
    BadRequest(&'static str),
    /// Resource not found.
    NotFound,
    /// Upstream persistence failure (message is sanitized).
    StoreError,
}

impl AdminApiErrorKind {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BadRequest(code) => (StatusCode::BAD_REQUEST, code),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::StoreError => (StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        }
    }
}

/// One page of work items plus the filter's total row count ( pagination
/// envelope). `total` respects the `state` filter but ignores `limit`/`offset`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemPageDto {
    pub items: Vec<WorkItemDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// The admin-gated diagnostics router (ADR-0139). Shares `AdminAccessState`
/// so the `AdminPrincipal` extractor can validate native access JWTs.
pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/system/info", get(system_info))
        .route(
            "/api/admin/v1/ingest/work-items/counts",
            get(work_item_counts),
        )
        .route("/api/admin/v1/ingest/work-items", get(list_work_items))
        .route("/api/admin/v1/ingest/work-items/{id}", get(get_work_item))
        .route(
            "/api/admin/v1/ingest/work-items/{id}/events",
            get(list_work_item_events),
        )
        .route("/api/admin/v1/ingest/leases", get(list_active_leases))
}

/// GET /api/admin/v1/system/info — health/version metadata, admin-gated per
/// ADR-0139 (the anonymous infrastructure probes remain `/healthz`/`/readyz`).
pub async fn system_info(_admin: AdminPrincipal) -> Json<SystemInfo> {
    crate::system::healthz().await
}

/// GET /api/admin/v1/ingest/work-items/counts — per-state queue counts.
pub async fn work_item_counts(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
) -> Result<Json<Vec<StateCountDto>>, AdminApiError> {
    let counts = WorkItemStore
        .counts_by_state(&state.pool)
        .await
        .map_err(AdminApiError::from_store)?;
    Ok(Json(counts.into_iter().map(StateCountDto::from).collect()))
}

/// GET /api/admin/v1/ingest/work-items — bounded, filtered, sorted, paged
/// listing inside a pagination envelope.
pub async fn list_work_items(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListWorkItemsParams>,
) -> Result<Json<WorkItemPageDto>, AdminApiError> {
    let parsed_state = match params.state.as_deref() {
        None => None,
        Some(raw) => Some(IngestionState::parse(raw).ok_or_else(|| {
            AdminApiError::new(
                AdminApiErrorKind::BadRequest("unknown_state"),
                format!("unknown state '{raw}'"),
            )
        })?),
    };
    let sort = match params.sort.as_deref() {
        None => WorkItemSort::default_order(),
        Some(raw) => WorkItemSort::parse(raw).ok_or_else(|| {
            AdminApiError::new(
                AdminApiErrorKind::BadRequest("unknown_sort"),
                format!("unknown sort '{raw}'"),
            )
        })?,
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    let (items, total) = WorkItemStore
        .list_page(&state.pool, parsed_state, limit, offset, sort)
        .await
        .map_err(AdminApiError::from_store)?;
    Ok(Json(WorkItemPageDto {
        items: items.into_iter().map(WorkItemDto::from).collect(),
        total,
        limit,
        offset,
    }))
}

/// GET /api/admin/v1/ingest/work-items/:id — one work item by id or api key.
pub async fn get_work_item(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkItemDto>, AdminApiError> {
    let item = WorkItemStore
        .get(&state.pool, id)
        .await
        .map_err(AdminApiError::from_store)?;
    Ok(Json(WorkItemDto::from(item)))
}

/// GET /api/admin/v1/ingest/work-items/:id/events — append-only history.
pub async fn list_work_item_events(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<WorkItemEventDto>>, AdminApiError> {
    // The store lists events by id; a nonexistent work item yields an empty
    // list rather than a 404. Resolve the item first so the endpoint reports
    // 404 for an unknown id consistently with get_work_item.
    WorkItemStore
        .get(&state.pool, id)
        .await
        .map_err(AdminApiError::from_store)?;
    let events = WorkItemStore
        .list_events(&state.pool, id)
        .await
        .map_err(AdminApiError::from_store)?;
    Ok(Json(
        events.into_iter().map(WorkItemEventDto::from).collect(),
    ))
}

/// GET /api/admin/v1/ingest/leases — active (unexpired) lease evidence.
pub async fn list_active_leases(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListActiveLeasesParams>,
) -> Result<Json<Vec<ActiveLeaseDto>>, AdminApiError> {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let leases = WorkItemStore
        .list_active_leases(&state.pool, OffsetDateTime::now_utc(), limit)
        .await
        .map_err(AdminApiError::from_store)?;
    Ok(Json(leases.into_iter().map(ActiveLeaseDto::from).collect()))
}

/// Wraps a typed error with a correlation id.
pub struct AdminApiError {
    kind: AdminApiErrorKind,
    message: String,
    correlation_id: Uuid,
}

impl AdminApiError {
    fn new(kind: AdminApiErrorKind, message: String) -> Self {
        Self {
            kind,
            message,
            correlation_id: crate::correlation::envelope_id(),
        }
    }

    fn from_store(error: melodee_persistence::WorkStoreError) -> Self {
        match error {
            melodee_persistence::WorkStoreError::NotFound(id) => Self {
                kind: AdminApiErrorKind::NotFound,
                message: format!("work item not found: {id}"),
                correlation_id: crate::correlation::envelope_id(),
            },
            other => Self::new(AdminApiErrorKind::StoreError, other.to_string()),
        }
    }
}

impl IntoResponse for AdminApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.kind.status_and_code();
        (
            status,
            Json(AdminApiErrorBody {
                code,
                message: self.message,
                correlation_id: self.correlation_id,
            }),
        )
            .into_response()
    }
}
