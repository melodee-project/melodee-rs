//! Administrator Artist and Release deletion (ADR-0199).
//!
//! Two operator-selected scopes, both terminating in a durable tombstone and
//! both audited with the acting administrator and a mandatory reason:
//!
//! * `catalogOnly` — retire the Artist identity; keep every Release, credit,
//!   and media file untouched.
//! * `releasesAndFiles` — additionally retire every Release the selection owns
//!   alone (primary Release billing with no other primary Artist). Canonical
//!   and staged storage for those Releases is removed first, using only paths
//!   derived from durable publication/handoff evidence and the shared
//!   no-follow deletion helper. A failure leaves the catalog unchanged.
//!
//! The Release bulk action uses the same two scopes for an explicit Release
//! selection from **Catalog → Releases**, so both catalog lists expose delete
//! beside merge.
//!
//! Catalog evidence is never row-deleted: identities, credits, observations,
//! provider evidence, per-user state, and audit history stay attributable to
//! the deleted public ids.

use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use melodee_persistence::{
    ArtistDelete, ArtistDeleteMode, CatalogStore, CatalogStoreError, MAX_ARTIST_DELETE_SOURCES,
    MAX_RELEASE_DELETE_SOURCES, ReleaseDelete, ReleaseDeleteMode,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};
use crate::admin_agent::agent_label;

#[derive(Clone)]
pub struct AdminArtistDeleteState {
    pub admin: AdminAccessState,
    pub canonical_root: Option<PathBuf>,
    pub staging_root: Option<PathBuf>,
}

impl AdminArtistDeleteState {
    #[must_use]
    pub fn new(
        admin: AdminAccessState,
        canonical_root: Option<PathBuf>,
        staging_root: Option<PathBuf>,
    ) -> Self {
        Self {
            admin,
            canonical_root,
            staging_root,
        }
    }
}

/// The deletion surface shares the administrator gate; extraction delegates to
/// the canonical `AdminAccessState` implementation so 401/403 behavior is
/// identical on every admin route.
impl axum::extract::FromRequestParts<AdminArtistDeleteState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AdminArtistDeleteState,
    ) -> Result<Self, Self::Rejection> {
        <Self as axum::extract::FromRequestParts<AdminAccessState>>::from_request_parts(
            parts,
            &state.admin,
        )
        .await
    }
}

pub fn router() -> Router<AdminArtistDeleteState> {
    Router::new()
        .route(
            "/api/admin/v1/catalog/artists/delete/preview",
            post(preview_artist_delete),
        )
        .route("/api/admin/v1/catalog/artists/delete", post(delete_artists))
        .route(
            "/api/admin/v1/catalog/releases/delete/preview",
            post(preview_release_delete),
        )
        .route(
            "/api/admin/v1/catalog/releases/delete",
            post(delete_releases),
        )
}

fn delete_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
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

fn catalog_error(error: CatalogStoreError) -> Response {
    match error {
        CatalogStoreError::InvalidRequest => delete_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("select 1 to {MAX_ARTIST_DELETE_SOURCES} distinct entries of one entity type"),
        ),
        CatalogStoreError::MutationConflict => delete_error(
            StatusCode::CONFLICT,
            "not_deletable",
            "a selected entry is missing, already merged, deprecated, or already deleted",
        ),
        CatalogStoreError::CorrectionScopeExceeded { dimension, limit } => delete_error(
            StatusCode::CONFLICT,
            "scope_exceeded",
            format!("the deletion exceeds the {dimension} limit of {limit}; narrow the selection"),
        ),
        other => {
            tracing::error!(%other, "admin Artist deletion store failure");
            delete_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                "Artist deletion failed",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSelection {
    artist_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRequest {
    artist_ids: Vec<Uuid>,
    /// `catalogOnly` or `releasesAndFiles`.
    mode: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImpactDto {
    releases: i64,
    tracks: i64,
    media_files: i64,
    media_bytes: i64,
    retained_shared_releases: i64,
    release_credits: i64,
    track_credits: i64,
    contributions: i64,
    user_states: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewArtistDto {
    id: Uuid,
    canonical_name: String,
    status: String,
    owned_releases: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewDto {
    artists: Vec<PreviewArtistDto>,
    impact: ImpactDto,
    /// Owned Releases with durable publication evidence for storage removal.
    storage_ready_releases: i64,
    /// True when canonical storage is configured on this server.
    storage_configured: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResultDto {
    deleted_artist_ids: Vec<Uuid>,
    deleted_release_ids: Vec<Uuid>,
    retained_shared_releases: i64,
    mode: String,
}

async fn preview_artist_delete(
    State(state): State<AdminArtistDeleteState>,
    _admin: AdminPrincipal,
    Json(selection): Json<DeleteSelection>,
) -> Response {
    let preview = match CatalogStore
        .preview_artist_delete(&state.admin.pool, &selection.artist_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    let storage_ready_releases = match CatalogStore
        .artist_delete_release_storage(&state.admin.pool, &selection.artist_ids)
        .await
    {
        Ok(targets) => i64::try_from(targets.len()).unwrap_or(i64::MAX),
        Err(error) => return catalog_error(error),
    };
    Json(PreviewDto {
        artists: preview
            .artists
            .into_iter()
            .map(|artist| PreviewArtistDto {
                id: artist.public_api_id,
                canonical_name: artist.canonical_name,
                status: artist.status,
                owned_releases: artist.owned_releases,
            })
            .collect(),
        impact: ImpactDto {
            releases: preview.impact.releases,
            tracks: preview.impact.tracks,
            media_files: preview.impact.media_files,
            media_bytes: preview.impact.media_bytes,
            retained_shared_releases: preview.impact.retained_shared_releases,
            release_credits: preview.impact.release_credits,
            track_credits: preview.impact.track_credits,
            contributions: preview.impact.contributions,
            user_states: preview.impact.user_states,
        },
        storage_ready_releases,
        storage_configured: state.canonical_root.is_some(),
        warnings: preview.warnings,
    })
    .into_response()
}

async fn delete_artists(
    State(state): State<AdminArtistDeleteState>,
    admin: AdminPrincipal,
    headers: axum::http::HeaderMap,
    Json(request): Json<DeleteRequest>,
) -> Response {
    let reason = request.reason.trim().to_owned();
    if reason.is_empty() {
        return delete_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let mode = match ArtistDeleteMode::parse(&request.mode) {
        Ok(mode) => mode,
        Err(_) => {
            return delete_error(
                StatusCode::BAD_REQUEST,
                "invalid_mode",
                "mode must be catalogOnly or releasesAndFiles",
            );
        }
    };
    let preview = match CatalogStore
        .preview_artist_delete(&state.admin.pool, &request.artist_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    if mode == ArtistDeleteMode::ReleasesAndFiles {
        let Some(canonical_root) = state.canonical_root.as_deref() else {
            return delete_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "delete_storage_unavailable",
                "canonical storage is not configured; choose the catalog-only scope",
            );
        };
        let targets = match CatalogStore
            .artist_delete_release_storage(&state.admin.pool, &request.artist_ids)
            .await
        {
            Ok(targets) => targets,
            Err(error) => return catalog_error(error),
        };
        if i64::try_from(targets.len()).unwrap_or(i64::MAX) != preview.impact.releases {
            return delete_error(
                StatusCode::CONFLICT,
                "storage_evidence_missing",
                format!(
                    "{} of {} owned Releases have durable publication evidence; \
                     deletion would leave files behind, so nothing was changed",
                    targets.len(),
                    preview.impact.releases
                ),
            );
        }
        for target in &targets {
            if let Err(error) = melodee_ingestion::delete_release_storage(
                canonical_root,
                &target.canonical_relative_path,
                state.staging_root.as_deref(),
                target.staging_relative_path.as_deref(),
            )
            .await
            {
                tracing::error!(
                    release = %target.release_public_id,
                    work_item = %target.work_item_id,
                    %error,
                    "administrative Artist deletion storage removal failed; catalog unchanged"
                );
                return delete_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete_execution_failed",
                    "Release storage removal failed; no catalog change was made, inspect the server log and retry",
                );
            }
        }
    }
    let artists = match CatalogStore
        .artists_by_public_ids(&state.admin.pool, &request.artist_ids)
        .await
    {
        Ok(artists) => artists,
        Err(error) => return catalog_error(error),
    };
    if artists.len() != request.artist_ids.len() {
        return delete_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "one or more Artists were not found",
        );
    }
    let occurred_at = OffsetDateTime::now_utc();
    let actor = admin.public_api_id.to_string();
    let deletes = artists
        .into_iter()
        .map(|artist| ArtistDelete {
            artist_id: artist.id,
            public_api_id: artist.public_api_id,
            mode,
            actor: actor.clone(),
            reason: reason.clone(),
            occurred_at,
        })
        .collect::<Vec<_>>();
    match CatalogStore
        .delete_artists(&state.admin.pool, &deletes)
        .await
    {
        Ok(outcome) => {
            tracing::info!(
                actor = %admin.public_api_id,
                agent = agent_label(&headers).as_deref().unwrap_or("none"),
                mode = mode.as_str(),
                artists = outcome.deleted_artists.len(),
                releases = outcome.deleted_releases.len(),
                "admin deleted Artists"
            );
            Json(DeleteResultDto {
                deleted_artist_ids: outcome.deleted_artists,
                deleted_release_ids: outcome.deleted_releases,
                retained_shared_releases: outcome.retained_shared_releases,
                mode: mode.as_str().to_owned(),
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}

// ---- Release bulk deletion -------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDeleteSelection {
    release_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDeleteRequest {
    release_ids: Vec<Uuid>,
    /// `catalogOnly` or `releasesAndFiles`.
    mode: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleasePreviewReleaseDto {
    id: Uuid,
    title: String,
    display_artist: Option<String>,
    status: String,
    track_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleasePreviewDto {
    releases: Vec<ReleasePreviewReleaseDto>,
    impact: ImpactDto,
    /// Selected Releases with durable publication evidence for storage removal.
    storage_ready_releases: i64,
    storage_configured: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDeleteResultDto {
    deleted_release_ids: Vec<Uuid>,
    mode: String,
}

async fn preview_release_delete(
    State(state): State<AdminArtistDeleteState>,
    _admin: AdminPrincipal,
    Json(selection): Json<ReleaseDeleteSelection>,
) -> Response {
    let preview = match CatalogStore
        .preview_release_delete(&state.admin.pool, &selection.release_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    let storage_ready_releases = match CatalogStore
        .release_delete_storage(&state.admin.pool, &selection.release_ids)
        .await
    {
        Ok(targets) => i64::try_from(targets.len()).unwrap_or(i64::MAX),
        Err(error) => return catalog_error(error),
    };
    Json(ReleasePreviewDto {
        releases: preview
            .releases
            .into_iter()
            .map(|release| ReleasePreviewReleaseDto {
                id: release.public_api_id,
                title: release.title,
                display_artist: release.display_artist,
                status: release.status,
                track_count: release.track_count,
            })
            .collect(),
        impact: ImpactDto {
            releases: preview.impact.releases,
            tracks: preview.impact.tracks,
            media_files: preview.impact.media_files,
            media_bytes: preview.impact.media_bytes,
            retained_shared_releases: 0,
            release_credits: preview.impact.release_credits,
            track_credits: preview.impact.track_credits,
            contributions: preview.impact.contributions,
            user_states: preview.impact.user_states,
        },
        storage_ready_releases,
        storage_configured: state.canonical_root.is_some(),
        warnings: preview.warnings,
    })
    .into_response()
}

async fn delete_releases(
    State(state): State<AdminArtistDeleteState>,
    admin: AdminPrincipal,
    headers: axum::http::HeaderMap,
    Json(request): Json<ReleaseDeleteRequest>,
) -> Response {
    let reason = request.reason.trim().to_owned();
    if reason.is_empty() {
        return delete_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let mode = match ReleaseDeleteMode::parse(&request.mode) {
        Ok(mode) => mode,
        Err(_) => {
            return delete_error(
                StatusCode::BAD_REQUEST,
                "invalid_mode",
                "mode must be catalogOnly or releasesAndFiles",
            );
        }
    };
    let preview = match CatalogStore
        .preview_release_delete(&state.admin.pool, &request.release_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    if request.release_ids.len() > MAX_RELEASE_DELETE_SOURCES {
        return delete_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("select at most {MAX_RELEASE_DELETE_SOURCES} Releases"),
        );
    }
    if mode == ReleaseDeleteMode::ReleasesAndFiles {
        let Some(canonical_root) = state.canonical_root.as_deref() else {
            return delete_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "delete_storage_unavailable",
                "canonical storage is not configured; choose the catalog-only scope",
            );
        };
        let targets = match CatalogStore
            .release_delete_storage(&state.admin.pool, &request.release_ids)
            .await
        {
            Ok(targets) => targets,
            Err(error) => return catalog_error(error),
        };
        if i64::try_from(targets.len()).unwrap_or(i64::MAX) != preview.impact.releases {
            return delete_error(
                StatusCode::CONFLICT,
                "storage_evidence_missing",
                format!(
                    "{} of {} selected Releases have durable publication evidence; \
                     deletion would leave files behind, so nothing was changed",
                    targets.len(),
                    preview.impact.releases
                ),
            );
        }
        for target in &targets {
            if let Err(error) = melodee_ingestion::delete_release_storage(
                canonical_root,
                &target.canonical_relative_path,
                state.staging_root.as_deref(),
                target.staging_relative_path.as_deref(),
            )
            .await
            {
                tracing::error!(
                    release = %target.release_public_id,
                    work_item = %target.work_item_id,
                    %error,
                    "administrative Release deletion storage removal failed; catalog unchanged"
                );
                return delete_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete_execution_failed",
                    "Release storage removal failed; no catalog change was made, inspect the server log and retry",
                );
            }
        }
    }
    let releases = match CatalogStore
        .release_delete_refs(&state.admin.pool, &request.release_ids)
        .await
    {
        Ok(releases) => releases,
        Err(error) => return catalog_error(error),
    };
    if releases.len() != request.release_ids.len() {
        return delete_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "one or more Releases were not found",
        );
    }
    let occurred_at = OffsetDateTime::now_utc();
    let actor = admin.public_api_id.to_string();
    let deletes = releases
        .into_iter()
        .map(|(release_id, public_api_id)| ReleaseDelete {
            release_id,
            public_api_id,
            mode,
            actor: actor.clone(),
            reason: reason.clone(),
            occurred_at,
        })
        .collect::<Vec<_>>();
    match CatalogStore
        .delete_releases(&state.admin.pool, &deletes)
        .await
    {
        Ok(outcome) => {
            tracing::info!(
                actor = %admin.public_api_id,
                agent = agent_label(&headers).as_deref().unwrap_or("none"),
                mode = mode.as_str(),
                releases = outcome.deleted_releases.len(),
                "admin deleted Releases"
            );
            Json(ReleaseDeleteResultDto {
                deleted_release_ids: outcome.deleted_releases,
                mode: mode.as_str().to_owned(),
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}
