//! Phase 10 WP-14 dashboard aggregation and WP-17 system/workers/search
//! admin API (PRD-API-ADMIN-002, PRD-UI-001, PRD-WORK-002, SPEC §22).
//!
//! Everything here is aggregated from durable state with bounded, set-based
//! queries — no endpoint walks the catalog row-by-row and none of them run
//! probes beyond what `readyz`/onboarding already perform:
//!
//! - KPI counts come from the catalog aggregate read and work-item state
//!   counts; the rolling 24h failure/completion windows and the per-hour
//!   activity series are set-based `FILTER`/`date_trunc` aggregates over
//!   `release_work_items`.
//! - Pool pause/resume is the same lease-coordinated durable control plane
//!   the CLI uses (`WorkerPoolStore`); workers honor the pool row between
//!   claim batches, so a pause never strands an in-flight lease.
//! - Pool metrics are reported only where a durable queue table exists
//!   (policy, podcast); pools without one (discovery maintenance) report
//!   `metricsAvailable: false` instead of invented zeros. "Running" is
//!   unexpired-lease evidence, not a worker liveness claim.
//! - Search status/rebuild go through the same `melodee-search` service
//!   functions as the CLI. Rebuild is *requested* here; the running search
//!   worker claims and executes the durable job (a second request while one
//!   is unfinished is `409 rebuild_in_progress`).
//! - Subsystem health reuses the onboarding preflight checks (storage paths,
//!   media tools) and the `readyz`-style bounded probes (PostgreSQL
//!   `SELECT 1`, Tantivy active-generation diagnostic).

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_domain::IngestionState;
use melodee_persistence::{
    CatalogStore, PolicyStore, WorkItemSort, WorkItemStore, WorkerPoolState, WorkerPoolStore,
    WorkerPoolStoreError,
};
use melodee_search::SearchConfig;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin::{StateCountDto, WorkItemDto};
use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};
use crate::onboarding::PreflightCheck;

/// Maximum rows returned by the failures listing.
const MAX_LIST_LIMIT: u32 = 500;
/// Bounded probe timeout shared with `readyz`.
const HEALTH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Dashboard rolling window for failures/completions/activity.
const DASHBOARD_WINDOW: time::Duration = time::Duration::hours(24);

/// Shared state for the WP-14/WP-17 surface: the common admin gate plus the
/// configured search projection (absent when the search worker is disabled).
#[derive(Clone)]
pub struct AdminSystemState {
    pub admin: AdminAccessState,
    pub search: Option<SearchConfig>,
}

impl AdminSystemState {
    #[must_use]
    pub fn new(admin: AdminAccessState, search: Option<SearchConfig>) -> Self {
        Self { admin, search }
    }
}

/// The surface shares the administrator gate; extraction delegates to the
/// canonical `AdminAccessState` implementation so 401/403 behavior is
/// identical on every admin route.
impl FromRequestParts<AdminSystemState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminSystemState,
    ) -> Result<Self, Self::Rejection> {
        <Self as FromRequestParts<AdminAccessState>>::from_request_parts(parts, &state.admin).await
    }
}

fn system_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: message.into(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: &impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin system store failure");
    system_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

// ---------------------------------------------------------------------------
// Health aggregation (dashboard + /system/health)
// ---------------------------------------------------------------------------

/// One subsystem's health verdict. `status` is `ok`, `degraded`, or
/// `disabled` (a deliberately unconfigured optional subsystem).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemHealthDto {
    pub status: &'static str,
    pub detail: String,
}

impl SubsystemHealthDto {
    fn ok(detail: impl Into<String>) -> Self {
        Self {
            status: "ok",
            detail: detail.into(),
        }
    }

    fn degraded(detail: impl Into<String>) -> Self {
        Self {
            status: "degraded",
            detail: detail.into(),
        }
    }

    fn disabled(detail: impl Into<String>) -> Self {
        Self {
            status: "disabled",
            detail: detail.into(),
        }
    }
}

fn preflight_health(check: &PreflightCheck) -> SubsystemHealthDto {
    if check.passed {
        SubsystemHealthDto::ok(check.detail.clone())
    } else {
        SubsystemHealthDto::degraded(check.detail.clone())
    }
}

/// One mounted API surface. These are process-local routers: serving this
/// response at all proves the process hosts them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiSurfaceDto {
    pub surface: &'static str,
    pub status: &'static str,
}

/// Aggregated subsystem health (SPEC §22 panel content).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDto {
    /// `ok` when PostgreSQL is reachable and Tantivy is healthy or disabled.
    pub status: &'static str,
    pub postgresql: SubsystemHealthDto,
    pub tantivy: SubsystemHealthDto,
    pub storage: SubsystemHealthDto,
    pub ffmpeg: SubsystemHealthDto,
    pub api_surfaces: Vec<ApiSurfaceDto>,
}

async fn postgresql_health(pool: &sqlx::PgPool) -> SubsystemHealthDto {
    let probe = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool);
    match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, probe).await {
        Ok(Ok(1)) => SubsystemHealthDto::ok("postgresql answered a bounded probe"),
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
            SubsystemHealthDto::degraded("postgresql probe failed or timed out")
        }
    }
}

async fn tantivy_health(pool: &sqlx::PgPool, search: &Option<SearchConfig>) -> SubsystemHealthDto {
    let Some(config) = search.as_ref().filter(|config| config.enabled) else {
        return SubsystemHealthDto::disabled("search worker disabled");
    };
    let diagnostic = tokio::time::timeout(
        HEALTH_PROBE_TIMEOUT,
        melodee_search::diagnostic(config, pool),
    )
    .await;
    match diagnostic {
        Ok(Ok(diagnostic))
            if diagnostic.generation.is_some() && diagnostic.generation_error.is_none() =>
        {
            SubsystemHealthDto::ok("active search generation validated")
        }
        Ok(Ok(diagnostic)) => SubsystemHealthDto::degraded(
            diagnostic
                .generation_error
                .unwrap_or_else(|| "search generation unavailable".to_string()),
        ),
        Ok(Err(error)) => SubsystemHealthDto::degraded(error.operator_message().to_string()),
        Err(_) => SubsystemHealthDto::degraded("search diagnostic timed out"),
    }
}

async fn aggregate_health(pool: &sqlx::PgPool, search: &Option<SearchConfig>) -> HealthDto {
    let postgresql = postgresql_health(pool).await;
    let tantivy = tantivy_health(pool, search).await;
    // The onboarding preflight checks are configuration/filesystem reads
    // plus bounded `-version` probes — exactly what the onboarding surface
    // already runs, nothing more.
    let storage = preflight_health(&crate::onboarding::check_storage_paths());
    let ffmpeg = preflight_health(&crate::onboarding::check_media_tools().await);
    let status = if postgresql.status == "ok" && tantivy.status != "degraded" {
        "ok"
    } else {
        "degraded"
    };
    HealthDto {
        status,
        postgresql,
        tantivy,
        storage,
        ffmpeg,
        api_surfaces: vec![
            ApiSurfaceDto {
                surface: "native",
                status: "serving",
            },
            ApiSurfaceDto {
                surface: "opensubsonic",
                status: "serving",
            },
            ApiSurfaceDto {
                surface: "jellyfin",
                status: "serving",
            },
            ApiSurfaceDto {
                surface: "admin",
                status: "serving",
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// WP-14 dashboard
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCatalogDto {
    pub artists: i64,
    pub releases: i64,
    pub tracks: i64,
    pub media_assets: i64,
    pub pending_search_outbox: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardWorkItemsDto {
    pub counts_by_state: Vec<StateCountDto>,
    pub failed_last_24h: i64,
    pub completed_last_24h: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardAttentionDto {
    pub unresolved_identities: i64,
    pub pending_policy_decisions: i64,
    pub failed_processing: i64,
    pub quarantined: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardActivityBucketDto {
    pub hour: OffsetDateTime,
    pub count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardActivityDto {
    /// Completed work items per hour over the trailing 24h (durable
    /// `completed_at` evidence; hours without completions are omitted).
    pub completed_per_hour: Vec<DashboardActivityBucketDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDto {
    pub generated_at: OffsetDateTime,
    pub catalog: DashboardCatalogDto,
    pub work_items: DashboardWorkItemsDto,
    pub attention: DashboardAttentionDto,
    pub health: HealthDto,
    pub activity: DashboardActivityDto,
}

// ---------------------------------------------------------------------------
// WP-17 worker pools
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPoolDto {
    pub name: String,
    pub paused: bool,
    pub paused_by: Option<String>,
    pub paused_at: Option<OffsetDateTime>,
    pub resume_requested_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
    /// False when the pool's worker has no durable queue table (discovery
    /// maintenance); all metric fields are then null.
    pub metrics_available: bool,
    pub queued: Option<i64>,
    pub running: Option<i64>,
    pub oldest_queued_age_seconds: Option<i64>,
    pub completed_last_hour: Option<i64>,
    pub throughput_per_minute: Option<f64>,
    pub failed_last_24h: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPoolPageDto {
    pub items: Vec<WorkerPoolDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Query parameters for the failures listing (same whitelist as the ingest
/// work-item listing).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailuresParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    pub sort: Option<String>,
}

const fn default_list_limit() -> u32 {
    50
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailuresPageDto {
    pub items: Vec<WorkItemDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

// ---------------------------------------------------------------------------
// WP-17 search administration
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProjectionDto {
    pub pending: i64,
    pub failed: i64,
    pub leased: i64,
    pub oldest_available_at: Option<OffsetDateTime>,
    pub last_processed_outbox_id: i64,
    pub active_generation_id: Option<Uuid>,
    pub latest_rebuild_status: Option<String>,
    pub latest_rebuild_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchGenerationDto {
    pub artists: usize,
    pub releases: usize,
    pub tracks: usize,
    pub total: usize,
    pub schema_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatusDto {
    pub enabled: bool,
    pub projection: Option<SearchProjectionDto>,
    pub generation: Option<SearchGenerationDto>,
    pub generation_error_code: Option<String>,
    pub generation_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildDto {
    pub job_id: Uuid,
    pub status: String,
    pub requested_by: String,
}

pub fn router() -> Router<AdminSystemState> {
    Router::new()
        .route("/api/admin/v1/dashboard", get(dashboard))
        .route("/api/admin/v1/workers/pools", get(worker_pools))
        .route("/api/admin/v1/workers/pools/{name}/pause", post(pause_pool))
        .route(
            "/api/admin/v1/workers/pools/{name}/resume",
            post(resume_pool),
        )
        .route("/api/admin/v1/workers/failures", get(worker_failures))
        .route("/api/admin/v1/search/status", get(search_status))
        .route("/api/admin/v1/search/rebuild", post(search_rebuild))
        .route("/api/admin/v1/system/health", get(system_health))
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

/// GET /api/admin/v1/dashboard — aggregated KPIs, attention counts, health,
/// and activity for the overview page (PRD-UI-001).
async fn dashboard(State(state): State<AdminSystemState>, _admin: AdminPrincipal) -> Response {
    let pool = &state.admin.pool;
    let now = OffsetDateTime::now_utc();
    let since = now - DASHBOARD_WINDOW;

    let catalog = match CatalogStore.counts(pool).await {
        Ok(counts) => counts,
        Err(error) => return store_failure("catalog counts failed", &error),
    };
    let counts_by_state = match WorkItemStore.counts_by_state(pool).await {
        Ok(counts) => counts,
        Err(error) => return store_failure("work-item counts failed", &error),
    };
    let window = match WorkItemStore.window_counts(pool, since).await {
        Ok(window) => window,
        Err(error) => return store_failure("work-item window counts failed", &error),
    };
    let unresolved = match CatalogStore.count_unresolved_observations(pool).await {
        Ok(count) => count,
        Err(error) => return store_failure("unresolved identity count failed", &error),
    };
    let pending_decisions = match PolicyStore.count_pending_decisions(pool).await {
        Ok(count) => count,
        Err(error) => return store_failure("pending decision count failed", &error),
    };
    let hourly = match WorkItemStore.completed_hourly(pool, since).await {
        Ok(hourly) => hourly,
        Err(error) => return store_failure("activity series failed", &error),
    };
    let failed_processing = counts_by_state
        .iter()
        .find(|count| count.state == IngestionState::Failed)
        .map_or(0, |count| count.count);
    let quarantined = counts_by_state
        .iter()
        .find(|count| count.state == IngestionState::Quarantined)
        .map_or(0, |count| count.count);
    let health = aggregate_health(pool, &state.search).await;
    Json(DashboardDto {
        generated_at: now,
        catalog: DashboardCatalogDto {
            artists: catalog.artists,
            releases: catalog.releases,
            tracks: catalog.tracks,
            media_assets: catalog.media_assets,
            pending_search_outbox: catalog.pending_outbox,
        },
        work_items: DashboardWorkItemsDto {
            counts_by_state: counts_by_state
                .into_iter()
                .map(StateCountDto::from)
                .collect(),
            failed_last_24h: window.failed,
            completed_last_24h: window.completed,
        },
        attention: DashboardAttentionDto {
            unresolved_identities: unresolved,
            pending_policy_decisions: pending_decisions,
            failed_processing,
            quarantined,
        },
        health,
        activity: DashboardActivityDto {
            completed_per_hour: hourly
                .into_iter()
                .map(|bucket| DashboardActivityBucketDto {
                    hour: bucket.hour,
                    count: bucket.count,
                })
                .collect(),
        },
    })
    .into_response()
}

fn pool_dto(
    state: &WorkerPoolState,
    metrics: Option<melodee_persistence::WorkerPoolMetrics>,
    now: OffsetDateTime,
) -> WorkerPoolDto {
    let (
        metrics_available,
        queued,
        running,
        oldest_queued_age_seconds,
        completed_last_hour,
        throughput_per_minute,
        failed_last_24h,
    ) = match metrics {
        Some(metrics) => (
            true,
            Some(metrics.queued),
            Some(metrics.running),
            metrics
                .oldest_queued_at
                .map(|oldest| (now - oldest).whole_seconds().max(0)),
            Some(metrics.completed_last_hour),
            Some(metrics.completed_last_hour as f64 / 60.0),
            Some(metrics.failed_last_24h),
        ),
        None => (false, None, None, None, None, None, None),
    };
    WorkerPoolDto {
        name: state.pool.clone(),
        paused: state.paused,
        paused_by: state.paused_by.clone(),
        paused_at: state.paused_at,
        resume_requested_at: state.resume_requested_at,
        updated_at: state.updated_at,
        metrics_available,
        queued,
        running,
        oldest_queued_age_seconds,
        completed_last_hour,
        throughput_per_minute,
        failed_last_24h,
    }
}

/// GET /api/admin/v1/workers/pools — durable pool table with queue metrics.
async fn worker_pools(State(state): State<AdminSystemState>, _admin: AdminPrincipal) -> Response {
    let pool = &state.admin.pool;
    let now = OffsetDateTime::now_utc();
    let pools = match WorkerPoolStore.list(pool).await {
        Ok(pools) => pools,
        Err(error) => return store_failure("worker pool listing failed", &error),
    };
    let mut items = Vec::with_capacity(pools.len());
    for pool_state in &pools {
        let metrics = match WorkerPoolStore.metrics(pool, &pool_state.pool, now).await {
            Ok(metrics) => metrics,
            Err(error) => return store_failure("worker pool metrics failed", &error),
        };
        items.push(pool_dto(pool_state, metrics, now));
    }
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    let limit = u32::try_from(items.len()).unwrap_or(u32::MAX);
    Json(WorkerPoolPageDto {
        items,
        total,
        limit,
        offset: 0,
    })
    .into_response()
}

/// Shared pause/resume path: the exact lease-coordinated durable control the
/// CLI uses (workers honor the pool row between claim batches).
async fn set_pool_paused(
    state: &AdminSystemState,
    admin: &AdminPrincipal,
    name: &str,
    paused: bool,
) -> Response {
    let now = OffsetDateTime::now_utc();
    match WorkerPoolStore
        .set_paused(
            &state.admin.pool,
            name,
            paused,
            &admin.public_api_id.to_string(),
            now,
        )
        .await
    {
        Ok(pool_state) => {
            tracing::info!(
                actor = %admin.public_api_id,
                pool = %name,
                paused,
                "admin changed worker pool state"
            );
            let metrics = match WorkerPoolStore.metrics(&state.admin.pool, name, now).await {
                Ok(metrics) => metrics,
                Err(error) => return store_failure("worker pool metrics failed", &error),
            };
            Json(pool_dto(&pool_state, metrics, now)).into_response()
        }
        Err(WorkerPoolStoreError::InvalidPoolName) => system_error(
            StatusCode::BAD_REQUEST,
            "invalid_pool_name",
            "pool name must match ^[a-z][a-z0-9_-]{0,63}$",
        ),
        Err(error) => store_failure("worker pool control failed", &error),
    }
}

/// POST /api/admin/v1/workers/pools/{name}/pause
async fn pause_pool(
    State(state): State<AdminSystemState>,
    admin: AdminPrincipal,
    Path(name): Path<String>,
) -> Response {
    set_pool_paused(&state, &admin, &name, true).await
}

/// POST /api/admin/v1/workers/pools/{name}/resume
async fn resume_pool(
    State(state): State<AdminSystemState>,
    admin: AdminPrincipal,
    Path(name): Path<String>,
) -> Response {
    set_pool_paused(&state, &admin, &name, false).await
}

/// GET /api/admin/v1/workers/failures — recent failed work items, bounded.
async fn worker_failures(
    State(state): State<AdminSystemState>,
    _admin: AdminPrincipal,
    Query(params): Query<FailuresParams>,
) -> Response {
    let sort = match params.sort.as_deref() {
        None => WorkItemSort::default_order(),
        Some(raw) => match WorkItemSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return system_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match WorkItemStore
        .list_page_states(
            &state.admin.pool,
            &[IngestionState::Failed],
            limit,
            offset,
            sort,
        )
        .await
    {
        Ok((items, total)) => Json(FailuresPageDto {
            items: items.into_iter().map(WorkItemDto::from).collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => store_failure("failure listing failed", &error),
    }
}

/// GET /api/admin/v1/search/status — projection/generation status through
/// the same diagnostic the CLI `search status` uses.
async fn search_status(State(state): State<AdminSystemState>, _admin: AdminPrincipal) -> Response {
    let Some(config) = state.search.as_ref().filter(|config| config.enabled) else {
        return Json(SearchStatusDto {
            enabled: false,
            projection: None,
            generation: None,
            generation_error_code: None,
            generation_error: None,
        })
        .into_response();
    };
    match melodee_search::diagnostic(config, &state.admin.pool).await {
        Ok(diagnostic) => Json(SearchStatusDto {
            enabled: true,
            projection: Some(SearchProjectionDto {
                pending: diagnostic.projection.pending,
                failed: diagnostic.projection.failed,
                leased: diagnostic.projection.leased,
                oldest_available_at: diagnostic.projection.oldest_available_at,
                last_processed_outbox_id: diagnostic.projection.last_processed_outbox_id,
                active_generation_id: diagnostic.projection.active_generation_id,
                latest_rebuild_status: diagnostic.projection.latest_rebuild_status,
                latest_rebuild_error: diagnostic.projection.latest_rebuild_error,
            }),
            generation: diagnostic.generation.map(|generation| SearchGenerationDto {
                artists: generation.artists,
                releases: generation.releases,
                tracks: generation.tracks,
                total: generation.total,
                schema_version: generation.schema_version,
            }),
            generation_error_code: diagnostic.generation_error_code,
            generation_error: diagnostic.generation_error,
        })
        .into_response(),
        Err(error) => store_failure("search diagnostic failed", &error.operator_message()),
    }
}

/// POST /api/admin/v1/search/rebuild — durably request a generational
/// rebuild (the same `request_rebuild` the CLI uses). The running search
/// worker claims and executes the job; this endpoint never blocks on the
/// rebuild itself. `409 rebuild_in_progress` while an unfinished job exists.
async fn search_rebuild(State(state): State<AdminSystemState>, admin: AdminPrincipal) -> Response {
    let Some(config) = state.search.as_ref().filter(|config| config.enabled) else {
        return system_error(
            StatusCode::CONFLICT,
            "search_disabled",
            "the search worker is disabled; enable MELODEE__SEARCH__ENABLED to rebuild",
        );
    };
    match melodee_search::request_rebuild(
        config,
        &state.admin.pool,
        &admin.public_api_id.to_string(),
    )
    .await
    {
        Ok(job) => {
            tracing::info!(
                actor = %admin.public_api_id,
                job_id = %job.job_id,
                "admin requested search rebuild"
            );
            (
                StatusCode::ACCEPTED,
                Json(SearchRebuildDto {
                    job_id: job.job_id,
                    status: job.status,
                    requested_by: job.requested_by,
                }),
            )
                .into_response()
        }
        Err(melodee_search::SearchError::Persistence(
            melodee_persistence::SearchStoreError::RebuildConflict(root),
        )) => system_error(
            StatusCode::CONFLICT,
            "rebuild_in_progress",
            format!("an unfinished search rebuild already exists for root {root}"),
        ),
        Err(error) => store_failure("search rebuild request failed", &error.operator_message()),
    }
}

/// GET /api/admin/v1/system/health — aggregated subsystem health.
async fn system_health(State(state): State<AdminSystemState>, _admin: AdminPrincipal) -> Response {
    Json(aggregate_health(&state.admin.pool, &state.search).await).into_response()
}
