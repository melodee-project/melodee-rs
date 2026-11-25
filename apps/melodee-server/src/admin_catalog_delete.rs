//! Administrator and editor hard deletion of catalog Artists, Releases, and
//! Tracks (ADR-0211).
//!
//! The confirmation dialog is the only operator step. A confirmed request
//! removes the selected catalog rows, their dependent Track/credit/evidence
//! rows, the originating processing work item, and every media or artwork file
//! derived from durable paths below configured roots. Deletion is available to
//! administrators and editors at any time and needs no reason, scope choice, or
//! workflow transition. One `catalog_audit_events` row records the actor and
//! the optional note so accountability survives the removed rows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use melodee_persistence::{
    ArtistHardDeleteStorage, CatalogStore, CatalogStoreError, HardDeleteArtistPreview,
    HardDeleteImpact, HardDeleteOutcome, HardDeleteReleasePreview, HardDeleteTrackPreview,
    ReleaseHardDeleteStorage, TrackHardDeleteStorage,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, authenticated_operator_user};
use crate::admin_agent::agent_label;

/// The audit note recorded when the operator leaves the optional reason blank.
pub const DEFAULT_DELETE_REASON: &str = "administrator or editor confirmed catalog deletion";

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

/// Administrator or editor identity for catalog deletion and catalog reads.
#[derive(Debug, Clone, Copy)]
pub struct CatalogOperator {
    pub public_api_id: Uuid,
}

impl axum::extract::FromRequestParts<AdminArtistDeleteState> for CatalogOperator {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AdminArtistDeleteState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticated_operator_user(parts, &state.admin).await?;
        if !user.is_admin && !user.is_editor {
            return Err(delete_error(
                StatusCode::FORBIDDEN,
                "forbidden",
                "catalog deletion requires an administrator or editor",
            ));
        }
        Ok(Self {
            public_api_id: user.public_api_id,
        })
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
        .route(
            "/api/admin/v1/catalog/tracks/delete/preview",
            post(preview_track_delete),
        )
        .route("/api/admin/v1/catalog/tracks/delete", post(delete_tracks))
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
            "select between 1 and 100 distinct entries of one entity type",
        ),
        CatalogStoreError::MutationConflict => delete_error(
            StatusCode::CONFLICT,
            "not_found",
            "a selected entry no longer exists; refresh and retry",
        ),
        other => {
            tracing::error!(%other, "catalog hard-deletion store failure");
            delete_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                "deletion failed",
            )
        }
    }
}

/// Failure returned by the shared hard-deletion executor.
#[derive(Debug)]
pub struct HardDeleteApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl HardDeleteApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn into_response(self) -> Response {
        delete_error(self.status, self.code, self.message)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtistSelection {
    artist_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSelection {
    release_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackSelection {
    track_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtistDeleteRequest {
    artist_ids: Vec<Uuid>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDeleteRequest {
    release_ids: Vec<Uuid>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackDeleteRequest {
    track_ids: Vec<Uuid>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImpactDto {
    artists: i64,
    releases: i64,
    tracks: i64,
    media_files: i64,
    media_bytes: i64,
    retained_shared_releases: i64,
    release_credits: i64,
    track_credits: i64,
    contributions: i64,
    user_states: i64,
    images: i64,
}

impl From<HardDeleteImpact> for ImpactDto {
    fn from(impact: HardDeleteImpact) -> Self {
        Self {
            artists: impact.artists,
            releases: impact.releases,
            tracks: impact.tracks,
            media_files: impact.media_files,
            media_bytes: impact.media_bytes,
            retained_shared_releases: impact.retained_shared_releases,
            release_credits: impact.release_credits,
            track_credits: impact.track_credits,
            contributions: impact.contributions,
            user_states: impact.user_states,
            images: impact.images,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistPreviewArtistDto {
    id: Uuid,
    canonical_name: String,
    status: String,
    owned_releases: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistPreviewDto {
    artists: Vec<ArtistPreviewArtistDto>,
    impact: ImpactDto,
    storage_configured: bool,
    warnings: Vec<String>,
}

impl From<HardDeleteArtistPreview> for ArtistPreviewDto {
    fn from(preview: HardDeleteArtistPreview) -> Self {
        Self {
            artists: preview
                .artists
                .into_iter()
                .map(|artist| ArtistPreviewArtistDto {
                    id: artist.public_api_id,
                    canonical_name: artist.canonical_name,
                    status: artist.status,
                    owned_releases: artist.owned_releases,
                })
                .collect(),
            impact: preview.impact.into(),
            storage_configured: false,
            warnings: preview.warnings,
        }
    }
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
    storage_configured: bool,
    warnings: Vec<String>,
}

impl From<HardDeleteReleasePreview> for ReleasePreviewDto {
    fn from(preview: HardDeleteReleasePreview) -> Self {
        Self {
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
            impact: preview.impact.into(),
            storage_configured: false,
            warnings: preview.warnings,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackPreviewTrackDto {
    id: Uuid,
    title: String,
    release_id: Uuid,
    release_title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackPreviewDto {
    tracks: Vec<TrackPreviewTrackDto>,
    impact: ImpactDto,
    storage_configured: bool,
    warnings: Vec<String>,
}

impl From<HardDeleteTrackPreview> for TrackPreviewDto {
    fn from(preview: HardDeleteTrackPreview) -> Self {
        Self {
            tracks: preview
                .tracks
                .into_iter()
                .map(|track| TrackPreviewTrackDto {
                    id: track.public_api_id,
                    title: track.title,
                    release_id: track.release_public_id,
                    release_title: track.release_title,
                })
                .collect(),
            impact: preview.impact.into(),
            storage_configured: false,
            warnings: preview.warnings,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResultDto {
    deleted_artist_ids: Vec<Uuid>,
    deleted_release_ids: Vec<Uuid>,
    deleted_track_ids: Vec<Uuid>,
}

impl From<HardDeleteOutcome> for DeleteResultDto {
    fn from(outcome: HardDeleteOutcome) -> Self {
        Self {
            deleted_artist_ids: outcome.deleted_artists,
            deleted_release_ids: outcome.deleted_releases,
            deleted_track_ids: outcome.deleted_tracks,
        }
    }
}

async fn preview_artist_delete(
    State(state): State<AdminArtistDeleteState>,
    _operator: CatalogOperator,
    Json(selection): Json<ArtistSelection>,
) -> Response {
    let mut preview = match CatalogStore
        .preview_hard_delete_artists(&state.admin.pool, &selection.artist_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    preview.impact.artists = i64::try_from(selection.artist_ids.len()).unwrap_or(i64::MAX);
    let mut dto = ArtistPreviewDto::from(preview);
    dto.storage_configured = state.canonical_root.is_some();
    Json(dto).into_response()
}

async fn preview_release_delete(
    State(state): State<AdminArtistDeleteState>,
    _operator: CatalogOperator,
    Json(selection): Json<ReleaseSelection>,
) -> Response {
    let mut preview = match CatalogStore
        .preview_hard_delete_releases(&state.admin.pool, &selection.release_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    preview.impact.releases = i64::try_from(selection.release_ids.len()).unwrap_or(i64::MAX);
    let mut dto = ReleasePreviewDto::from(preview);
    dto.storage_configured = state.canonical_root.is_some();
    Json(dto).into_response()
}

async fn preview_track_delete(
    State(state): State<AdminArtistDeleteState>,
    _operator: CatalogOperator,
    Json(selection): Json<TrackSelection>,
) -> Response {
    let mut preview = match CatalogStore
        .preview_hard_delete_tracks(&state.admin.pool, &selection.track_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return catalog_error(error),
    };
    preview.impact.tracks = i64::try_from(selection.track_ids.len()).unwrap_or(i64::MAX);
    let mut dto = TrackPreviewDto::from(preview);
    dto.storage_configured = state.canonical_root.is_some();
    Json(dto).into_response()
}

fn normalized_reason(reason: Option<&str>) -> String {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DELETE_REASON)
        .to_owned()
}

async fn delete_artists(
    State(state): State<AdminArtistDeleteState>,
    operator: CatalogOperator,
    headers: axum::http::HeaderMap,
    Json(request): Json<ArtistDeleteRequest>,
) -> Response {
    let reason = normalized_reason(request.reason.as_deref());
    let targets = match CatalogStore
        .artist_hard_delete_storage(&state.admin.pool, &request.artist_ids)
        .await
    {
        Ok(targets) => targets,
        Err(error) => return catalog_error(error),
    };
    if targets.len() != request.artist_ids.len() {
        return delete_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "one or more Artists were not found",
        );
    }
    if let Err(error) = remove_artist_files(&state, &targets).await {
        return error.into_response();
    }
    let outcome = match CatalogStore
        .hard_delete_artists(
            &state.admin.pool,
            &request.artist_ids,
            &operator.public_api_id.to_string(),
            &reason,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return catalog_error(error),
    };
    tracing::info!(
        actor = %operator.public_api_id,
        agent = agent_label(&headers).as_deref().unwrap_or("none"),
        artists = outcome.deleted_artists.len(),
        releases = outcome.deleted_releases.len(),
        "admin or editor hard-deleted Artists"
    );
    cleanup_asset_keys(&outcome).await;
    Json(DeleteResultDto::from(outcome)).into_response()
}

async fn delete_releases(
    State(state): State<AdminArtistDeleteState>,
    operator: CatalogOperator,
    headers: axum::http::HeaderMap,
    Json(request): Json<ReleaseDeleteRequest>,
) -> Response {
    let reason = normalized_reason(request.reason.as_deref());
    match execute_release_hard_delete(
        &state.admin.pool,
        state.canonical_root.as_deref(),
        state.staging_root.as_deref(),
        &request.release_ids,
        &operator.public_api_id.to_string(),
        &reason,
    )
    .await
    {
        Ok(outcome) => {
            tracing::info!(
                actor = %operator.public_api_id,
                agent = agent_label(&headers).as_deref().unwrap_or("none"),
                releases = outcome.deleted_releases.len(),
                "admin or editor hard-deleted Releases"
            );
            Json(DeleteResultDto::from(outcome)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn delete_tracks(
    State(state): State<AdminArtistDeleteState>,
    operator: CatalogOperator,
    headers: axum::http::HeaderMap,
    Json(request): Json<TrackDeleteRequest>,
) -> Response {
    let reason = normalized_reason(request.reason.as_deref());
    let targets = match CatalogStore
        .track_hard_delete_storage(&state.admin.pool, &request.track_ids)
        .await
    {
        Ok(targets) => targets,
        Err(error) => return catalog_error(error),
    };
    if targets.len() != request.track_ids.len() {
        return delete_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "one or more Tracks were not found",
        );
    }
    if let Err(error) = remove_track_files(&state, &targets).await {
        return error.into_response();
    }
    let outcome = match CatalogStore
        .hard_delete_tracks(
            &state.admin.pool,
            &request.track_ids,
            &operator.public_api_id.to_string(),
            &reason,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return catalog_error(error),
    };
    tracing::info!(
        actor = %operator.public_api_id,
        agent = agent_label(&headers).as_deref().unwrap_or("none"),
        tracks = outcome.deleted_tracks.len(),
        "admin or editor hard-deleted Tracks"
    );
    cleanup_asset_keys(&outcome).await;
    Json(DeleteResultDto::from(outcome)).into_response()
}

/// Shared Release hard-deletion executor used by the catalog list/detail
/// surface and the Processing Queue bulk action.
pub async fn execute_release_hard_delete(
    pool: &sqlx::PgPool,
    canonical_root: Option<&Path>,
    staging_root: Option<&Path>,
    release_public_ids: &[Uuid],
    actor: &str,
    reason: &str,
) -> Result<HardDeleteOutcome, HardDeleteApiError> {
    let targets = CatalogStore
        .release_hard_delete_storage(pool, release_public_ids)
        .await
        .map_err(|error| match error {
            CatalogStoreError::InvalidRequest => HardDeleteApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "select between 1 and 100 distinct Releases",
            ),
            other => {
                tracing::error!(%other, "Release hard-deletion storage lookup failed");
                HardDeleteApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "store_error",
                    "Release deletion failed",
                )
            }
        })?;
    if targets.len() != release_public_ids.len() {
        return Err(HardDeleteApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "one or more Releases were not found",
        ));
    }
    remove_release_files(canonical_root, staging_root, &targets).await?;
    let outcome = CatalogStore
        .hard_delete_releases(
            pool,
            release_public_ids,
            actor,
            reason,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|error| match error {
            CatalogStoreError::MutationConflict => HardDeleteApiError::new(
                StatusCode::CONFLICT,
                "not_found",
                "a selected Release no longer exists; refresh and retry",
            ),
            other => {
                tracing::error!(%other, "Release hard-deletion commit failed");
                HardDeleteApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "store_error",
                    "Release deletion failed",
                )
            }
        })?;
    cleanup_asset_keys(&outcome).await;
    Ok(outcome)
}

async fn remove_release_files(
    canonical_root: Option<&Path>,
    staging_root: Option<&Path>,
    targets: &[ReleaseHardDeleteStorage],
) -> Result<(), HardDeleteApiError> {
    for target in dedupe_release_targets(targets) {
        if let (Some(canonical_relative_path), Some(root)) =
            (target.canonical_relative_path.as_deref(), canonical_root)
        {
            if let Err(error) = melodee_ingestion::delete_release_storage(
                root,
                canonical_relative_path,
                staging_root,
                target.staging_relative_path.as_deref(),
            )
            .await
            {
                tracing::error!(
                    release = %target.public_api_id,
                    %error,
                    "catalog hard deletion could not remove Release storage; catalog unchanged"
                );
                return Err(HardDeleteApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete_execution_failed",
                    "Release storage removal failed; no catalog change was made",
                ));
            }
        } else if !target.media_relative_paths.is_empty() {
            let Some(root) = canonical_root else {
                return Err(HardDeleteApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "delete_storage_unavailable",
                    "canonical storage is not configured; nothing was changed",
                ));
            };
            if let Err(error) =
                melodee_ingestion::delete_storage_files(root, &target.media_relative_paths).await
            {
                tracing::error!(
                    release = %target.public_api_id,
                    %error,
                    "catalog hard deletion could not remove Release media; catalog unchanged"
                );
                return Err(HardDeleteApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete_execution_failed",
                    "Release media removal failed; no catalog change was made",
                ));
            }
        }
    }
    Ok(())
}

async fn remove_track_files(
    state: &AdminArtistDeleteState,
    targets: &[TrackHardDeleteStorage],
) -> Result<(), HardDeleteApiError> {
    let files = targets
        .iter()
        .flat_map(|target| target.media_relative_paths.iter().cloned())
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(());
    }
    let Some(root) = state.canonical_root.as_deref() else {
        return Err(HardDeleteApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "delete_storage_unavailable",
            "canonical storage is not configured; nothing was changed",
        ));
    };
    melodee_ingestion::delete_storage_files(root, &files)
        .await
        .map_err(|error| {
            tracing::error!(%error, "catalog hard deletion could not remove Track media");
            HardDeleteApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_execution_failed",
                "Track media removal failed; no catalog change was made",
            )
        })?;
    Ok(())
}

async fn remove_artist_files(
    state: &AdminArtistDeleteState,
    targets: &[ArtistHardDeleteStorage],
) -> Result<(), HardDeleteApiError> {
    let releases = targets
        .iter()
        .flat_map(|target| target.releases.iter().cloned())
        .collect::<Vec<_>>();
    remove_release_files(
        state.canonical_root.as_deref(),
        state.staging_root.as_deref(),
        &releases,
    )
    .await
}

fn dedupe_release_targets(targets: &[ReleaseHardDeleteStorage]) -> Vec<&ReleaseHardDeleteStorage> {
    let mut seen = BTreeSet::new();
    targets
        .iter()
        .filter(|target| seen.insert(target.public_api_id))
        .collect()
}

async fn cleanup_asset_keys(outcome: &HardDeleteOutcome) {
    for key in &outcome.released_asset_keys {
        if let Err(error) = melodee_persistence::asset_files::remove(key.clone()).await {
            tracing::error!(asset_key = %key, %error, "deleted catalog artwork file could not be removed");
        }
    }
}
