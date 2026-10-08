//! Release Review API (PRD-API-ADMIN-002, PRD-UI-007/008,
//! SPEC §20): the operator queue for Releases parked in review-able states,
//! full review detail (manifest summary, classification evidence, catalog
//! comparison, policy decision, timeline), audited decisions, operator
//! reprocess, and a gated pre-publication audio preview.
//!
//! All routes are `AdminPrincipal`-gated. Decisions are driven through the
//! durable policy store (`apply_admin_override`) when a pending durable
//! decision exists, and through the operator arm of the work-item state
//! machine otherwise. A confirmed `delete` removes the exact durable staging
//! and canonical directories before `complete_destructive_action` records the
//! catalog tombstone and completed audit evidence.
//!
//! The preview endpoint never trusts client-supplied paths: the track file
//! is located from the durable manifest and the configured staging/work
//! roots, canonicalized, containment-checked, and opened with the no-follow
//! `CanonicalAssetRoot` used by native streaming.

use std::{path::Path, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_domain::{
    DecisionCompletionState, Disposition, IngestionState, ReleaseManifest, TransitionKind,
};
use melodee_persistence::{
    CatalogStore, PolicyAdminOverrideRequest, PolicyDestructiveCompletion, PolicyStore,
    PolicyStoreError, StoredNoveltyEvaluation, StoredPolicyDecision, StoredReleaseClassification,
    WorkItemSort, WorkItemStore, WorkStoreError,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin::{StateCountDto, WorkItemDto, WorkItemEventDto};
use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};
use crate::native_stream::{CanonicalAssetRoot, DirectPlayService, PlayableAsset, StreamError};

/// Maximum rows returned by the review queue listing.
const MAX_LIST_LIMIT: u32 = 500;
/// Streams an operator preview may hold open at once per request service.
/// Preview is a rare, admin-gated action; the bound exists so a stuck client
/// cannot strand unbounded file handles.
const PREVIEW_STREAM_CONCURRENCY: usize = 4;

/// Work-item states an operator can review: parked for review, failed
/// (awaiting retry/reprocess), or quarantined (parked, reprocessable).
const REVIEW_STATES: [IngestionState; 3] = [
    IngestionState::NeedsReview,
    IngestionState::Failed,
    IngestionState::Quarantined,
];

/// One configured durable staging root eligible to back a preview. The name
/// remains compatible with the review test harness.
#[derive(Debug, Clone)]
pub struct PreviewSourceRoot {
    pub id: Option<Uuid>,
    pub root: PathBuf,
}

/// Path-safety context for the pre-publication preview endpoint. Roots are
/// configured server-side; per request the candidate base directory is
/// canonicalized and must remain inside the canonicalized configured root.
#[derive(Debug)]
pub struct ReviewPreview {
    staging_root: Option<PathBuf>,
    work_area_base: PathBuf,
}

impl ReviewPreview {
    #[must_use]
    pub fn new(source_roots: Vec<PreviewSourceRoot>, work_area_base: PathBuf) -> Self {
        Self {
            staging_root: source_roots.into_iter().next().map(|root| root.root),
            work_area_base,
        }
    }
}

/// Shared state for the review surface: the common admin state (auth
/// gate, pool) plus the preview path-safety context.
#[derive(Clone)]
pub struct AdminReviewState {
    pub admin: AdminAccessState,
    pub preview: Arc<ReviewPreview>,
    deletion: Arc<ReviewDeletion>,
}

#[derive(Debug)]
struct ReviewDeletion {
    staging_root: Option<PathBuf>,
    canonical_root: Option<PathBuf>,
}

impl AdminReviewState {
    #[must_use]
    pub fn new(
        admin: AdminAccessState,
        source_roots: Vec<PreviewSourceRoot>,
        work_area_base: PathBuf,
        canonical_root: Option<PathBuf>,
    ) -> Self {
        let staging_root = source_roots.first().map(|root| root.root.clone());
        Self {
            admin,
            preview: Arc::new(ReviewPreview::new(source_roots, work_area_base)),
            deletion: Arc::new(ReviewDeletion {
                staging_root,
                canonical_root,
            }),
        }
    }
}

/// The review surface shares the administrator gate; extraction delegates to
/// the canonical `AdminAccessState` implementation so 401/403 behavior is
/// identical on every admin route.
impl FromRequestParts<AdminReviewState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminReviewState,
    ) -> Result<Self, Self::Rejection> {
        <Self as FromRequestParts<AdminAccessState>>::from_request_parts(parts, &state.admin).await
    }
}

/// Stable admin API error envelope (shared conventions).
fn review_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: message.into(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: &impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin review store failure");
    review_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

/// Query parameters for the review queue listing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQueueParams {
    /// Optional state filter; restricted to the review-able states
    /// (`needs_review`, `failed`, `quarantined`).
    pub state: Option<String>,
    /// Maximum rows to return (clamped to MAX_LIST_LIMIT). Default 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    /// Zero-based offset for paging.
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort expression `key:asc|desc` (same keys as the ingest
    /// work-item listing: `addedAt`, `updatedAt`, `state`, `attemptCount`).
    pub sort: Option<String>,
}

const fn default_list_limit() -> u32 {
    50
}

/// One review-queue row: the durable work item plus its catalog Release
/// binding when the item has been imported.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQueueItemDto {
    #[serde(flatten)]
    pub item: WorkItemDto,
    pub release_id: Option<Uuid>,
}

/// Paginated review queue with per-review-state counts.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQueuePageDto {
    pub items: Vec<ReviewQueueItemDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
    pub counts_by_state: Vec<StateCountDto>,
}

/// One manifest track row as shown in review.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTrackDto {
    pub index: u32,
    pub file_path: String,
    pub title: Option<String>,
    pub duration_ms: Option<u64>,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub mime_type: Option<String>,
    /// `captured` once normalization recorded a fingerprint for the file,
    /// `missing` when normalization completed without one, `pending` before
    /// normalization ran.
    pub fingerprint_status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewArtworkDto {
    pub relative_path: String,
    pub source: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
    pub is_embedded: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSidecarDto {
    pub relative_path: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

/// Manifest summary for review: track inventory, artwork observations,
/// sidecars, and the finding count — never the raw payload blob.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSummaryDto {
    pub manifest_hash: String,
    pub schema_version: i32,
    pub generated_at: OffsetDateTime,
    pub source_path: String,
    pub release_title: Option<String>,
    pub release_artists: Vec<String>,
    pub tracks: Vec<ReviewTrackDto>,
    pub artwork: Vec<ReviewArtworkDto>,
    pub sidecars: Vec<ReviewSidecarDto>,
    pub finding_count: usize,
}

/// One classification evidence row (trait with its confidence).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationEvidenceDto {
    pub trait_: String,
    pub source: String,
    pub kind: String,
    pub confidence: u16,
    pub detail: String,
}

/// Persisted classification evidence for the bound Release.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationDto {
    pub traits: Vec<String>,
    pub confidence: u16,
    pub classified_at: OffsetDateTime,
    pub classifier_version: String,
    pub evidence: Vec<ClassificationEvidenceDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoveltyTrackDto {
    pub track_id: Uuid,
    pub category: String,
    pub confidence: u16,
    pub evidence_kind: String,
    pub detail: String,
}

/// Catalog comparison aggregates (Known/New/Alternate/Uncertain) grouped
/// from the persisted per-Track novelty categories, plus the durable counts
/// and per-Track rows exactly as the novelty evaluation recorded them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogComparisonDto {
    pub known: u32,
    pub new: u32,
    pub alternate: u32,
    pub uncertain: u32,
    pub exact_duplicates: u32,
    pub known_audio_identities: u32,
    pub alternate_recordings: u32,
    pub new_tracks: u32,
    pub uncertain_tracks: u32,
    pub total_tracks: u32,
    pub evaluated_at: OffsetDateTime,
    pub evaluator_version: String,
    pub tracks: Vec<NoveltyTrackDto>,
}

/// The durable policy decision for the bound Release, including the matched
/// rule ids, the evidence summary, and the override reason when overridden.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionDto {
    pub id: Uuid,
    pub release_id: Uuid,
    pub rule_set_id: Uuid,
    pub rule_set_version: u32,
    pub disposition: String,
    pub completion_state: String,
    pub matched_rule_ids: Vec<String>,
    pub evidence_summary: String,
    pub override_reason: Option<String>,
    pub actor: serde_json::Value,
    pub decided_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

/// Full release review detail.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDetailDto {
    pub work_item: WorkItemDto,
    pub release_id: Option<Uuid>,
    pub manifest: Option<ManifestSummaryDto>,
    pub classification: Option<ClassificationDto>,
    pub catalog_comparison: Option<CatalogComparisonDto>,
    pub decision: Option<DecisionDto>,
    pub timeline: Vec<WorkItemEventDto>,
}

/// Operator decision request. `reason` is required and audited (SPEC §20).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRequest {
    pub action: String,
    pub reason: String,
    /// Explicit destructive acknowledgement; required when `action` is
    /// `delete` so a destructive authorization is never recorded by default.
    #[serde(default, rename = "override")]
    pub override_: bool,
}

/// Operator reprocess request; the reason is optional and audited.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReprocessRequest {
    pub reason: Option<String>,
}

/// Result of an applied review decision.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDecisionResultDto {
    pub work_item: WorkItemDto,
    pub decision: Option<DecisionDto>,
    /// True when a destructive (delete) authorization was recorded and its
    /// execution remains pending in the durable pipeline.
    pub pending_execution: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewAction {
    Accept,
    Quarantine,
    Ignore,
    Delete,
}

impl ReviewAction {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "accept" => Some(Self::Accept),
            "quarantine" => Some(Self::Quarantine),
            "ignore" => Some(Self::Ignore),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }

    const fn disposition(self) -> Disposition {
        match self {
            Self::Accept => Disposition::Accept,
            Self::Quarantine => Disposition::Quarantine,
            Self::Ignore => Disposition::Ignore,
            Self::Delete => Disposition::Delete,
        }
    }
}

pub fn router() -> Router<AdminReviewState> {
    Router::new()
        .route("/api/admin/v1/review/queue", get(review_queue))
        .route("/api/admin/v1/review/{workItemId}", get(review_detail))
        .route(
            "/api/admin/v1/review/{workItemId}/decision",
            post(review_decision),
        )
        .route(
            "/api/admin/v1/review/{workItemId}/reprocess",
            post(review_reprocess),
        )
        .route(
            "/api/admin/v1/review/{workItemId}/tracks/{trackIndex}/preview",
            get(track_preview),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

/// GET /api/admin/v1/review/queue — review-able work items with counts.
async fn review_queue(
    State(state): State<AdminReviewState>,
    _admin: AdminPrincipal,
    Query(params): Query<ReviewQueueParams>,
) -> Response {
    let states: Vec<IngestionState> = match params.state.as_deref() {
        None => REVIEW_STATES.to_vec(),
        Some(raw) => {
            let Some(parsed) = IngestionState::parse(raw) else {
                return review_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_state",
                    format!("unknown state '{raw}'"),
                );
            };
            if !REVIEW_STATES.contains(&parsed) {
                return review_error(
                    StatusCode::BAD_REQUEST,
                    "state_not_reviewable",
                    format!("state '{raw}' is not a review-able state"),
                );
            }
            vec![parsed]
        }
    };
    let sort = match params.sort.as_deref() {
        None => WorkItemSort::default_order(),
        Some(raw) => match WorkItemSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return review_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    let pool = &state.admin.pool;
    let (items, total) = match WorkItemStore
        .list_page_states(pool, &states, limit, offset, sort)
        .await
    {
        Ok(page) => page,
        Err(error) => return store_failure("review queue listing failed", &error),
    };
    let counts = match WorkItemStore.counts_by_state(pool).await {
        Ok(counts) => counts
            .into_iter()
            .filter(|count| REVIEW_STATES.contains(&count.state))
            .map(StateCountDto::from)
            .collect(),
        Err(error) => return store_failure("review queue counts failed", &error),
    };
    let item_ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();
    let bindings = match CatalogStore.list_import_release_ids(pool, &item_ids).await {
        Ok(bindings) => bindings,
        Err(error) => return store_failure("review queue bindings failed", &error),
    };
    let release_by_item: std::collections::BTreeMap<Uuid, Uuid> = bindings.into_iter().collect();
    let rows = items
        .into_iter()
        .map(|item| ReviewQueueItemDto {
            release_id: release_by_item.get(&item.id).copied(),
            item: WorkItemDto::from(item),
        })
        .collect();
    Json(ReviewQueuePageDto {
        items: rows,
        total,
        limit,
        offset,
        counts_by_state: counts,
    })
    .into_response()
}

/// GET /api/admin/v1/review/{workItemId} — full review detail.
async fn review_detail(
    State(state): State<AdminReviewState>,
    _admin: AdminPrincipal,
    AxumPath(work_item_id): AxumPath<Uuid>,
) -> Response {
    match load_review_detail(&state.admin.pool, work_item_id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(response) => *response,
    }
}

async fn load_review_detail(
    pool: &sqlx::PgPool,
    work_item_id: Uuid,
) -> Result<ReviewDetailDto, Box<Response>> {
    let item = WorkItemStore
        .get(pool, work_item_id)
        .await
        .map_err(|error| match error {
            WorkStoreError::NotFound(id) => review_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("work item not found: {id}"),
            ),
            other => store_failure("work item read failed", &other),
        })?;
    let manifest = match WorkItemStore.get_manifest(pool, item.id).await {
        Ok(Some(stored)) => {
            let parsed: ReleaseManifest = serde_json::from_value(stored.payload.clone())
                .map_err(|error| store_failure("stored manifest is not parseable", &error))?;
            Some(manifest_summary(&stored, &parsed))
        }
        Ok(None) => None,
        Err(error) => return Err(Box::new(store_failure("manifest read failed", &error))),
    };
    let events = WorkItemStore
        .list_events(pool, item.id)
        .await
        .map_err(|error| store_failure("work item history read failed", &error))?;
    let release_id = match CatalogStore.get_import(pool, item.id).await {
        Ok(Some(import)) => Some(import.release_id),
        Ok(None) => None,
        Err(error) => {
            return Err(Box::new(store_failure(
                "catalog binding read failed",
                &error,
            )));
        }
    };
    let (classification, catalog_comparison, decision) = if let Some(release_uuid) = release_id {
        let release_id = melodee_domain::ReleaseId::from_uuid(release_uuid);
        let classification = PolicyStore
            .load_classification(pool, release_id)
            .await
            .map_err(|error| store_failure("classification read failed", &error))?
            .map(classification_dto);
        let comparison = PolicyStore
            .load_novelty(pool, release_id)
            .await
            .map_err(|error| store_failure("novelty read failed", &error))?
            .map(comparison_dto);
        let decision = PolicyStore
            .get_decision(pool, release_id)
            .await
            .map_err(|error| store_failure("decision read failed", &error))?
            .map(|decision| decision_dto(&decision));
        (classification, comparison, decision)
    } else {
        (None, None, None)
    };
    Ok(ReviewDetailDto {
        work_item: WorkItemDto::from(item),
        release_id,
        manifest,
        classification,
        catalog_comparison,
        decision,
        timeline: events.into_iter().map(WorkItemEventDto::from).collect(),
    })
}

/// POST /api/admin/v1/review/{workItemId}/decision — audited decision.
async fn review_decision(
    State(state): State<AdminReviewState>,
    admin: AdminPrincipal,
    AxumPath(work_item_id): AxumPath<Uuid>,
    Json(request): Json<DecisionRequest>,
) -> Response {
    let reason = request.reason.trim();
    if reason.is_empty() {
        return review_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let Some(action) = ReviewAction::parse(&request.action) else {
        return review_error(
            StatusCode::BAD_REQUEST,
            "unknown_action",
            format!("unknown action '{}'", request.action),
        );
    };
    if action == ReviewAction::Delete && !request.override_ {
        return review_error(
            StatusCode::BAD_REQUEST,
            "override_required",
            "delete requires explicit `\"override\": true` acknowledgement",
        );
    }
    let pool = &state.admin.pool;
    let item = match WorkItemStore.get(pool, work_item_id).await {
        Ok(item) => item,
        Err(WorkStoreError::NotFound(id)) => {
            return review_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("work item not found: {id}"),
            );
        }
        Err(error) => return store_failure("work item read failed", &error),
    };
    if item.state != IngestionState::NeedsReview {
        return review_error(
            StatusCode::CONFLICT,
            "invalid_state",
            format!(
                "work item is '{}' and not awaiting review",
                item.state.as_str()
            ),
        );
    }
    let now = OffsetDateTime::now_utc();
    let import = match CatalogStore.get_import(pool, item.id).await {
        Ok(import) => import,
        Err(error) => return store_failure("catalog binding read failed", &error),
    };
    let decision = match &import {
        Some(import) => match PolicyStore
            .get_decision(
                pool,
                melodee_domain::ReleaseId::from_uuid(import.release_id),
            )
            .await
        {
            Ok(decision) => decision,
            Err(error) => return store_failure("decision read failed", &error),
        },
        None => None,
    };
    let pending_decision =
        decision.filter(|decision| decision.completion_state == DecisionCompletionState::Pending);

    if let Some(pending) = pending_decision {
        return apply_policy_override(&state, &item, &pending, action, reason, &admin, now).await;
    }

    // Validation-parked items predate catalog import and have no durable
    // policy decision. Accept and delete both require that durable decision
    // context (SPEC §15/§20: a decision record must exist before any
    // destructive execution, and accept routes through policy evaluation);
    // quarantine/ignore are honest operator state-machine moves.
    let target = match action {
        ReviewAction::Accept => {
            return review_error(
                StatusCode::CONFLICT,
                "accept_unavailable",
                "accept requires a durable pending policy decision; this work item has not completed catalog import and policy evaluation",
            );
        }
        ReviewAction::Delete => {
            return review_error(
                StatusCode::CONFLICT,
                "delete_requires_policy_decision",
                "delete requires a durable policy decision record; classification is uncertain for this pre-catalog work item",
            );
        }
        ReviewAction::Quarantine => IngestionState::Quarantined,
        ReviewAction::Ignore => IngestionState::Ignored,
    };
    match WorkItemStore
        .transition(
            pool,
            item.id,
            None,
            TransitionKind::Operator,
            target,
            Some(reason),
            now,
        )
        .await
    {
        Ok(updated) => {
            tracing::info!(
                actor = %admin.public_api_id,
                work_item = %item.id,
                action = request.action,
                reason,
                "admin review decision applied"
            );
            Json(ReviewDecisionResultDto {
                work_item: WorkItemDto::from(updated),
                decision: None,
                pending_execution: false,
            })
            .into_response()
        }
        Err(error) => work_transition_error(item.id, error),
    }
}

async fn apply_policy_override(
    state: &AdminReviewState,
    item: &melodee_domain::ReleaseWorkItem,
    pending: &StoredPolicyDecision,
    action: ReviewAction,
    reason: &str,
    admin: &AdminPrincipal,
    now: OffsetDateTime,
) -> Response {
    let pool = &state.admin.pool;
    let disposition = action.disposition();
    let request = PolicyAdminOverrideRequest {
        work_item_id: item.id,
        decision_id: pending.id,
        chosen_disposition: disposition,
        actor: admin.public_api_id.to_string(),
        reason: reason.to_string(),
        source_fingerprint: pending.source_fingerprint.clone(),
        now,
    };
    match PolicyStore.apply_admin_override(pool, &request).await {
        Ok(result) => {
            tracing::info!(
                actor = %admin.public_api_id,
                work_item = %item.id,
                decision_id = %pending.id,
                disposition = disposition.as_str(),
                reason,
                "admin policy override applied"
            );
            if disposition == Disposition::Delete {
                return execute_release_deletion(state, result.decision).await;
            }
            Json(ReviewDecisionResultDto {
                work_item: WorkItemDto::from(result.item),
                decision: Some(decision_dto(&result.decision)),
                pending_execution: false,
            })
            .into_response()
        }
        Err(error) => policy_error(item.id, error),
    }
}

async fn execute_release_deletion(
    state: &AdminReviewState,
    decision: StoredPolicyDecision,
) -> Response {
    let Some(canonical_root) = state.deletion.canonical_root.as_deref() else {
        return review_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "delete_storage_unavailable",
            "delete authorization is pending because canonical storage is not configured",
        );
    };
    let publication = match WorkItemStore
        .get_publication(&state.admin.pool, decision.work_item_id)
        .await
    {
        Ok(Some(publication)) => publication,
        Ok(None) => {
            return review_error(
                StatusCode::CONFLICT,
                "delete_publication_missing",
                "delete authorization is pending because canonical publication evidence is missing",
            );
        }
        Err(error) => return store_failure("publication read failed", &error),
    };
    let handoff = match WorkItemStore
        .staging_handoff(&state.admin.pool, decision.work_item_id)
        .await
    {
        Ok(handoff) => handoff,
        Err(error) => return store_failure("staging handoff read failed", &error),
    };
    let staging_relative_path = handoff
        .as_ref()
        .map(|handoff| handoff.staging_relative_path.as_str());
    let deleted = match melodee_ingestion::delete_release_storage(
        canonical_root,
        &publication.plan.target_relative_path,
        state.deletion.staging_root.as_deref(),
        staging_relative_path,
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(error) => {
            tracing::error!(
                work_item = %decision.work_item_id,
                release_id = %decision.release_id,
                error = ?error,
                "confirmed Release deletion failed; authorization remains pending"
            );
            return review_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_execution_failed",
                "Release deletion failed and remains pending; inspect the server log and retry",
            );
        }
    };
    let (actor, reason) = match (&decision.actor, decision.override_reason.as_deref()) {
        (melodee_domain::PolicyActor::Administrator { actor }, Some(reason)) => {
            (actor.clone(), reason.to_owned())
        }
        _ => {
            return review_error(
                StatusCode::CONFLICT,
                "delete_authorization_invalid",
                "delete authorization is not an administrator-confirmed decision",
            );
        }
    };
    let completion = PolicyDestructiveCompletion {
        work_item_id: decision.work_item_id,
        decision_id: decision.id,
        actor,
        reason,
        source_fingerprint: decision.source_fingerprint.clone(),
        canonical_relative_path: deleted.canonical_relative_path,
        staging_relative_path: deleted.staging_relative_path,
        completed_at: OffsetDateTime::now_utc(),
    };
    match PolicyStore
        .complete_destructive_action(&state.admin.pool, &completion)
        .await
    {
        Ok(result) => {
            tracing::info!(
                work_item = %result.item.id,
                release_id = %result.decision.release_id,
                "confirmed Release deletion completed"
            );
            Json(ReviewDecisionResultDto {
                work_item: WorkItemDto::from(result.item),
                decision: Some(decision_dto(&result.decision)),
                pending_execution: false,
            })
            .into_response()
        }
        Err(error) => policy_error(decision.work_item_id, error),
    }
}

fn policy_error(work_item_id: Uuid, error: PolicyStoreError) -> Response {
    match error {
        PolicyStoreError::AuthorizationRequired => review_error(
            StatusCode::CONFLICT,
            "decision_not_actionable",
            format!("work item {work_item_id} is not awaiting a pending decision"),
        ),
        PolicyStoreError::DecisionConflict => review_error(
            StatusCode::CONFLICT,
            "decision_conflict",
            "the durable decision changed concurrently; reload and retry",
        ),
        PolicyStoreError::FingerprintMismatch => review_error(
            StatusCode::CONFLICT,
            "fingerprint_mismatch",
            "the catalog source fingerprint no longer matches the decision",
        ),
        PolicyStoreError::NotCataloged(id) => review_error(
            StatusCode::CONFLICT,
            "not_cataloged",
            format!("work item {id} is not in the cataloged policy stage"),
        ),
        PolicyStoreError::Work(WorkStoreError::NotFound(id)) => review_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("work item not found: {id}"),
        ),
        PolicyStoreError::Work(WorkStoreError::InvalidState(id, state)) => review_error(
            StatusCode::CONFLICT,
            "invalid_state",
            format!("work item {id} is in state '{}'", state.as_str()),
        ),
        other => store_failure("policy override failed", &other),
    }
}

fn work_transition_error(work_item_id: Uuid, error: WorkStoreError) -> Response {
    match error {
        WorkStoreError::NotFound(id) => review_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("work item not found: {id}"),
        ),
        WorkStoreError::InvalidState(_, state) => review_error(
            StatusCode::CONFLICT,
            "invalid_state",
            format!(
                "state '{}' cannot take this operator transition",
                state.as_str()
            ),
        ),
        WorkStoreError::LeaseConflict(_) => review_error(
            StatusCode::CONFLICT,
            "lease_conflict",
            format!("work item {work_item_id} is leased by an active worker"),
        ),
        other => store_failure("work item transition failed", &other),
    }
}

/// POST /api/admin/v1/review/{workItemId}/reprocess — operator requeue of a
/// failed or quarantined work item (operator arm of the state machine:
/// `failed|quarantined -> ready`).
async fn review_reprocess(
    State(state): State<AdminReviewState>,
    admin: AdminPrincipal,
    AxumPath(work_item_id): AxumPath<Uuid>,
    Json(request): Json<Option<ReprocessRequest>>,
) -> Response {
    let pool = &state.admin.pool;
    let item = match WorkItemStore.get(pool, work_item_id).await {
        Ok(item) => item,
        Err(WorkStoreError::NotFound(id)) => {
            return review_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("work item not found: {id}"),
            );
        }
        Err(error) => return store_failure("work item read failed", &error),
    };
    if !matches!(
        item.state,
        IngestionState::Failed | IngestionState::Quarantined
    ) {
        return review_error(
            StatusCode::CONFLICT,
            "not_reprocessable",
            format!(
                "work item is '{}'; only failed or quarantined items can be reprocessed",
                item.state.as_str()
            ),
        );
    }
    let reason = request
        .and_then(|request| request.reason)
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());
    let now = OffsetDateTime::now_utc();
    match WorkItemStore
        .transition(
            pool,
            item.id,
            None,
            TransitionKind::Operator,
            IngestionState::Ready,
            Some(reason.as_deref().unwrap_or("operator reprocess")),
            now,
        )
        .await
    {
        Ok(updated) => {
            tracing::info!(
                actor = %admin.public_api_id,
                work_item = %item.id,
                from_state = %item.state.as_str(),
                "admin reprocessed work item"
            );
            Json(WorkItemDto::from(updated)).into_response()
        }
        Err(error) => work_transition_error(item.id, error),
    }
}

/// GET /api/admin/v1/review/{workItemId}/tracks/{trackIndex}/preview —
/// operator-gated pre-publication audio preview with Range support. The file
/// is located exclusively from the durable manifest and configured roots.
async fn track_preview(
    State(state): State<AdminReviewState>,
    admin: AdminPrincipal,
    AxumPath((work_item_id, track_index)): AxumPath<(Uuid, u32)>,
    headers: HeaderMap,
) -> Response {
    let pool = &state.admin.pool;
    let item = match WorkItemStore.get(pool, work_item_id).await {
        Ok(item) => item,
        Err(WorkStoreError::NotFound(id)) => {
            return review_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("work item not found: {id}"),
            );
        }
        Err(error) => return store_failure("work item read failed", &error),
    };
    let stored = match WorkItemStore.get_manifest(pool, item.id).await {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return review_error(
                StatusCode::NOT_FOUND,
                "preview_unavailable",
                "no inspection manifest is recorded for this work item",
            );
        }
        Err(error) => return store_failure("manifest read failed", &error),
    };
    let manifest: ReleaseManifest = match serde_json::from_value(stored.payload) {
        Ok(manifest) => manifest,
        Err(error) => return store_failure("stored manifest is not parseable", &error),
    };
    let Ok(index) = usize::try_from(track_index) else {
        return preview_not_found();
    };
    let Some(track) = manifest.tracks.get(index) else {
        return preview_not_found();
    };

    let candidates = preview_candidates(&manifest, item.id, track, &state.preview);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    for candidate in candidates {
        let Some(canonical_base) = contained_base(&candidate.base, &candidate.contained_under)
        else {
            continue;
        };
        let Ok(root) = CanonicalAssetRoot::open(&canonical_base) else {
            continue;
        };
        let Ok(Some(byte_size)) = root.relative_len(&candidate.relative_path) else {
            continue;
        };
        let asset = PlayableAsset {
            canonical_relative_path: candidate.relative_path,
            byte_size,
            mime_type: candidate.mime_type,
        };
        let Some(service) = DirectPlayService::new(root, PREVIEW_STREAM_CONCURRENCY) else {
            return store_failure("preview stream service unavailable", &"zero concurrency");
        };
        return match service.response(&asset, range_header, false).await {
            Ok(mut response) => {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                tracing::info!(
                    actor = %admin.public_api_id,
                    work_item = %item.id,
                    track_index,
                    ranged = range_header.is_some(),
                    "admin previewed pre-publication track audio"
                );
                response
            }
            Err(StreamError::InvalidRange) => review_error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "the requested byte range is invalid for this asset",
            ),
            Err(StreamError::CapacityClosed) => review_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "stream_unavailable",
                "preview capacity is unavailable",
            ),
            Err(StreamError::AssetUnavailable) => preview_not_found(),
            Err(StreamError::InvalidAssetMetadata) => {
                store_failure("preview asset metadata is invalid", &"mime/size")
            }
        };
    }
    preview_not_found()
}

fn preview_not_found() -> Response {
    review_error(
        StatusCode::NOT_FOUND,
        "preview_unavailable",
        "no previewable track file is available under the configured roots",
    )
}

/// One candidate location for a track's previewable bytes.
struct PreviewCandidate {
    base: PathBuf,
    /// Configured root the canonicalized base must remain inside.
    contained_under: PathBuf,
    relative_path: String,
    mime_type: String,
}

/// Locates preview candidates in preference order: the normalized work-area
/// output first (that is what would be published), then the immutable staged
/// source. Nothing here trusts client
/// input; every path comes from the durable manifest or server config.
fn preview_candidates(
    manifest: &ReleaseManifest,
    work_item_id: Uuid,
    track: &melodee_domain::ObservedTrack,
    preview: &ReviewPreview,
) -> Vec<PreviewCandidate> {
    let mut candidates = Vec::new();
    if let Some(normalization) = &manifest.normalization
        && let Some(work_area) = normalization.provenance.work_area.as_deref()
        && let Some(output) = normalization
            .outputs
            .iter()
            .find(|output| output.source_file == track.file_path)
    {
        candidates.push(PreviewCandidate {
            base: PathBuf::from(work_area),
            contained_under: preview.work_area_base.clone(),
            relative_path: output.output_path.clone(),
            mime_type: mime_for_container(&output.container),
        });
    }
    if let Some(staging_root) = &preview.staging_root {
        let mime_type = manifest
            .files
            .iter()
            .find(|file| file.relative_path == track.file_path)
            .and_then(|file| file.mime_type.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        candidates.push(PreviewCandidate {
            base: staging_root
                .join("releases")
                .join(work_item_id.to_string())
                .join("source"),
            contained_under: staging_root.clone(),
            relative_path: track.file_path.clone(),
            mime_type,
        });
    }
    candidates
}

/// Canonicalizes the candidate base and requires it to stay inside the
/// canonicalized configured root. Missing directories simply do not match.
fn contained_base(base: &Path, root: &Path) -> Option<PathBuf> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let canonical_base = std::fs::canonicalize(base).ok()?;
    canonical_base
        .starts_with(&canonical_root)
        .then_some(canonical_base)
}

/// Content-Type for a normalized output, from the probed container label.
fn mime_for_container(container: &str) -> String {
    match container.to_ascii_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "wav" | "wave" => "audio/wav",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn manifest_summary(
    stored: &melodee_persistence::StoredManifest,
    manifest: &ReleaseManifest,
) -> ManifestSummaryDto {
    let tracks = manifest
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            let file = manifest
                .files
                .iter()
                .find(|file| file.relative_path == track.file_path);
            let fingerprint_status = match &manifest.normalization {
                None => "pending",
                Some(normalization) => {
                    if normalization
                        .fingerprints
                        .iter()
                        .any(|fingerprint| fingerprint.source_file == track.file_path)
                    {
                        "captured"
                    } else {
                        "missing"
                    }
                }
            };
            ReviewTrackDto {
                index: u32::try_from(index).unwrap_or(u32::MAX),
                file_path: track.file_path.clone(),
                title: track.track_title.clone(),
                duration_ms: track.duration_ms,
                container: file.and_then(|file| {
                    file.audio
                        .as_ref()
                        .and_then(|audio| audio.container.clone())
                }),
                codec: file
                    .and_then(|file| file.audio.as_ref().and_then(|audio| audio.codec.clone())),
                mime_type: file.and_then(|file| file.mime_type.clone()),
                fingerprint_status,
            }
        })
        .collect();
    ManifestSummaryDto {
        manifest_hash: stored.manifest_hash.clone(),
        schema_version: stored.schema_version,
        generated_at: stored.generated_at,
        source_path: manifest.source_path.clone(),
        release_title: manifest.release_observation.title.clone(),
        release_artists: manifest.release_observation.artists.clone(),
        tracks,
        artwork: manifest
            .artwork
            .iter()
            .map(|artwork| ReviewArtworkDto {
                relative_path: artwork.relative_path.clone(),
                source: artwork.source.clone(),
                width: artwork.width,
                height: artwork.height,
                size_bytes: artwork.size_bytes,
                mime_type: artwork.mime_type.clone(),
                is_embedded: artwork.is_embedded,
            })
            .collect(),
        sidecars: manifest
            .sidecars
            .iter()
            .map(|sidecar| ReviewSidecarDto {
                relative_path: sidecar.relative_path.clone(),
                size_bytes: sidecar.size_bytes,
                mime_type: sidecar.mime_type.clone(),
            })
            .collect(),
        finding_count: manifest.findings.len(),
    }
}

fn classification_dto(stored: StoredReleaseClassification) -> ClassificationDto {
    ClassificationDto {
        traits: stored
            .traits
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        confidence: stored.confidence.basis_points(),
        classified_at: stored.classified_at,
        classifier_version: stored.classifier_version,
        evidence: stored
            .evidence
            .iter()
            .map(|evidence| ClassificationEvidenceDto {
                trait_: evidence.trait_.as_str().to_string(),
                source: evidence.source.as_str().to_string(),
                kind: evidence.kind.as_str().to_string(),
                confidence: evidence.confidence.basis_points(),
                detail: evidence.detail.clone(),
            })
            .collect(),
    }
}

fn comparison_dto(stored: StoredNoveltyEvaluation) -> CatalogComparisonDto {
    CatalogComparisonDto {
        known: stored
            .exact_duplicate_count
            .saturating_add(stored.known_audio_identity_count),
        new: stored.new_track_count,
        alternate: stored.alternate_recording_count,
        uncertain: stored.uncertain_count,
        exact_duplicates: stored.exact_duplicate_count,
        known_audio_identities: stored.known_audio_identity_count,
        alternate_recordings: stored.alternate_recording_count,
        new_tracks: stored.new_track_count,
        uncertain_tracks: stored.uncertain_count,
        total_tracks: stored.total_track_count,
        evaluated_at: stored.evaluated_at,
        evaluator_version: stored.evaluator_version,
        tracks: stored
            .tracks
            .iter()
            .map(|track| NoveltyTrackDto {
                track_id: track.track_id.into_uuid(),
                category: track.category.as_str().to_string(),
                confidence: track.confidence.basis_points(),
                evidence_kind: track.evidence_kind.as_str().to_string(),
                detail: track.detail.clone(),
            })
            .collect(),
    }
}

fn decision_dto(stored: &StoredPolicyDecision) -> DecisionDto {
    DecisionDto {
        id: stored.id.into_uuid(),
        release_id: stored.release_id.into_uuid(),
        rule_set_id: stored.rule_set_id.into_uuid(),
        rule_set_version: stored.rule_set_version,
        disposition: stored.disposition.as_str().to_string(),
        completion_state: stored.completion_state.as_str().to_string(),
        matched_rule_ids: stored.matched_rule_ids.clone(),
        evidence_summary: stored.evidence_summary.clone(),
        override_reason: stored.override_reason.clone(),
        actor: serde_json::to_value(&stored.actor).unwrap_or(serde_json::Value::Null),
        decided_at: stored.decided_at,
        completed_at: stored.completed_at,
    }
}
