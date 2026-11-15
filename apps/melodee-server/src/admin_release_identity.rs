//! Release identity correction API (ADR-0196, ADR-0198, PRD-API-ADMIN-002).
//!
//! Wraps the durable, audited, reversible Release merge/split store operations
//! in the Admin API so operators and AI harnesses can act on the Release
//! duplicate evidence returned by [`crate::admin_duplicates`], and so the Admin
//! UI can fold several Releases — from any Artist — into one:
//!
//! * `POST /api/admin/v1/catalog/releases/merge/preview` — bounded impact
//!   preview before any mutation;
//! * `POST /api/admin/v1/catalog/releases/merge` — single-source audited merge
//!   with a mandatory reason, preserving the source as a redirect tombstone;
//! * `POST /api/admin/v1/catalog/releases/merge/batch` — one-transaction merge
//!   of every selected source Release into one target;
//! * `POST /api/admin/v1/catalog/releases/{id}/split` — reverses exactly one
//!   recorded merge event using its immutable Track-ordinal map.
//!
//! Nothing here writes identity rows directly: execution goes through
//! `CatalogStore::merge_releases` / `split_release_merge`, which retain every
//! observation, media asset, Track, and piece of artwork evidence, and record
//! audit evidence. The surviving Release keeps its own metadata, Release-level
//! billing (Release artist), and artwork; sources contribute their Tracks and
//! only the artwork the target lacks.

use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use melodee_persistence::{
    CatalogStore, CatalogStoreError, MAX_RELEASE_MERGE_SOURCES, ReleaseMerge, ReleaseMergeEvent,
    ReleaseMergePreviewRelease,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};
use crate::admin_agent::agent_label;

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/catalog/releases/merge/preview",
            post(preview_release_merge),
        )
        .route("/api/admin/v1/catalog/releases/merge", post(merge_release))
        .route(
            "/api/admin/v1/catalog/releases/merge/batch",
            post(batch_merge_releases),
        )
        .route(
            "/api/admin/v1/catalog/releases/{id}/split",
            post(split_release_merge),
        )
}

fn release_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
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

fn store_failure(context: &'static str, error: &CatalogStoreError) -> Response {
    tracing::error!(%error, context, "admin release identity store failure");
    release_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

fn catalog_error(error: CatalogStoreError) -> Response {
    match error {
        CatalogStoreError::InvalidRequest => release_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid Release merge request",
        ),
        CatalogStoreError::MutationConflict => release_error(
            StatusCode::CONFLICT,
            "merge_conflict",
            "the requested Release change conflicts with persisted catalog data",
        ),
        CatalogStoreError::CorrectionScopeExceeded { dimension, limit } => release_error(
            StatusCode::CONFLICT,
            "scope_exceeded",
            format!("the change exceeds the {dimension} limit of {limit}; narrow the operation"),
        ),
        other => store_failure("Release identity mutation failed", &other),
    }
}

/// Preview/apply selection.
///
/// `sourceReleaseIds` is the canonical multi-source shape used by the Admin UI.
/// The single `sourceReleaseId` field is still accepted so clients built against
/// the original ADR-0196 contract keep working.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeSelection {
    #[serde(default)]
    source_release_id: Option<Uuid>,
    #[serde(default)]
    source_release_ids: Vec<Uuid>,
    target_release_id: Uuid,
}

impl MergeSelection {
    fn single(source_release_id: Uuid, target_release_id: Uuid) -> Self {
        Self {
            source_release_id: Some(source_release_id),
            source_release_ids: Vec::new(),
            target_release_id,
        }
    }

    /// Distinct, bounded source ids; the target can never be its own source.
    #[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
    fn validated(&self) -> Result<Vec<Uuid>, Box<Response>> {
        let mut requested = self.source_release_ids.clone();
        if let Some(single) = self.source_release_id {
            requested.push(single);
        }
        if requested.is_empty() || requested.len() > MAX_RELEASE_MERGE_SOURCES {
            return Err(Box::new(release_error(
                StatusCode::BAD_REQUEST,
                "invalid_sources",
                format!("select between 1 and {MAX_RELEASE_MERGE_SOURCES} source Releases"),
            )));
        }
        let distinct = requested.iter().copied().collect::<BTreeSet<_>>();
        if distinct.len() != requested.len() {
            return Err(Box::new(release_error(
                StatusCode::BAD_REQUEST,
                "duplicate_source",
                "source Release ids must be distinct",
            )));
        }
        if distinct.contains(&self.target_release_id) {
            return Err(Box::new(release_error(
                StatusCode::BAD_REQUEST,
                "self_merge",
                "the target Release cannot also be a source",
            )));
        }
        Ok(distinct.into_iter().collect())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeReleaseRequest {
    source_release_id: Uuid,
    target_release_id: Uuid,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchMergeReleaseRequest {
    source_release_ids: Vec<Uuid>,
    target_release_id: Uuid,
    reason: String,
}

impl BatchMergeReleaseRequest {
    fn selection(&self) -> MergeSelection {
        MergeSelection {
            source_release_id: None,
            source_release_ids: self.source_release_ids.clone(),
            target_release_id: self.target_release_id,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitReleaseRequest {
    merge_event_id: Uuid,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSideDto {
    id: Uuid,
    title: String,
    display_artist: Option<String>,
    release_date: Option<String>,
    status: String,
    track_count: i64,
    release_artist_count: i64,
    has_operator_image: bool,
    provider_image_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeImpactDto {
    moved_tracks: i64,
    moved_observations: i64,
    moved_contributions: i64,
    target_track_count_after: i64,
    shared_audio_identities: i64,
    /// Per-user Release state (star/rating/pin/play count) rows moved because
    /// the user had no state on the surviving Release.
    moved_user_states: i64,
    /// Source user-state rows kept on the tombstone because that user already
    /// has state on the surviving Release.
    retained_user_states: i64,
}

/// Artwork and Release-billing evidence the merge folds in.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeArtworkImpactDto {
    detached_release_artist_observations: i64,
    operator_images_adopted: i64,
    operator_images_preserved: i64,
    provider_images_adopted: i64,
    provider_image_conflicts: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergePreviewDto {
    /// First source Release. Kept for the original single-source contract;
    /// `sources` carries the complete selection.
    source: ReleaseSideDto,
    sources: Vec<ReleaseSideDto>,
    target: ReleaseSideDto,
    impact: ReleaseMergeImpactDto,
    artwork: ReleaseMergeArtworkImpactDto,
    metadata_conflicts: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeResultDto {
    merge_event_id: Uuid,
    source_release_id: Uuid,
    target_release_id: Uuid,
}

/// One committed source merge, so an operator can split exactly that source.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeEventDto {
    source_release_id: Uuid,
    merge_event_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchMergeReleaseResultDto {
    merge_events: Vec<ReleaseMergeEventDto>,
    target_release_id: Uuid,
    merge_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSplitResultDto {
    merge_event_id: Uuid,
    source_release_id: Uuid,
    target_release_id: Uuid,
    restored: bool,
}

struct ReleaseSide {
    internal_id: Uuid,
    public_api_id: Uuid,
    title: String,
    display_artist: Option<String>,
    release_date: Option<String>,
    status: String,
    track_count: i64,
}

async fn load_release_side(
    pool: &PgPool,
    public_api_id: Uuid,
) -> Result<Option<ReleaseSide>, sqlx::Error> {
    sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            Option<String>,
            Option<String>,
            String,
            i64,
        ),
    >(
        "SELECT r.id, r.public_api_id, r.title, r.display_artist,
                r.release_date::text AS release_date,
                r.status, COALESCE(aggregate.track_count, 0) AS track_count
           FROM releases r
           LEFT JOIN release_catalog_aggregates aggregate
             ON aggregate.release_id = r.id
          WHERE r.public_api_id = $1",
    )
    .bind(public_api_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map(
            |(
                internal_id,
                public_api_id,
                title,
                display_artist,
                release_date,
                status,
                track_count,
            )| ReleaseSide {
                internal_id,
                public_api_id,
                title,
                display_artist,
                release_date,
                status,
                track_count,
            },
        )
    })
}

/// Validated merge selection resolved to durable Release rows.
struct ResolvedSelection {
    sources: Vec<ReleaseSide>,
    target: ReleaseSide,
}

#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn resolve_selection(
    pool: &PgPool,
    selection: &MergeSelection,
) -> Result<ResolvedSelection, Box<Response>> {
    let source_ids = selection.validated()?;
    let lookup_failure = |error: sqlx::Error| {
        Box::new(store_failure(
            "Release lookup failed",
            &CatalogStoreError::Database(error),
        ))
    };
    let mut sources = Vec::with_capacity(source_ids.len());
    for source_id in source_ids {
        let source = load_release_side(pool, source_id)
            .await
            .map_err(lookup_failure)?
            .ok_or_else(|| {
                Box::new(release_error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("source Release not found: {source_id}"),
                ))
            })?;
        sources.push(source);
    }
    let target = load_release_side(pool, selection.target_release_id)
        .await
        .map_err(lookup_failure)?
        .ok_or_else(|| {
            Box::new(release_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("target Release not found: {}", selection.target_release_id),
            ))
        })?;
    if let Some(inactive) = sources
        .iter()
        .chain(std::iter::once(&target))
        .find(|release| release.status != "active")
    {
        return Err(Box::new(release_error(
            StatusCode::CONFLICT,
            "release_not_mergeable",
            format!(
                "Release {} is {}; only active Releases can be merged, so split a merged Release first",
                inactive.public_api_id, inactive.status
            ),
        )));
    }
    Ok(ResolvedSelection { sources, target })
}

fn side_dto(side: &ReleaseSide) -> ReleaseSideDto {
    ReleaseSideDto {
        id: side.public_api_id,
        title: side.title.clone(),
        display_artist: side.display_artist.clone(),
        release_date: side.release_date.clone(),
        status: side.status.clone(),
        track_count: side.track_count,
        release_artist_count: 0,
        has_operator_image: false,
        provider_image_count: 0,
    }
}

fn preview_side_dto(side: &ReleaseMergePreviewRelease) -> ReleaseSideDto {
    ReleaseSideDto {
        id: side.public_api_id,
        title: side.title.clone(),
        display_artist: side.display_artist.clone(),
        release_date: side.release_date.clone(),
        status: side.status.clone(),
        track_count: side.track_count,
        release_artist_count: side.release_artist_count,
        has_operator_image: side.has_operator_image,
        provider_image_count: side.provider_image_count,
    }
}

async fn preview_release_merge(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Json(selection): Json<MergeSelection>,
) -> Response {
    let resolved = match resolve_selection(&state.pool, &selection).await {
        Ok(resolved) => resolved,
        Err(response) => return *response,
    };
    let source_ids = resolved
        .sources
        .iter()
        .map(|source| source.internal_id)
        .collect::<Vec<_>>();
    match CatalogStore
        .preview_release_merge(&state.pool, &source_ids, resolved.target.internal_id)
        .await
    {
        Ok(preview) => {
            let sources = preview
                .sources
                .iter()
                .map(preview_side_dto)
                .collect::<Vec<_>>();
            let source = sources
                .first()
                .cloned()
                .unwrap_or_else(|| side_dto(&resolved.sources[0]));
            Json(ReleaseMergePreviewDto {
                source,
                sources,
                target: preview_side_dto(&preview.target),
                impact: ReleaseMergeImpactDto {
                    moved_tracks: preview.impact.moved_tracks,
                    moved_observations: preview.impact.moved_observations,
                    moved_contributions: preview.impact.moved_contributions,
                    target_track_count_after: preview.target_track_count_after,
                    shared_audio_identities: preview.impact.shared_audio_identities,
                    moved_user_states: preview.impact.moved_user_states,
                    retained_user_states: preview.impact.retained_user_states,
                },
                artwork: ReleaseMergeArtworkImpactDto {
                    detached_release_artist_observations: preview
                        .impact
                        .detached_release_artist_observations,
                    operator_images_adopted: preview.impact.operator_images_adopted,
                    operator_images_preserved: preview.impact.operator_images_preserved,
                    provider_images_adopted: preview.impact.provider_images_adopted,
                    provider_image_conflicts: preview.impact.provider_image_conflicts,
                },
                metadata_conflicts: preview.metadata_conflicts,
                warnings: preview.warnings,
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}

/// Committed merge events mapped back to the public source ids the caller sent.
struct CommittedMerge {
    events: Vec<(Uuid, Uuid)>,
    target_public_id: Uuid,
}

/// Shared merge execution: resolve, audit, and apply within one transaction.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn execute_merge(
    state: &AdminAccessState,
    admin: AdminPrincipal,
    headers: &HeaderMap,
    selection: &MergeSelection,
    reason: &str,
) -> Result<CommittedMerge, Response> {
    let reason = reason.trim().to_string();
    if reason.is_empty() {
        return Err(release_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        ));
    }
    let resolved = resolve_selection(&state.pool, selection)
        .await
        .map_err(|response| *response)?;
    let now = OffsetDateTime::now_utc();
    let merges = resolved
        .sources
        .iter()
        .map(|source| ReleaseMerge {
            source_release_id: source.internal_id,
            target_release_id: resolved.target.internal_id,
            actor: admin.public_api_id.to_string(),
            reason: reason.clone(),
            occurred_at: now,
        })
        .collect::<Vec<_>>();
    match CatalogStore.merge_releases(&state.pool, &merges).await {
        Ok(committed) => {
            tracing::info!(
                actor = %admin.public_api_id,
                agent = agent_label(headers).as_deref().unwrap_or("none"),
                source_count = merges.len(),
                target = %resolved.target.public_api_id,
                "admin merged Releases"
            );
            Ok(CommittedMerge {
                events: merge_events_to_public(&committed, &resolved),
                target_public_id: resolved.target.public_api_id,
            })
        }
        Err(error) => Err(catalog_error(error)),
    }
}

/// Maps durable (internal id) merge events to the caller's public source ids.
fn merge_events_to_public(
    committed: &[ReleaseMergeEvent],
    resolved: &ResolvedSelection,
) -> Vec<(Uuid, Uuid)> {
    committed
        .iter()
        .filter_map(|event| {
            resolved
                .sources
                .iter()
                .find(|source| source.internal_id == event.source_release_id)
                .map(|source| (source.public_api_id, event.merge_event_id))
        })
        .collect()
}

async fn merge_release(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    headers: HeaderMap,
    Json(request): Json<MergeReleaseRequest>,
) -> Response {
    let selection = MergeSelection::single(request.source_release_id, request.target_release_id);
    match execute_merge(&state, admin, &headers, &selection, &request.reason).await {
        Ok(committed) => {
            let Some((source_release_id, merge_event_id)) = committed.events.first().copied()
            else {
                return store_failure(
                    "Release merge returned no event",
                    &CatalogStoreError::InvalidRequest,
                );
            };
            Json(ReleaseMergeResultDto {
                merge_event_id,
                source_release_id,
                target_release_id: committed.target_public_id,
            })
            .into_response()
        }
        Err(response) => response,
    }
}

async fn batch_merge_releases(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    headers: HeaderMap,
    Json(request): Json<BatchMergeReleaseRequest>,
) -> Response {
    let selection = request.selection();
    match execute_merge(&state, admin, &headers, &selection, &request.reason).await {
        Ok(committed) => {
            let merge_count = committed.events.len();
            Json(BatchMergeReleaseResultDto {
                merge_events: committed
                    .events
                    .into_iter()
                    .map(|(source_release_id, merge_event_id)| ReleaseMergeEventDto {
                        source_release_id,
                        merge_event_id,
                    })
                    .collect(),
                target_release_id: committed.target_public_id,
                merge_count,
            })
            .into_response()
        }
        Err(response) => response,
    }
}

async fn split_release_merge(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<SplitReleaseRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return release_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let source = match load_release_side(&state.pool, id).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            return release_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("source Release not found: {id}"),
            );
        }
        Err(error) => {
            return store_failure("Release lookup failed", &CatalogStoreError::Database(error));
        }
    };
    let event = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT source_release_id, target_release_id
           FROM release_merge_events
          WHERE id = $1",
    )
    .bind(request.merge_event_id)
    .fetch_optional(&state.pool)
    .await;
    let (event_source, event_target) = match event {
        Ok(Some(event)) => event,
        Ok(None) => {
            return release_error(
                StatusCode::NOT_FOUND,
                "merge_event_not_found",
                format!("merge event not found: {}", request.merge_event_id),
            );
        }
        Err(error) => {
            return store_failure(
                "Release merge event lookup failed",
                &CatalogStoreError::Database(error),
            );
        }
    };
    if event_source != source.internal_id {
        return release_error(
            StatusCode::CONFLICT,
            "merge_event_release_mismatch",
            "the merge event does not belong to this source Release",
        );
    }
    let target_public_id: Uuid =
        match sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(event_target)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(Some(public_id)) => public_id,
            Ok(None) => {
                return release_error(
                    StatusCode::CONFLICT,
                    "merge_target_missing",
                    "the merge event target Release no longer exists",
                );
            }
            Err(error) => {
                return store_failure(
                    "Release merge target lookup failed",
                    &CatalogStoreError::Database(error),
                );
            }
        };
    match CatalogStore
        .split_release_merge(
            &state.pool,
            request.merge_event_id,
            &admin.public_api_id.to_string(),
            &reason,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                agent = agent_label(&headers).as_deref().unwrap_or("none"),
                source = %source.public_api_id,
                merge_event = %request.merge_event_id,
                "admin split Release merge"
            );
            Json(ReleaseSplitResultDto {
                merge_event_id: request.merge_event_id,
                source_release_id: source.public_api_id,
                target_release_id: target_public_id,
                restored: true,
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}
