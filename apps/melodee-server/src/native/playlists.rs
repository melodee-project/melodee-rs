//! Native-v1 playlist and smart-playlist compatibility routes.

use std::{
    collections::{HashMap, HashSet},
    path::Path as FilePath,
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State, multipart::MultipartRejection},
    http::{HeaderValue, StatusCode, header::LOCATION},
    response::IntoResponse,
    routing::{get, post, put},
};
use melodee_persistence::{
    NativePlaylist, NativePlaylistCreate, NativePlaylistUpdate, NativeSmartPlaylist,
    NativeSmartPlaylistCreate, NativeSmartPlaylistUpdate, NativeUserStateStore,
    NativeUserStateStoreError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use super::{
    Authenticated, NativePrincipal, NativeState,
    catalog::{album_dtos_by_public_ids, artist_dtos_by_public_ids, song_dtos_by_public_ids},
    dto::{PagedResponse, PaginationMetadata, PlaylistDto, SongDto, UserDto, duration_formatted},
    error::NativeApiError,
    images::{ImageUploadValidationError, validate_uploaded_image},
};

const DEFAULT_PAGE_SIZE: u16 = 50;
const MAX_PAGE_SIZE: u16 = 500;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_M3U_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPageParams {
    page: Option<i32>,
    page_size: Option<i32>,
    limit: Option<i32>,
    order_by: Option<String>,
    order_direction: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Page {
    number: u32,
    size: u16,
    offset: usize,
}

impl Page {
    fn parse(params: &PlaylistPageParams) -> Result<Self, NativeApiError> {
        let page = params.page.unwrap_or(1);
        let size = params
            .page_size
            .or(params.limit)
            .unwrap_or(i32::from(DEFAULT_PAGE_SIZE));
        if page < 1 || !(1..=i32::from(MAX_PAGE_SIZE)).contains(&size) {
            return Err(NativeApiError::BadRequest);
        }
        let number = u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?;
        let size = u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?;
        let offset = usize::try_from(number.saturating_sub(1))
            .ok()
            .and_then(|page| page.checked_mul(usize::from(size)))
            .ok_or(NativeApiError::BadRequest)?;
        if offset > 10_000 {
            return Err(NativeApiError::BadRequest);
        }
        Ok(Self {
            number,
            size,
            offset,
        })
    }

    fn metadata(self, total: i64) -> Result<PaginationMetadata, NativeApiError> {
        PaginationMetadata::new(total, self.number, self.size)
            .map_err(|_| NativeApiError::BadRequest)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePlaylistRequest {
    name: String,
    comment: Option<String>,
    #[serde(default)]
    is_public: bool,
    song_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlaylistRequest {
    name: Option<String>,
    comment: Option<String>,
    is_public: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongIdsRequest {
    song_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AddSongsRequest {
    Canonical(Vec<Uuid>),
    MeloAmp(SongIdsRequest),
}

impl AddSongsRequest {
    fn ids(self) -> Vec<Uuid> {
        match self {
            Self::Canonical(ids) => ids,
            Self::MeloAmp(request) => request.song_ids,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSmartPlaylistRequest {
    name: String,
    mql_query: String,
    entity_type: String,
    #[serde(default)]
    is_public: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSmartPlaylistRequest {
    name: Option<String>,
    mql_query: Option<String>,
    entity_type: Option<String>,
    is_public: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistImportResult {
    playlist_api_key: Uuid,
    total_entries: i32,
    matched_count: i32,
    missing_count: i32,
    missing_references: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartPlaylistModel {
    api_key: Uuid,
    name: String,
    mql_query: String,
    entity_type: String,
    last_result_count: i32,
    last_evaluated_at: String,
    is_public: bool,
    normalized_query: Option<String>,
    created_at: String,
    owner: UserDto,
}

#[derive(Debug, Serialize)]
pub struct SmartPlaylistEvaluateResponse {
    playlist: SmartPlaylistModel,
    meta: PaginationMetadata,
    data: Vec<Value>,
}

pub fn router() -> Router<NativeState> {
    Router::new()
        .route(
            "/api/v1/playlists",
            get(list_playlists).post(create_playlist),
        )
        .route("/api/v1/user/playlists", get(list_user_playlists_alias))
        .route(
            "/api/v1/playlists/import",
            post(import_playlist).layer(DefaultBodyLimit::max(MAX_M3U_BYTES + 64 * 1024)),
        )
        .route(
            "/api/v1/playlists/smart",
            get(list_smart_playlists).post(create_smart_playlist),
        )
        .route(
            "/api/v1/playlists/smart/{id}",
            get(get_smart_playlist)
                .put(update_smart_playlist)
                .delete(delete_smart_playlist),
        )
        .route(
            "/api/v1/playlists/smart/{id}/evaluate",
            get(evaluate_smart_playlist),
        )
        .route(
            "/api/v1/playlists/{id}",
            get(get_playlist)
                .put(update_playlist)
                .delete(delete_playlist),
        )
        .route(
            "/api/v1/playlists/{id}/songs",
            get(playlist_songs)
                .post(add_playlist_songs)
                .delete(remove_playlist_songs),
        )
        .route(
            "/api/v1/playlists/{id}/songs/reorder",
            put(reorder_playlist_songs),
        )
        .route(
            "/api/v1/playlists/{id}/image",
            post(upload_playlist_image)
                .delete(delete_playlist_image)
                .layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES + 64 * 1024)),
        )
}

pub async fn get_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<PlaylistDto>, NativeApiError> {
    let playlist = match NativeUserStateStore::playlist(&state.pool, principal.user_id, id).await {
        Ok(playlist) => playlist,
        Err(NativeUserStateStoreError::NotFound) => {
            return Err(NativeApiError::CapturedNotFound("Playlist not found"));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(Json(
        playlist_dto(&state, principal.user_id, &playlist).await?,
    ))
}

pub async fn list_playlists(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<PlaylistPageParams>,
) -> Result<Json<PagedResponse<PlaylistDto>>, NativeApiError> {
    list_playlists_inner(&state, principal, &params).await
}

pub async fn list_user_playlists_alias(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<PlaylistPageParams>,
) -> Result<Json<PagedResponse<PlaylistDto>>, NativeApiError> {
    list_playlists_inner(&state, principal, &params).await
}

async fn list_playlists_inner(
    state: &NativeState,
    principal: NativePrincipal,
    params: &PlaylistPageParams,
) -> Result<Json<PagedResponse<PlaylistDto>>, NativeApiError> {
    let page = Page::parse(params)?;
    let (total, playlists) =
        NativeUserStateStore::playlists(&state.pool, principal.user_id, page.number, page.size)
            .await?;
    let data = playlist_dtos(state, principal.user_id, &playlists).await?;
    Ok(Json(PagedResponse {
        meta: page.metadata(total)?,
        data,
    }))
}

pub async fn create_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Json(request): Json<CreatePlaylistRequest>,
) -> Result<Json<PlaylistDto>, NativeApiError> {
    let track_public_ids = request.song_ids.unwrap_or_default();
    require_accessible_tracks(&state, principal.user_id, &track_public_ids).await?;
    let playlist = NativeUserStateStore::create_playlist(
        &state.pool,
        principal.user_id,
        &NativePlaylistCreate {
            public_api_id: None,
            name: request.name,
            description: request.comment,
            is_public: request.is_public,
            track_public_ids,
        },
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(
        playlist_dto(&state, principal.user_id, &playlist).await?,
    ))
}

pub async fn update_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdatePlaylistRequest>,
) -> Result<Json<PlaylistDto>, NativeApiError> {
    require_playlist_owner(&state, principal.user_id, id).await?;
    let playlist = NativeUserStateStore::update_playlist(
        &state.pool,
        principal.user_id,
        id,
        &NativePlaylistUpdate {
            name: request.name,
            description: request.comment,
            is_public: request.is_public,
        },
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(
        playlist_dto(&state, principal.user_id, &playlist).await?,
    ))
}

pub async fn delete_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    require_playlist_owner(&state, principal.user_id, id).await?;
    NativeUserStateStore::delete_playlist(&state.pool, principal.user_id, id).await?;
    Ok(StatusCode::OK)
}

pub async fn playlist_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<Uuid>,
    Query(params): Query<PlaylistPageParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    let page = Page::parse(&params)?;
    let playlist = NativeUserStateStore::playlist(&state.pool, principal.user_id, id).await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_playlist_items item
         JOIN tracks track ON track.id=item.track_id
         JOIN releases release ON release.id=track.release_id AND release.status='active'
         WHERE item.playlist_id=$1 AND native_user_can_access_release($2,release.id)",
    )
    .bind(playlist.id)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT track.public_api_id FROM native_playlist_items item
         JOIN tracks track ON track.id=item.track_id
         JOIN releases release ON release.id=track.release_id AND release.status='active'
         WHERE item.playlist_id=$1 AND native_user_can_access_release($2,release.id)
         ORDER BY item.position,item.id LIMIT $3 OFFSET $4",
    )
    .bind(playlist.id)
    .bind(principal.user_id)
    .bind(i64::from(page.size))
    .bind(i64::try_from(page.offset).map_err(|_| NativeApiError::BadRequest)?)
    .fetch_all(&state.pool)
    .await?;
    let data = song_dtos_by_public_ids(&state, principal, &binding.0, &ids).await?;
    Ok(Json(PagedResponse {
        meta: page.metadata(total)?,
        data,
    }))
}

pub async fn add_playlist_songs(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<AddSongsRequest>,
) -> Result<StatusCode, NativeApiError> {
    let ids = request.ids();
    if ids.is_empty() {
        return Err(NativeApiError::BadRequest);
    }
    require_playlist_owner(&state, principal.user_id, id).await?;
    require_accessible_tracks(&state, principal.user_id, &ids).await?;
    NativeUserStateStore::append_playlist_tracks(
        &state.pool,
        principal.user_id,
        id,
        &ids,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::OK)
}

pub async fn remove_playlist_songs(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Json(ids): Json<Vec<Uuid>>,
) -> Result<StatusCode, NativeApiError> {
    if ids.is_empty() {
        return Err(NativeApiError::BadRequest);
    }
    require_playlist_owner(&state, principal.user_id, id).await?;
    NativeUserStateStore::remove_playlist_tracks(
        &state.pool,
        principal.user_id,
        id,
        &ids,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::OK)
}

pub async fn reorder_playlist_songs(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<SongIdsRequest>,
) -> Result<StatusCode, NativeApiError> {
    if request.song_ids.is_empty() {
        return Err(NativeApiError::BadRequest);
    }
    require_playlist_owner(&state, principal.user_id, id).await?;
    NativeUserStateStore::reorder_playlist_tracks(
        &state.pool,
        principal.user_id,
        id,
        &request.song_ids,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::OK)
}

pub async fn upload_playlist_image(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<StatusCode, NativeApiError> {
    require_playlist_owner(&state, principal.user_id, id).await?;
    let multipart = multipart.map_err(|_| NativeApiError::CapturedMissingFile)?;
    let upload = read_file_part(multipart, MAX_IMAGE_BYTES).await?;
    let mime = image_mime_from_magic(&upload.bytes).ok_or(NativeApiError::BadRequest)?;
    let bytes = validate_uploaded_image(upload.bytes)
        .await
        .map_err(|error| match error {
            ImageUploadValidationError::Invalid => NativeApiError::BadRequest,
            ImageUploadValidationError::Busy => NativeApiError::Unavailable,
        })?;
    NativeUserStateStore::set_playlist_image(
        &state.pool,
        principal.user_id,
        id,
        Some((mime, &bytes)),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::OK)
}

pub async fn delete_playlist_image(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    require_playlist_owner(&state, principal.user_id, id).await?;
    NativeUserStateStore::set_playlist_image(
        &state.pool,
        principal.user_id,
        id,
        None,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::OK)
}

pub async fn import_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<ImportParams>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Json<PlaylistImportResult>, NativeApiError> {
    let multipart = multipart.map_err(|_| NativeApiError::CapturedMissingFile)?;
    let upload = read_file_part(multipart, MAX_M3U_BYTES).await?;
    let extension_ok = upload.file_name.as_ref().is_some_and(|name| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".m3u") || lower.ends_with(".m3u8")
    });
    if !extension_ok {
        return Err(NativeApiError::BadRequest);
    }
    let references = parse_m3u(&upload.bytes)?;
    let matched = match_m3u_tracks(&state, principal.user_id, &references).await?;
    let matched_ids = matched
        .iter()
        .filter_map(|value| *value)
        .collect::<Vec<_>>();
    let missing_references = references
        .iter()
        .zip(&matched)
        .filter_map(|(reference, public_id)| public_id.is_none().then_some(reference.clone()))
        .collect::<Vec<_>>();
    let derived_name = upload
        .file_name
        .as_deref()
        .and_then(|name| FilePath::new(name).file_stem())
        .and_then(|name| name.to_str())
        .ok_or(NativeApiError::BadRequest)?;
    let playlist = NativeUserStateStore::create_playlist(
        &state.pool,
        principal.user_id,
        &NativePlaylistCreate {
            public_api_id: None,
            name: params
                .playlist_name
                .as_deref()
                .unwrap_or(derived_name)
                .to_string(),
            description: None,
            is_public: false,
            track_public_ids: matched_ids,
        },
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(PlaylistImportResult {
        playlist_api_key: playlist.public_api_id,
        total_entries: i32::try_from(references.len()).map_err(|_| NativeApiError::BadRequest)?,
        matched_count: i32::try_from(matched.iter().flatten().count())
            .map_err(|_| NativeApiError::BadRequest)?,
        missing_count: i32::try_from(missing_references.len())
            .map_err(|_| NativeApiError::BadRequest)?,
        missing_references,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportParams {
    playlist_name: Option<String>,
}

pub async fn create_smart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Json(request): Json<CreateSmartPlaylistRequest>,
) -> Result<impl IntoResponse, NativeApiError> {
    validate_mql(&request.mql_query)?;
    let smart = NativeUserStateStore::create_smart_playlist(
        &state.pool,
        principal.user_id,
        &NativeSmartPlaylistCreate {
            public_api_id: None,
            name: request.name,
            mql_query: request.mql_query,
            entity_type: request.entity_type,
            is_public: request.is_public,
        },
        None,
        OffsetDateTime::now_utc(),
    )
    .await?;
    let model = smart_playlist_model(&state, &smart).await?;
    let location =
        HeaderValue::from_str(&format!("/api/v1/playlists/smart/{}", smart.public_api_id))
            .map_err(|_| NativeApiError::Unavailable)?;
    Ok((StatusCode::CREATED, [(LOCATION, location)], Json(model)))
}

pub async fn get_smart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<SmartPlaylistModel>, NativeApiError> {
    let smart = NativeUserStateStore::smart_playlist(&state.pool, principal.user_id, id).await?;
    Ok(Json(smart_playlist_model(&state, &smart).await?))
}

pub async fn list_smart_playlists(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<PlaylistPageParams>,
) -> Result<Json<PagedResponse<SmartPlaylistModel>>, NativeApiError> {
    let (order_by, descending) = smart_order(&params)?;
    let page = Page::parse(&params)?;
    let (total, rows) = NativeUserStateStore::smart_playlists(
        &state.pool,
        principal.user_id,
        page.number,
        page.size,
        order_by,
        descending,
    )
    .await?;
    let owner = user_dto(&state, principal.user_id).await?;
    let data = rows
        .iter()
        .map(|smart| smart_playlist_model_with_owner(smart, owner.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(PagedResponse {
        meta: page.metadata(total)?,
        data,
    }))
}

pub async fn update_smart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateSmartPlaylistRequest>,
) -> Result<Json<SmartPlaylistModel>, NativeApiError> {
    if let Some(query) = request.mql_query.as_deref() {
        validate_mql(query)?;
    }
    let smart = NativeUserStateStore::update_smart_playlist(
        &state.pool,
        principal.user_id,
        id,
        &NativeSmartPlaylistUpdate {
            name: request.name,
            mql_query: request.mql_query,
            entity_type: request.entity_type,
            is_public: request.is_public,
        },
        None,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(smart_playlist_model(&state, &smart).await?))
}

pub async fn delete_smart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    NativeUserStateStore::delete_smart_playlist(&state.pool, principal.user_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn evaluate_smart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<Uuid>,
    Query(params): Query<PlaylistPageParams>,
) -> Result<Json<SmartPlaylistEvaluateResponse>, NativeApiError> {
    smart_order(&params)?;
    let page = Page::parse(&params)?;
    let mut smart =
        NativeUserStateStore::smart_playlist(&state.pool, principal.user_id, id).await?;
    let lacks_catalog_wide_access: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM native_library_group_libraries binding
           WHERE NOT native_user_can_access_library($1, binding.library_id)
         )",
    )
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    if lacks_catalog_wide_access {
        // The Phase 6 projection deliberately contains no authorization
        // fields. Refuse to expose its global hit count until evaluation can
        // be performed against a principal-scoped projection.
        return Err(NativeApiError::Forbidden);
    }
    let search = state.search.as_ref().ok_or(NativeApiError::Unavailable)?;
    if usize::from(page.size) > search.max_limit {
        return Err(NativeApiError::BadRequest);
    }
    let kind = smart_entity_kind(&smart.entity_type)?;
    let (total, hits) = melodee_search::query_active_kind(
        search,
        &state.pool,
        &smart.mql_query,
        kind,
        usize::from(page.size),
        page.offset,
    )
    .await?;
    let public_ids = hits
        .into_iter()
        .map(|hit| Uuid::parse_str(&hit.public_id).map_err(|_| NativeApiError::Unavailable))
        .collect::<Result<Vec<_>, _>>()?;
    let data = smart_result_dtos(&state, principal, &binding.0, kind, &public_ids).await?;
    let evaluated_at = OffsetDateTime::now_utc();
    if smart.owner_user_id == principal.user_id {
        NativeUserStateStore::record_smart_playlist_evaluation(
            &state.pool,
            principal.user_id,
            smart.public_api_id,
            i32::try_from(total).map_err(|_| NativeApiError::Unavailable)?,
            evaluated_at,
        )
        .await?;
        smart.last_result_count = i32::try_from(total).map_err(|_| NativeApiError::Unavailable)?;
        smart.last_evaluated_at = Some(evaluated_at);
    }
    Ok(Json(SmartPlaylistEvaluateResponse {
        playlist: smart_playlist_model(&state, &smart).await?,
        meta: page.metadata(i64::try_from(total).map_err(|_| NativeApiError::Unavailable)?)?,
        data,
    }))
}

async fn smart_result_dtos(
    state: &NativeState,
    principal: NativePrincipal,
    binding: &str,
    kind: &str,
    ids: &[Uuid],
) -> Result<Vec<Value>, NativeApiError> {
    match kind {
        "track" => song_dtos_by_public_ids(state, principal, binding, ids)
            .await?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeApiError::Unavailable),
        "release" => album_dtos_by_public_ids(state, principal, ids)
            .await?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeApiError::Unavailable),
        "artist" => artist_dtos_by_public_ids(state, principal, ids)
            .await?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeApiError::Unavailable),
        _ => Err(NativeApiError::BadRequest),
    }
}

fn validate_mql(query: &str) -> Result<(), NativeApiError> {
    melodee_mql::parse_and_plan(query)
        .map(|_| ())
        .map_err(|_| NativeApiError::BadRequest)
}

fn smart_order(params: &PlaylistPageParams) -> Result<(&'static str, bool), NativeApiError> {
    let column = match params.order_by.as_deref().map(str::to_ascii_lowercase) {
        None => "created_at",
        Some(value) if value == "createdat" => "created_at",
        Some(value) if value == "name" => "name",
        Some(value) if value == "lastresultcount" => "last_result_count",
        Some(_) => return Err(NativeApiError::BadRequest),
    };
    let descending = match params
        .order_direction
        .as_deref()
        .map(str::to_ascii_lowercase)
    {
        None => true,
        Some(value) if value == "desc" => true,
        Some(value) if value == "asc" => false,
        Some(_) => return Err(NativeApiError::BadRequest),
    };
    Ok((column, descending))
}

fn smart_entity_kind(entity_type: &str) -> Result<&'static str, NativeApiError> {
    match entity_type {
        "songs" => Ok("track"),
        "albums" => Ok("release"),
        "artists" => Ok("artist"),
        _ => Err(NativeApiError::BadRequest),
    }
}

async fn require_playlist_owner(
    state: &NativeState,
    user_id: Uuid,
    public_id: Uuid,
) -> Result<(), NativeApiError> {
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_user_id FROM native_playlists WHERE public_api_id = $1")
            .bind(public_id)
            .fetch_optional(&state.pool)
            .await?;
    match owner {
        None => Err(NativeApiError::NotFound),
        Some(owner) if owner != user_id => Err(NativeApiError::Forbidden),
        Some(_) => Ok(()),
    }
}

async fn playlist_dto(
    state: &NativeState,
    reader_user_id: Uuid,
    playlist: &NativePlaylist,
) -> Result<PlaylistDto, NativeApiError> {
    let mut dtos = playlist_dtos(state, reader_user_id, std::slice::from_ref(playlist)).await?;
    dtos.pop().ok_or(NativeApiError::Unavailable)
}

async fn playlist_dtos(
    state: &NativeState,
    reader_user_id: Uuid,
    playlists: &[NativePlaylist],
) -> Result<Vec<PlaylistDto>, NativeApiError> {
    if playlists.is_empty() {
        return Ok(Vec::new());
    }
    let ids = playlists
        .iter()
        .map(|playlist| playlist.id)
        .collect::<Vec<_>>();
    let stats = sqlx::query(
        "SELECT p.id,
                count(i.id) FILTER (
                    WHERE release.id IS NOT NULL
                      AND native_user_can_access_release($2,release.id)
                )::bigint,
                COALESCE(sum(COALESCE(t.duration_ms, media.duration_ms, 0)) FILTER (
                    WHERE release.id IS NOT NULL
                      AND native_user_can_access_release($2,release.id)
                ), 0)::bigint
         FROM unnest($1::uuid[]) requested(id)
         JOIN native_playlists p ON p.id = requested.id
         LEFT JOIN native_playlist_items i ON i.playlist_id = p.id
         LEFT JOIN tracks t ON t.id = i.track_id
         LEFT JOIN releases release ON release.id = t.release_id AND release.status = 'active'
         LEFT JOIN media_assets media ON media.id = t.playable_media_asset_id
         GROUP BY p.id",
    )
    .bind(&ids)
    .bind(reader_user_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            row.try_get::<Uuid, _>(0)?,
            (row.try_get::<i64, _>(1)?, row.try_get::<i64, _>(2)?),
        ))
    })
    .collect::<Result<HashMap<_, _>, sqlx::Error>>()?;
    let owner_ids = playlists
        .iter()
        .map(|playlist| playlist.owner_user_id)
        .collect::<HashSet<_>>();
    let mut owners = HashMap::with_capacity(owner_ids.len());
    for owner_id in owner_ids {
        owners.insert(owner_id, user_dto(state, owner_id).await?);
    }
    playlists
        .iter()
        .map(|playlist| {
            let (count, duration_ms) = stats.get(&playlist.id).copied().unwrap_or((0, 0));
            let owner = owners
                .get(&playlist.owner_user_id)
                .cloned()
                .ok_or(NativeApiError::Unavailable)?;
            Ok(PlaylistDto {
                id: playlist.public_api_id,
                thumbnail_url: image_url(state, "playlist", playlist.public_api_id, 80),
                image_url: image_url(state, "playlist", playlist.public_api_id, 1024),
                name: playlist.name.clone(),
                description: playlist.description.clone().unwrap_or_default(),
                duration_ms,
                duration_formatted: duration_formatted(duration_ms),
                song_count: i16::try_from(count).map_err(|_| NativeApiError::Unavailable)?,
                is_public: playlist.is_public,
                owner,
                created_at: wire_time(playlist.created_at)?,
                updated_at: playlist_updated_at(playlist)?,
            })
        })
        .collect()
}

async fn smart_playlist_model(
    state: &NativeState,
    smart: &NativeSmartPlaylist,
) -> Result<SmartPlaylistModel, NativeApiError> {
    let owner = user_dto(state, smart.owner_user_id).await?;
    smart_playlist_model_with_owner(smart, owner)
}

fn smart_playlist_model_with_owner(
    smart: &NativeSmartPlaylist,
    owner: UserDto,
) -> Result<SmartPlaylistModel, NativeApiError> {
    Ok(SmartPlaylistModel {
        api_key: smart.public_api_id,
        name: smart.name.clone(),
        mql_query: smart.mql_query.clone(),
        entity_type: smart.entity_type.clone(),
        last_result_count: smart.last_result_count,
        last_evaluated_at: smart
            .last_evaluated_at
            .map(wire_time)
            .transpose()?
            .unwrap_or_default(),
        is_public: smart.is_public,
        normalized_query: smart.normalized_query.clone(),
        created_at: wire_time(smart.created_at)?,
        owner,
    })
}

async fn user_dto(state: &NativeState, user_id: Uuid) -> Result<UserDto, NativeApiError> {
    let row = sqlx::query(
        "SELECT u.public_api_id, u.username, u.email, u.is_admin, u.is_editor,
                u.created_at, u.updated_at
         FROM native_users u WHERE u.id=$1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    let roles = sqlx::query_scalar::<_, String>(
        "SELECT role FROM native_user_roles WHERE user_id=$1
         ORDER BY CASE role
           WHEN 'HasSettingsRole' THEN 0
           WHEN 'HasDownloadRole' THEN 1
           WHEN 'HasUploadRole' THEN 2
           WHEN 'HasPlaylistRole' THEN 3
           WHEN 'HasCoverArtRole' THEN 4
           WHEN 'HasCommentRole' THEN 5
           WHEN 'HasPodcastRole' THEN 6
           WHEN 'HasStreamRole' THEN 7
           WHEN 'HasJukeboxRole' THEN 8
           WHEN 'HasShareRole' THEN 9
           WHEN 'Administrator' THEN 10
           ELSE 11
         END, role",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    let public_id = row.try_get(0)?;
    Ok(UserDto {
        id: public_id,
        thumbnail_url: image_url(state, "user", public_id, 80),
        image_url: image_url(state, "user", public_id, 1024),
        username: row.try_get(1)?,
        email: row.try_get(2)?,
        is_admin: row.try_get(3)?,
        is_editor: row.try_get(4)?,
        roles,
        // The legacy native serializer emits zero for every user statistic,
        // even when the migrated state tables contain activity.
        songs_played: 0,
        artists_liked: 0,
        artists_disliked: 0,
        albums_liked: 0,
        albums_disliked: 0,
        songs_liked: 0,
        songs_disliked: 0,
        created_at: wire_time(row.try_get(5)?)?,
        updated_at: wire_time(row.try_get(6)?)?,
    })
}

fn image_url(state: &NativeState, kind: &str, id: Uuid, size: u16) -> String {
    format!(
        "{}/images/{kind}_{id}/{size}",
        state.public_origin.trim_end_matches('/')
    )
}

fn wire_time(value: OffsetDateTime) -> Result<String, NativeApiError> {
    value
        .format(&Rfc3339)
        .map_err(|_| NativeApiError::Unavailable)
}

/// The migration clamps a nullable legacy LastUpdatedAt to CreatedAt to
/// satisfy the target invariant. Re-expose that marker as the legacy empty
/// wire value for unchanged imported playlists.
fn playlist_updated_at(playlist: &NativePlaylist) -> Result<String, NativeApiError> {
    if playlist.created_at == playlist.updated_at {
        Ok(String::new())
    } else {
        wire_time(playlist.updated_at)
    }
}

struct UploadedFile {
    file_name: Option<String>,
    bytes: Vec<u8>,
}

async fn read_file_part(
    mut multipart: Multipart,
    limit: usize,
) -> Result<UploadedFile, NativeApiError> {
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| NativeApiError::BadRequest)?
    {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field.file_name().map(str::to_string);
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| NativeApiError::BadRequest)?
        {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|size| size > limit)
            {
                return Err(NativeApiError::BadRequest);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(NativeApiError::BadRequest);
        }
        return Ok(UploadedFile { file_name, bytes });
    }
    Err(NativeApiError::BadRequest)
}

fn image_mime_from_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

fn parse_m3u(bytes: &[u8]) -> Result<Vec<String>, NativeApiError> {
    let text = std::str::from_utf8(bytes).map_err(|_| NativeApiError::BadRequest)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut references = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.chars().count() > 4_096 || references.len() == 10_000 {
            return Err(NativeApiError::BadRequest);
        }
        references.push(line.to_string());
    }
    if references.is_empty() {
        return Err(NativeApiError::BadRequest);
    }
    Ok(references)
}

async fn match_m3u_tracks(
    state: &NativeState,
    user_id: Uuid,
    references: &[String],
) -> Result<Vec<Option<Uuid>>, NativeApiError> {
    let file_names = references
        .iter()
        .map(|reference| normalized_m3u_file_name(reference).unwrap_or_default())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT DISTINCT ON (input.ordinal)
           input.ordinal::bigint,
           CASE WHEN release.id IS NOT NULL THEN track.public_api_id END
         FROM unnest($1::text[]) WITH ORDINALITY input(file_name, ordinal)
         LEFT JOIN native_track_file_names file_name
           ON file_name.normalized_file_name = lower(input.file_name)
         LEFT JOIN tracks track
           ON track.id = file_name.track_id
         LEFT JOIN releases release
           ON release.id = track.release_id
          AND release.status = 'active'
          AND native_user_can_access_release($2, release.id)
         ORDER BY input.ordinal,(release.id IS NOT NULL) DESC,track.public_api_id NULLS LAST",
    )
    .bind(&file_names)
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            row.try_get::<Option<Uuid>, _>(1)
                .map_err(NativeApiError::from)
        })
        .collect()
}

async fn require_accessible_tracks(
    state: &NativeState,
    user_id: Uuid,
    public_ids: &[Uuid],
) -> Result<(), NativeApiError> {
    if public_ids.is_empty() {
        return Ok(());
    }
    let distinct = public_ids.iter().copied().collect::<HashSet<_>>();
    let public_ids = distinct.iter().copied().collect::<Vec<_>>();
    let accessible = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tracks track \
         JOIN releases release ON release.id=track.release_id AND release.status='active' \
         WHERE track.public_api_id=ANY($1) \
           AND native_user_can_access_release($2,release.id)",
    )
    .bind(&public_ids)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    if accessible != i64::try_from(distinct.len()).map_err(|_| NativeApiError::BadRequest)? {
        // Unknown and inaccessible IDs intentionally share one response so the
        // playlist mutation surface cannot be used as a catalog membership oracle.
        return Err(NativeApiError::Forbidden);
    }
    Ok(())
}

fn normalized_m3u_file_name(reference: &str) -> Option<String> {
    let slash_normalized = reference.trim_matches(['"', '\'']).replace('\\', "/");
    let file_name = slash_normalized.rsplit('/').next()?;
    percent_decode(file_name)
}

fn percent_decode(value: &str) -> Option<String> {
    let input = value.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let high = hex_value(input[index + 1]);
            let low = hex_value(input[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(input[index]);
        index += 1;
    }
    String::from_utf8(output).ok()
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn add_songs_accepts_both_adr_0114_shapes_and_rejects_malformed() {
        let id = Uuid::now_v7();
        let bare: AddSongsRequest =
            serde_json::from_value(serde_json::json!([id])).expect("bare array");
        assert_eq!(bare.ids(), vec![id]);
        let object: AddSongsRequest = serde_json::from_value(serde_json::json!({
            "songIds": [id]
        }))
        .expect("MeloAmp object");
        assert_eq!(object.ids(), vec![id]);
        assert!(
            serde_json::from_value::<AddSongsRequest>(serde_json::json!({"ids": [id]})).is_err()
        );
    }

    #[test]
    fn image_validation_uses_magic_bytes_not_claimed_content_type() {
        assert_eq!(
            image_mime_from_magic(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
            Some("image/png")
        );
        assert_eq!(
            image_mime_from_magic(&[0xff, 0xd8, 0xff]),
            Some("image/jpeg")
        );
        assert_eq!(image_mime_from_magic(b"RIFF....WEBP"), Some("image/webp"));
        assert_eq!(image_mime_from_magic(b"RIFF....WAVE"), None);
        assert_eq!(image_mime_from_magic(b"<svg onload=alert(1)>"), None);
    }

    #[test]
    fn m3u_parser_is_bounded_and_ignores_comments() {
        assert_eq!(
            parse_m3u(b"#EXTM3U\n#EXTINF:1,Track\nArtist/Release/track.flac\n").expect("valid M3U"),
            vec!["Artist/Release/track.flac"]
        );
        assert!(parse_m3u(b"#EXTM3U\n").is_err());
        assert!(parse_m3u(&[0xff, 0xfe]).is_err());
        assert_eq!(
            normalized_m3u_file_name("'Artist\\Release\\Neon%20Tokyo.flac'"),
            Some("Neon Tokyo.flac".to_string())
        );
    }

    #[test]
    fn smart_mql_is_parsed_with_phase_6_resource_bounds() {
        assert!(validate_mql("artist:Bjork AND year>=2000").is_ok());
        assert!(validate_mql(&"(".repeat(4_097)).is_err());
        assert!(smart_entity_kind("songs").is_ok());
        assert!(smart_entity_kind("unknown").is_err());
    }
}
