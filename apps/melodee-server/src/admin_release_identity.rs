//! Release identity correction API (ADR-0196, PRD-API-ADMIN-002).
//!
//! Wraps the existing durable, audited, reversible Release merge/split store
//! operations in the Admin API so operators and AI harnesses can act on the
//! Release duplicate evidence returned by
//! [`crate::admin_duplicates`]:
//!
//! * `POST /api/admin/v1/catalog/releases/merge/preview` — bounded impact
//!   preview before any mutation;
//! * `POST /api/admin/v1/catalog/releases/merge` — audited merge with a
//!   mandatory reason, preserving the source as a redirect tombstone;
//! * `POST /api/admin/v1/catalog/releases/{id}/split` — reverses exactly one
//!   recorded merge event using its immutable Track-ordinal map.
//!
//! Nothing here writes identity rows directly: execution goes through
//! `CatalogStore::merge_release` / `split_release_merge`, which retain every
//! observation, media asset, and Track, and record audit evidence.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use melodee_persistence::{CatalogStore, CatalogStoreError};
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeSelection {
    source_release_id: Uuid,
    target_release_id: Uuid,
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
struct SplitReleaseRequest {
    merge_event_id: Uuid,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSideDto {
    id: Uuid,
    title: String,
    display_artist: Option<String>,
    release_date: Option<String>,
    status: String,
    track_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeImpactDto {
    moved_tracks: i64,
    moved_observations: i64,
    moved_contributions: i64,
    target_track_count_after: i64,
    shared_audio_identities: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergePreviewDto {
    source: ReleaseSideDto,
    target: ReleaseSideDto,
    impact: ReleaseMergeImpactDto,
    metadata_conflicts: Vec<&'static str>,
    warnings: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseMergeResultDto {
    merge_event_id: Uuid,
    source_release_id: Uuid,
    target_release_id: Uuid,
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
    release_date_text: Option<String>,
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
            Option<String>,
            String,
            i64,
        ),
    >(
        "SELECT r.id, r.public_api_id, r.title, r.display_artist,
                r.release_date::text AS release_date, r.release_date_text,
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
                release_date_text,
                status,
                track_count,
            )| ReleaseSide {
                internal_id,
                public_api_id,
                title,
                display_artist,
                release_date,
                release_date_text,
                status,
                track_count,
            },
        )
    })
}

#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn resolve_pair(
    pool: &PgPool,
    selection: &MergeSelection,
) -> Result<(ReleaseSide, ReleaseSide), Box<Response>> {
    if selection.source_release_id == selection.target_release_id {
        return Err(Box::new(release_error(
            StatusCode::BAD_REQUEST,
            "self_merge",
            "source and target Releases must differ",
        )));
    }
    let source = load_release_side(pool, selection.source_release_id)
        .await
        .map_err(|error| {
            Box::new(store_failure(
                "Release lookup failed",
                &CatalogStoreError::Database(error),
            ))
        })?
        .ok_or_else(|| {
            Box::new(release_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("source Release not found: {}", selection.source_release_id),
            ))
        })?;
    let target = load_release_side(pool, selection.target_release_id)
        .await
        .map_err(|error| {
            Box::new(store_failure(
                "Release lookup failed",
                &CatalogStoreError::Database(error),
            ))
        })?
        .ok_or_else(|| {
            Box::new(release_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("target Release not found: {}", selection.target_release_id),
            ))
        })?;
    if source.status != "active" || target.status != "active" {
        return Err(Box::new(release_error(
            StatusCode::CONFLICT,
            "release_not_mergeable",
            "only two active Releases can be merged; a merged Release must be split first",
        )));
    }
    Ok((source, target))
}

async fn preview_release_merge(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Json(selection): Json<MergeSelection>,
) -> Response {
    let (source, target) = match resolve_pair(&state.pool, &selection).await {
        Ok(pair) => pair,
        Err(response) => return *response,
    };
    let counts = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT
             (SELECT count(*) FROM observed_artists WHERE release_id = $1),
             (SELECT count(*) FROM contributions WHERE release_id = $1),
             (SELECT count(*) FROM tracks WHERE release_id = $1),
             (SELECT count(DISTINCT source.audio_identity_id)
                FROM tracks source
                JOIN tracks target
                  ON target.audio_identity_id = source.audio_identity_id
                 AND target.release_id = $2
               WHERE source.release_id = $1)",
    )
    .bind(source.internal_id)
    .bind(target.internal_id)
    .fetch_one(&state.pool)
    .await;
    let (moved_observations, moved_contributions, moved_tracks, shared_audio_identities) =
        match counts {
            Ok(counts) => counts,
            Err(error) => {
                return store_failure(
                    "Release merge preview counts failed",
                    &CatalogStoreError::Database(error),
                );
            }
        };

    let mut metadata_conflicts = Vec::new();
    if source.title != target.title {
        metadata_conflicts.push("title");
    }
    if source.display_artist != target.display_artist {
        metadata_conflicts.push("displayArtist");
    }
    if source.release_date != target.release_date {
        metadata_conflicts.push("releaseDate");
    }
    if source.release_date_text != target.release_date_text {
        metadata_conflicts.push("releaseDateText");
    }
    let mut warnings = Vec::new();
    if moved_tracks == 0 {
        warnings.push("source has no tracks");
    }
    if shared_audio_identities > 0 {
        warnings.push("source and target share recordings; review duplicate Tracks after merge");
    }

    Json(ReleaseMergePreviewDto {
        source: ReleaseSideDto {
            id: source.public_api_id,
            title: source.title,
            display_artist: source.display_artist,
            release_date: source.release_date,
            status: source.status,
            track_count: source.track_count,
        },
        target: ReleaseSideDto {
            id: target.public_api_id,
            title: target.title,
            display_artist: target.display_artist,
            release_date: target.release_date,
            status: target.status,
            track_count: target.track_count,
        },
        impact: ReleaseMergeImpactDto {
            moved_tracks,
            moved_observations,
            moved_contributions,
            target_track_count_after: target.track_count.saturating_add(moved_tracks),
            shared_audio_identities,
        },
        metadata_conflicts,
        warnings,
    })
    .into_response()
}

async fn merge_release(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    headers: HeaderMap,
    Json(request): Json<MergeReleaseRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return release_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let selection = MergeSelection {
        source_release_id: request.source_release_id,
        target_release_id: request.target_release_id,
    };
    let (source, target) = match resolve_pair(&state.pool, &selection).await {
        Ok(pair) => pair,
        Err(response) => return *response,
    };
    match CatalogStore
        .merge_release(
            &state.pool,
            source.internal_id,
            target.internal_id,
            &admin.public_api_id.to_string(),
            &reason,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(merge_event_id) => {
            tracing::info!(
                actor = %admin.public_api_id,
                agent = agent_label(&headers).as_deref().unwrap_or("none"),
                source = %source.public_api_id,
                target = %target.public_api_id,
                "admin merged Release"
            );
            Json(ReleaseMergeResultDto {
                merge_event_id,
                source_release_id: source.public_api_id,
                target_release_id: target.public_api_id,
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
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
