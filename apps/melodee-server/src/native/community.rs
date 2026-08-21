//! Native-v1 requests and public shares.
//!
//! These adapters keep migrated public UUIDs at the wire boundary while all
//! authorization and joins use immutable internal user identities.

use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, FromRow, PgConnection};
use std::{future::Future, net::SocketAddr};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use melodee_persistence::{AuthStore, RateLimitOutcome, RateLimitPolicy};

use super::{
    Authenticated, NativePrincipal, NativeState,
    dto::{PagedResponse, PaginationMetadata, UserDto},
    error::NativeApiError,
};

const MAX_PAGE_SIZE: i32 = 500;
const SHARE_TOKEN_BYTES: usize = 16;
const MAX_SHARE_TOKEN_ATTEMPTS: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestListParams {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
    pub query: Option<String>,
    pub mine: Option<bool>,
    pub status: Option<i16>,
    pub artist_api_key: Option<Uuid>,
    pub album_api_key: Option<Uuid>,
    pub song_api_key: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageParams {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSummary {
    pub api_key: Uuid,
    pub user_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    pub api_key: Uuid,
    pub category: &'static str,
    pub status: &'static str,
    pub description: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub song_title: Option<String>,
    pub release_year: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: String,
    pub last_activity_type: String,
    pub comment_count: i32,
    pub created_by_user: UserSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDetail {
    pub api_key: Uuid,
    pub category: &'static str,
    pub status: &'static str,
    pub description: String,
    pub artist_name: Option<String>,
    pub target_artist_api_key: Option<Uuid>,
    pub album_title: Option<String>,
    pub target_album_api_key: Option<Uuid>,
    pub song_title: Option<String>,
    pub target_song_api_key: Option<Uuid>,
    pub release_year: Option<i32>,
    pub external_url: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: String,
    pub last_activity_type: String,
    pub comment_count: i32,
    pub created_by_user: UserSummary,
    pub last_activity_user: Option<UserSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub category: i16,
    pub description: String,
    pub artist_name: Option<String>,
    pub target_artist_api_key: Option<Uuid>,
    pub album_title: Option<String>,
    pub target_album_api_key: Option<Uuid>,
    pub song_title: Option<String>,
    pub target_song_api_key: Option<Uuid>,
    pub release_year: Option<i32>,
    pub external_url: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRequest {
    pub description: Option<String>,
    pub artist_name: Option<String>,
    pub target_artist_api_key: Option<Uuid>,
    pub album_title: Option<String>,
    pub target_album_api_key: Option<Uuid>,
    pub song_title: Option<String>,
    pub target_song_api_key: Option<Uuid>,
    pub release_year: Option<i32>,
    pub external_url: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, FromRow)]
struct RequestRow {
    internal_id: Uuid,
    public_api_id: Uuid,
    created_by_user_id: Uuid,
    category: i16,
    status: i16,
    description: String,
    artist_name: Option<String>,
    target_artist_public_api_id: Option<Uuid>,
    release_title: Option<String>,
    target_release_public_api_id: Option<Uuid>,
    track_title: Option<String>,
    target_track_public_api_id: Option<Uuid>,
    release_year: Option<i32>,
    external_url: Option<String>,
    notes: Option<String>,
    last_activity_at: OffsetDateTime,
    last_activity_type: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    creator_public_id: Uuid,
    creator_name: String,
    activity_user_public_id: Option<Uuid>,
    activity_user_name: Option<String>,
    comment_count: i64,
    total_count: i64,
}

const REQUEST_SELECT: &str = "SELECT r.id AS internal_id, r.public_api_id, r.created_by_user_id, r.category, r.status, \
     r.description, r.artist_name, r.target_artist_public_api_id, r.release_title, \
     r.target_release_public_api_id, r.track_title, r.target_track_public_api_id, \
     r.release_year, r.external_url, r.notes, r.last_activity_at, r.last_activity_type, \
     r.created_at, r.updated_at, creator.public_api_id AS creator_public_id, \
     creator.username AS creator_name, actor.public_api_id AS activity_user_public_id, \
     actor.username AS activity_user_name, \
     (SELECT count(*) FROM native_request_comments c WHERE c.request_id = r.id) AS comment_count, \
     count(*) OVER() AS total_count \
     FROM native_requests r JOIN native_users creator ON creator.id = r.created_by_user_id \
     LEFT JOIN native_users actor ON actor.id = r.last_activity_user_id";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateComment {
    pub body: String,
    pub parent_comment_api_key: Option<Uuid>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RequestComment {
    pub api_key: Uuid,
    pub parent_comment_api_key: Option<Uuid>,
    pub body: String,
    pub is_system: bool,
    pub created_at: String,
    pub created_by_user: Option<UserSummary>,
}

#[derive(Debug, FromRow)]
struct CommentRow {
    api_key: Uuid,
    parent_comment_api_key: Option<Uuid>,
    body: String,
    is_system: bool,
    created_at: OffsetDateTime,
    creator_public_id: Option<Uuid>,
    creator_name: Option<String>,
    total_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCheck {
    pub has_unread: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadRequestSummary {
    pub api_key: Uuid,
    pub category: &'static str,
    pub status: &'static str,
    pub description: String,
    pub last_activity_at: String,
    pub last_activity_type: String,
    pub last_activity_user: Option<UserSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShare {
    pub share_type: i16,
    pub resource_id: Uuid,
    pub description: Option<String>,
    #[serde(default)]
    pub is_downloadable: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateShare {
    pub description: Option<String>,
    // Native-v1 accepts this legacy request property, but the captured update
    // route does not apply it. Keep it deserializable without changing the
    // durable share policy behind the compatibility surface.
    #[serde(rename = "isDownloadable")]
    _is_downloadable: Option<bool>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDto {
    pub id: Uuid,
    pub share_url: String,
    pub share_type: &'static str,
    pub resource_id: Uuid,
    pub resource_name: String,
    pub resource_thumbnail_url: String,
    pub resource_image_url: String,
    pub description: Option<String>,
    pub is_downloadable: bool,
    pub visit_count: i32,
    pub owner: UserDto,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_visited_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct ShareRow {
    public_api_id: Uuid,
    share_unique_id: String,
    owner_user_id: Uuid,
    share_type: i16,
    resource_public_api_id: Uuid,
    description: Option<String>,
    is_downloadable: bool,
    visit_count: i32,
    expires_at: Option<OffsetDateTime>,
    last_visited_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    total_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicShareResponse {
    pub share_type: &'static str,
    pub resource_name: String,
    pub description: Option<String>,
    pub thumbnail_url: String,
    pub image_url: String,
    pub is_downloadable: bool,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub artist: Option<PublicArtistInfo>,
    pub album: Option<PublicReleaseInfo>,
    pub song: Option<PublicTrackInfo>,
    pub playlist: Option<PublicPlaylistInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicArtistInfo {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicReleaseInfo {
    pub id: Uuid,
    pub name: String,
    pub artist_name: String,
    pub release_year: i32,
    pub songs: Vec<PublicTrackInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicTrackInfo {
    pub id: Uuid,
    pub title: String,
    pub track_number: i32,
    pub duration_ms: f64,
    pub stream_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicPlaylistInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub songs: Vec<PublicTrackInfo>,
}

pub fn protected_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/requests", get(list_requests).post(create_request))
        .route("/api/v1/requests/activity", get(request_activity))
        .route("/api/v1/requests/activity/unread", get(unread_requests))
        .route(
            "/api/v1/requests/{id}",
            get(get_request).put(update_request).delete(delete_request),
        )
        .route("/api/v1/requests/{id}/complete", post(complete_request))
        .route(
            "/api/v1/requests/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/v1/requests/{id}/seen", post(mark_request_seen))
        .route("/api/v1/shares", get(list_shares).post(create_share))
        .route(
            "/api/v1/shares/{id}",
            get(get_share).put(update_share).delete(delete_share),
        )
}

pub fn public_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/shares/public/{unique_id}", get(public_share))
        // The captured DTO advertises this public URL. Phase 7 serves the same
        // bounded JSON representation here so clients can follow it verbatim
        // without depending on the later administrative UI phase.
        .route("/share/{unique_id}", get(public_share))
}

async fn list_requests(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<RequestListParams>,
) -> Result<Json<PagedResponse<RequestSummary>>, NativeApiError> {
    let (page, size, offset) = page(params.page, params.page_size, 20)?;
    if params.status.is_some_and(|value| !(1..=4).contains(&value)) {
        return Err(NativeApiError::BadRequest);
    }
    let sql = format!(
        "{REQUEST_SELECT} WHERE ($2::bool IS DISTINCT FROM true OR r.created_by_user_id = $1) \
         AND ($3::smallint IS NULL OR r.status = $3) \
         AND ($4::uuid IS NULL OR r.target_artist_public_api_id = $4) \
         AND ($5::uuid IS NULL OR r.target_release_public_api_id = $5) \
         AND ($6::uuid IS NULL OR r.target_track_public_api_id = $6) \
         AND ($7::text IS NULL OR r.description ILIKE ('%' || $7 || '%') \
              OR r.artist_name ILIKE ('%' || $7 || '%') \
              OR r.release_title ILIKE ('%' || $7 || '%') \
              OR r.track_title ILIKE ('%' || $7 || '%')) \
         ORDER BY r.updated_at DESC, r.public_api_id LIMIT $8 OFFSET $9"
    );
    let rows = sqlx::query_as::<_, RequestRow>(AssertSqlSafe(sql))
        .bind(principal.user_id)
        .bind(params.mine)
        .bind(params.status)
        .bind(params.artist_api_key)
        .bind(params.album_api_key)
        .bind(params.song_api_key)
        .bind(
            params
                .query
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty()),
        )
        .bind(i64::from(size))
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
    let total = rows.first().map_or(0, |row| row.total_count);
    let data = rows
        .into_iter()
        .map(request_summary)
        .collect::<Result<_, _>>()?;
    Ok(Json(PagedResponse {
        meta: metadata(total, page, size)?,
        data,
    }))
}

async fn get_request(
    State(state): State<NativeState>,
    _auth: Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<RequestDetail>, NativeApiError> {
    let row = load_request(&state, id).await?;
    Ok(Json(request_detail(row)?))
}

async fn create_request(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<RequestDetail>), NativeApiError> {
    validate_request(request.category, &request.description)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_requests (id, public_api_id, created_by_user_id, category, \
         description, artist_name, target_artist_public_api_id, release_title, \
         target_release_public_api_id, track_title, target_track_public_api_id, release_year, \
         external_url, notes, last_activity_at, last_activity_user_id, created_at, updated_at) \
         VALUES ($1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now(), $2, now(), now())",
    )
    .bind(id)
    .bind(principal.user_id)
    .bind(request.category)
    .bind(request.description.trim())
    .bind(request.artist_name)
    .bind(request.target_artist_api_key)
    .bind(request.album_title)
    .bind(request.target_album_api_key)
    .bind(request.song_title)
    .bind(request.target_song_api_key)
    .bind(request.release_year)
    .bind(request.external_url)
    .bind(request.notes)
    .execute(&state.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(request_detail(load_request(&state, id).await?)?),
    ))
}

async fn update_request(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<RequestDetail>, NativeApiError> {
    if request
        .description
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(NativeApiError::BadRequest);
    }
    let result = sqlx::query(
        "UPDATE native_requests SET description = COALESCE($3, description), \
         artist_name = COALESCE($4, artist_name), \
         target_artist_public_api_id = COALESCE($5, target_artist_public_api_id), \
         release_title = COALESCE($6, release_title), \
         target_release_public_api_id = COALESCE($7, target_release_public_api_id), \
         track_title = COALESCE($8, track_title), \
         target_track_public_api_id = COALESCE($9, target_track_public_api_id), \
         release_year = COALESCE($10, release_year), external_url = COALESCE($11, external_url), \
         notes = COALESCE($12, notes), last_activity_at = now(), last_activity_type = 'Edited', \
         last_activity_user_id = $1, updated_at = now() \
         WHERE public_api_id = $2 AND created_by_user_id = $1",
    )
    .bind(principal.user_id)
    .bind(id)
    .bind(request.description)
    .bind(request.artist_name)
    .bind(request.target_artist_api_key)
    .bind(request.album_title)
    .bind(request.target_album_api_key)
    .bind(request.song_title)
    .bind(request.target_song_api_key)
    .bind(request.release_year)
    .bind(request.external_url)
    .bind(request.notes)
    .execute(&state.pool)
    .await?;
    owner_mutation_result(&state, id, result.rows_affected()).await?;
    Ok(Json(request_detail(load_request(&state, id).await?)?))
}

async fn complete_request(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let result = sqlx::query(
        "UPDATE native_requests SET status = 3, last_activity_at = now(), \
         last_activity_type = 'StatusChanged', last_activity_user_id = $1, updated_at = now() \
         WHERE public_api_id = $2 AND created_by_user_id = $1",
    )
    .bind(principal.user_id)
    .bind(id)
    .execute(&state.pool)
    .await?;
    owner_mutation_result(&state, id, result.rows_affected()).await?;
    // WP-09: notify the request creator that the request is fulfilled.
    // Fire-and-record: a delivery failure never fails the completion
    // (PRD-NOTIF-002).
    let detail = sqlx::query_as::<_, (String, String)>(
        "SELECT u.email, r.description FROM native_requests r \
         JOIN native_users u ON u.id = r.created_by_user_id WHERE r.public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    if let Ok(Some((email, description))) = detail {
        state.notifications.dispatch(
            "request.fulfilled",
            email,
            vec![("description".to_string(), description)],
        );
    }
    Ok(StatusCode::OK)
}

/// Email lookup for notification wiring; a missing row or lookup failure
/// quietly skips the notification (delivery is fail-degraded, PRD-NOTIF-002).
async fn user_email_by_id(pool: &sqlx::PgPool, user_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT email FROM native_users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

async fn delete_request(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let result = sqlx::query(
        "DELETE FROM native_requests WHERE public_api_id = $2 AND created_by_user_id = $1 AND status = 1",
    )
    .bind(principal.user_id)
    .bind(id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        let owner_status = sqlx::query_as::<_, (Uuid, i16)>(
            "SELECT created_by_user_id, status FROM native_requests WHERE public_api_id = $1",
        )
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
        return match owner_status {
            None => Err(NativeApiError::NotFound),
            Some((owner, _)) if owner != principal.user_id => Err(NativeApiError::Forbidden),
            Some(_) => Err(NativeApiError::BadRequest),
        };
    }
    Ok(StatusCode::OK)
}

async fn owner_mutation_result(
    state: &NativeState,
    id: Uuid,
    changed: u64,
) -> Result<(), NativeApiError> {
    if changed > 0 {
        return Ok(());
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM native_requests WHERE public_api_id = $1)",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?
    {
        Err(NativeApiError::Forbidden)
    } else {
        Err(NativeApiError::NotFound)
    }
}

async fn list_comments(
    State(state): State<NativeState>,
    _auth: Authenticated,
    Path(id): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Json<PagedResponse<RequestComment>>, NativeApiError> {
    let request_id = request_internal_id(&state, id).await?;
    let (page, size, offset) = page(params.page, params.page_size, 50)?;
    let rows = sqlx::query_as::<_, CommentRow>(
        "SELECT c.public_api_id AS api_key, parent.public_api_id AS parent_comment_api_key, \
         c.body, c.is_system, c.created_at, u.public_api_id AS creator_public_id, \
         u.username AS creator_name, count(*) OVER() AS total_count \
         FROM native_request_comments c \
         LEFT JOIN native_request_comments parent ON parent.id = c.parent_comment_id \
         LEFT JOIN native_users u ON u.id = c.created_by_user_id \
         WHERE c.request_id = $1 ORDER BY c.created_at, c.public_api_id LIMIT $2 OFFSET $3",
    )
    .bind(request_id)
    .bind(i64::from(size))
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let total = rows.first().map_or(0, |row| row.total_count);
    let data = rows.into_iter().map(comment_dto).collect();
    Ok(Json(PagedResponse {
        meta: metadata(total, page, size)?,
        data,
    }))
}

async fn create_comment(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateComment>,
) -> Result<(StatusCode, Json<RequestComment>), NativeApiError> {
    if request.body.trim().is_empty() || request.body.chars().count() > 10_000 {
        return Err(NativeApiError::BadRequest);
    }
    let request_id = request_internal_id(&state, id).await?;
    let parent_id = match request.parent_comment_api_key {
        Some(parent) => sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM native_request_comments WHERE request_id = $1 AND public_api_id = $2",
        )
        .bind(request_id)
        .bind(parent)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(NativeApiError::BadRequest)
        .map(Some)?,
        None => None,
    };
    let comment_id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO native_request_comments (id, public_api_id, request_id, parent_comment_id, \
         created_by_user_id, body, created_at) VALUES ($1, $1, $2, $3, $4, $5, now())",
    )
    .bind(comment_id)
    .bind(request_id)
    .bind(parent_id)
    .bind(principal.user_id)
    .bind(request.body.trim())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE native_requests SET last_activity_at = now(), last_activity_type = 'UserComment', \
         last_activity_user_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(request_id)
    .bind(principal.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let row = load_comment(&state, comment_id).await?;
    Ok((StatusCode::CREATED, Json(comment_dto(row))))
}

async fn request_activity(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Json<ActivityCheck>, NativeApiError> {
    let has_unread = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM native_requests r \
         LEFT JOIN native_request_seen s ON s.request_id = r.id AND s.user_id = $1 \
         WHERE (r.created_by_user_id = $1 OR EXISTS(SELECT 1 FROM native_request_comments c \
                WHERE c.request_id = r.id AND c.created_by_user_id = $1)) \
         AND r.last_activity_user_id IS DISTINCT FROM $1 \
         AND (s.seen_at IS NULL OR s.seen_at < r.last_activity_at))",
    )
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ActivityCheck { has_unread }))
}

async fn unread_requests(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<PageParams>,
) -> Result<Json<PagedResponse<UnreadRequestSummary>>, NativeApiError> {
    let (page, size, offset) = page(params.page, params.page_size, 10)?;
    let sql = format!(
        "{REQUEST_SELECT} LEFT JOIN native_request_seen seen \
         ON seen.request_id = r.id AND seen.user_id = $1 \
         WHERE (r.created_by_user_id = $1 OR EXISTS(SELECT 1 FROM native_request_comments c \
                WHERE c.request_id = r.id AND c.created_by_user_id = $1)) \
         AND r.last_activity_user_id IS DISTINCT FROM $1 \
         AND (seen.seen_at IS NULL OR seen.seen_at < r.last_activity_at) \
         ORDER BY r.last_activity_at DESC, r.public_api_id LIMIT $2 OFFSET $3"
    );
    let rows = sqlx::query_as::<_, RequestRow>(AssertSqlSafe(sql))
        .bind(principal.user_id)
        .bind(i64::from(size))
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
    let total = rows.first().map_or(0, |row| row.total_count);
    let data = rows
        .into_iter()
        .map(unread_summary)
        .collect::<Result<_, _>>()?;
    Ok(Json(PagedResponse {
        meta: metadata(total, page, size)?,
        data,
    }))
}

async fn mark_request_seen(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let row = load_request(&state, id).await?;
    let participant = row.created_by_user_id == principal.user_id
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_request_comments WHERE request_id = $1 AND created_by_user_id = $2)",
        )
        .bind(row.internal_id)
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?;
    if !participant {
        return Err(NativeApiError::Forbidden);
    }
    sqlx::query(
        "INSERT INTO native_request_seen (request_id, user_id, seen_at) VALUES ($1, $2, now()) \
         ON CONFLICT (request_id, user_id) DO UPDATE SET seen_at = EXCLUDED.seen_at",
    )
    .bind(row.internal_id)
    .bind(principal.user_id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::OK)
}

async fn list_shares(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<PageParams>,
) -> Result<Json<PagedResponse<ShareDto>>, NativeApiError> {
    let (page, size, offset) = page(params.page, params.page_size, 20)?;
    let rows = sqlx::query_as::<_, ShareRow>(
        "SELECT public_api_id, share_unique_id, owner_user_id, share_type, resource_public_api_id, \
         description, is_downloadable, visit_count, expires_at, last_visited_at, created_at, \
         count(*) OVER() AS total_count FROM native_shares WHERE owner_user_id = $1 \
         -- The legacy share index is oldest-first; preserve that ordering so
         -- a migrated pre-existing share stays ahead of a newly-created one.
         ORDER BY created_at ASC, public_api_id LIMIT $2 OFFSET $3",
    )
    .bind(principal.user_id)
    .bind(i64::from(size))
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let total = rows.first().map_or(0, |row| row.total_count);
    let mut data = Vec::with_capacity(rows.len());
    for row in rows {
        data.push(share_dto(&state, row).await?);
    }
    Ok(Json(PagedResponse {
        meta: metadata(total, page, size)?,
        data,
    }))
}

async fn create_share(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<CreateShare>,
) -> Result<Json<ShareDto>, NativeApiError> {
    validate_share_type(request.share_type)?;
    resource_name(&state, request.share_type, request.resource_id).await?;
    require_share_resource_access(&state, principal, request.share_type, request.resource_id)
        .await?;
    let expires_at = parse_optional_time(request.expires_at.as_deref())?;
    let state_for_insert = state.clone();
    let owner_user_id = principal.user_id;
    let share_type = request.share_type;
    let resource_id = request.resource_id;
    let description = request.description;
    let dto = retry_unique_share_token(new_share_unique_id, move |unique_id| {
        let state = state_for_insert.clone();
        let description = description.clone();
        async move {
            let id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO native_shares (id, public_api_id, share_unique_id, owner_user_id, share_type, \
             resource_public_api_id, description, is_downloadable, expires_at, created_at, updated_at) \
             VALUES ($1, $1, $2, $3, $4, $5, $6, $7, $8, now(), now()) \
             ON CONFLICT (share_unique_id) DO NOTHING",
        )
        .bind(id)
        .bind(unique_id)
                .bind(owner_user_id)
                .bind(share_type)
                .bind(resource_id)
                .bind(description.as_deref())
        .bind(request.is_downloadable)
        .bind(expires_at)
        .execute(&state.pool)
        .await?;
            if inserted.rows_affected() == 1 {
                return Ok(Some(
                    share_dto(&state, load_share(&state, id).await?).await?,
                ));
            }
            Ok(None)
        }
    })
    .await?;
    // WP-09: notify the share owner that a share was created.
    // Fire-and-record: a delivery failure never fails share creation
    // (PRD-NOTIF-002).
    if let Some(email) = user_email_by_id(&state.pool, principal.user_id).await {
        state.notifications.dispatch(
            "share.created",
            email,
            vec![
                ("shareUrl".to_string(), dto.share_url.clone()),
                (
                    "description".to_string(),
                    dto.description.clone().unwrap_or_default(),
                ),
            ],
        );
    }
    Ok(Json(dto))
}

async fn retry_unique_share_token<T, Generate, Insert, InsertFuture>(
    mut generate: Generate,
    mut insert: Insert,
) -> Result<T, NativeApiError>
where
    Generate: FnMut() -> Result<String, NativeApiError>,
    Insert: FnMut(String) -> InsertFuture,
    InsertFuture: Future<Output = Result<Option<T>, NativeApiError>>,
{
    for _ in 0..MAX_SHARE_TOKEN_ATTEMPTS {
        let token = generate()?;
        if let Some(inserted) = insert(token).await? {
            return Ok(inserted);
        }
    }
    Err(NativeApiError::Unavailable)
}

async fn get_share(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<ShareDto>, NativeApiError> {
    let row = load_share(&state, id).await?;
    require_share_owner_or_admin(&state, principal, row.owner_user_id).await?;
    Ok(Json(share_dto(&state, row).await?))
}

async fn update_share(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateShare>,
) -> Result<Json<ShareDto>, NativeApiError> {
    let row = load_share(&state, id).await?;
    require_share_owner_or_admin(&state, principal, row.owner_user_id).await?;
    let expires_at = parse_optional_time(request.expires_at.as_deref())?;
    sqlx::query(
        "UPDATE native_shares SET description = COALESCE($2, description), \
         expires_at = COALESCE($3, expires_at), \
         updated_at = now() WHERE public_api_id = $1",
    )
    .bind(id)
    .bind(request.description)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    Ok(Json(
        share_dto(&state, load_share(&state, id).await?).await?,
    ))
}

async fn delete_share(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let row = load_share(&state, id).await?;
    require_share_owner_or_admin(&state, principal, row.owner_user_id).await?;
    sqlx::query("DELETE FROM native_shares WHERE public_api_id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::OK)
}

async fn public_share(
    State(state): State<NativeState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(unique_id): Path<String>,
) -> Result<Response, NativeApiError> {
    let client = state
        .client_addresses
        .resolve(
            peer,
            header_value(&headers, "forwarded"),
            header_value(&headers, "x-forwarded-for"),
        )
        .map_err(|_| NativeApiError::BadRequest)?;
    let actor_digest: [u8; 32] = Sha256::digest(client.to_string()).into();
    let rate = AuthStore::new()
        .record_rate_limit_attempt(
            &state.pool,
            &RateLimitPolicy {
                scope: "native-public-share",
                max_attempts: 120,
                window: Duration::minutes(1),
                max_retained_keys: 100_000,
            },
            &actor_digest,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|_| NativeApiError::Unavailable)?;
    if matches!(rate, RateLimitOutcome::Limited { .. }) {
        return Ok(StatusCode::TOO_MANY_REQUESTS.into_response());
    }
    if !valid_share_token(&unique_id) {
        return Err(NativeApiError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    // Public-share authorization and DTO assembly must observe one catalog/ACL
    // snapshot. These small mapping tables also need a predicate lock because
    // an inserted first binding can revoke previously open library access.
    sqlx::query(
        "LOCK TABLE native_user_library_groups, native_library_group_libraries IN SHARE MODE",
    )
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query_as::<_, ShareRow>(
        "SELECT public_api_id, share_unique_id, owner_user_id, share_type, resource_public_api_id, \
         description, is_downloadable, visit_count, expires_at, last_visited_at, created_at, \
         1::bigint AS total_count FROM native_shares WHERE share_unique_id = $1 FOR UPDATE",
    )
    .bind(&unique_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    if row
        .expires_at
        .is_some_and(|expires| expires < OffsetDateTime::now_utc())
    {
        return Err(NativeApiError::NotFound);
    }
    if !share_resource_access_connection(
        &mut tx,
        row.owner_user_id,
        row.share_type,
        row.resource_public_api_id,
    )
    .await?
    {
        return Err(NativeApiError::NotFound);
    }
    let response = public_share_dto_connection(&state, &mut tx, &row).await?;
    sqlx::query(
        "UPDATE native_shares SET visit_count = visit_count + 1, last_visited_at = now(), \
         updated_at = now() WHERE public_api_id = $1",
    )
    .bind(row.public_api_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(response).into_response())
}

async fn load_request(state: &NativeState, id: Uuid) -> Result<RequestRow, NativeApiError> {
    let sql = format!("{REQUEST_SELECT} WHERE r.public_api_id = $1");
    sqlx::query_as::<_, RequestRow>(AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(NativeApiError::NotFound)
}

async fn request_internal_id(state: &NativeState, id: Uuid) -> Result<Uuid, NativeApiError> {
    sqlx::query_scalar("SELECT id FROM native_requests WHERE public_api_id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(NativeApiError::NotFound)
}

async fn load_comment(state: &NativeState, id: Uuid) -> Result<CommentRow, NativeApiError> {
    sqlx::query_as::<_, CommentRow>(
        "SELECT c.public_api_id AS api_key, parent.public_api_id AS parent_comment_api_key, \
         c.body, c.is_system, c.created_at, u.public_api_id AS creator_public_id, \
         u.username AS creator_name, 1::bigint AS total_count FROM native_request_comments c \
         LEFT JOIN native_request_comments parent ON parent.id = c.parent_comment_id \
         LEFT JOIN native_users u ON u.id = c.created_by_user_id WHERE c.public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)
}

async fn load_share(state: &NativeState, id: Uuid) -> Result<ShareRow, NativeApiError> {
    sqlx::query_as::<_, ShareRow>(
        "SELECT public_api_id, share_unique_id, owner_user_id, share_type, resource_public_api_id, \
         description, is_downloadable, visit_count, expires_at, last_visited_at, created_at, \
         1::bigint AS total_count FROM native_shares WHERE public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)
}

async fn require_share_owner_or_admin(
    state: &NativeState,
    principal: NativePrincipal,
    owner: Uuid,
) -> Result<(), NativeApiError> {
    if principal.user_id == owner {
        return Ok(());
    }
    let is_admin = sqlx::query_scalar::<_, bool>(
        "SELECT is_admin FROM native_users WHERE id = $1 AND enabled AND NOT locked AND NOT blacklisted",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(false);
    if is_admin {
        Ok(())
    } else {
        Err(NativeApiError::Forbidden)
    }
}

async fn require_share_resource_access(
    state: &NativeState,
    principal: NativePrincipal,
    kind: i16,
    id: Uuid,
) -> Result<(), NativeApiError> {
    if principal.is_admin {
        return Ok(());
    }
    if share_resource_access(state, principal.user_id, kind, id).await? {
        Ok(())
    } else {
        Err(NativeApiError::Forbidden)
    }
}

async fn share_resource_access(
    state: &NativeState,
    owner_user_id: Uuid,
    kind: i16,
    id: Uuid,
) -> Result<bool, NativeApiError> {
    let mut connection = state.pool.acquire().await?;
    share_resource_access_connection(&mut connection, owner_user_id, kind, id).await
}

async fn share_resource_access_connection(
    connection: &mut PgConnection,
    owner_user_id: Uuid,
    kind: i16,
    id: Uuid,
) -> Result<bool, NativeApiError> {
    let active_admin = sqlx::query_scalar::<_, bool>(
        "SELECT is_admin FROM native_users
         WHERE id=$1 AND enabled AND NOT locked AND NOT blacklisted FOR SHARE",
    )
    .bind(owner_user_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(is_admin) = active_admin else {
        return Ok(false);
    };
    match kind {
        1 => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT true FROM tracks track
                 JOIN releases release ON release.id=track.release_id
                 WHERE track.public_api_id=$2 AND release.status='active'
                   AND ($3 OR native_user_can_access_release($1,track.release_id))
                 LIMIT 1 FOR SHARE OF track,release",
        )
        .bind(owner_user_id)
        .bind(id)
        .bind(is_admin)
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(false)),
        2 => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT true FROM releases release
                 WHERE release.public_api_id=$2 AND release.status='active'
                   AND ($3 OR native_user_can_access_release($1,release.id))
                 LIMIT 1 FOR SHARE OF release",
        )
        .bind(owner_user_id)
        .bind(id)
        .bind(is_admin)
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(false)),
        3 => {
            // Playlist mutations lock the owning row before changing items. A
            // share lock therefore freezes both membership and item order while
            // the nested release predicates and response are evaluated.
            let accessible = sqlx::query_scalar::<_, bool>(
                "SELECT true FROM native_playlists playlist
                 WHERE playlist.public_api_id=$2
                   AND ($3 OR playlist.owner_user_id=$1 OR playlist.is_public)
                   AND NOT EXISTS(
                     SELECT 1 FROM native_playlist_items item
                     JOIN tracks track ON track.id=item.track_id
                     JOIN releases release ON release.id=track.release_id
                     WHERE item.playlist_id=playlist.id
                       AND (release.status<>'active'
                            OR (NOT $3 AND NOT native_user_can_access_release($1,release.id)))
                   )
                 LIMIT 1 FOR SHARE OF playlist",
            )
            .bind(owner_user_id)
            .bind(id)
            .bind(is_admin)
            .fetch_optional(&mut *connection)
            .await?
            .unwrap_or(false);
            if accessible {
                sqlx::query(
                    "SELECT item.id FROM native_playlists playlist
                     JOIN native_playlist_items item ON item.playlist_id=playlist.id
                     JOIN tracks track ON track.id=item.track_id
                     JOIN releases release ON release.id=track.release_id
                     WHERE playlist.public_api_id=$1
                     FOR SHARE OF item,track,release",
                )
                .bind(id)
                .fetch_all(&mut *connection)
                .await?;
            }
            Ok(accessible)
        }
        4 => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT true FROM artists artist
                 WHERE artist.public_api_id=$2
                   AND artist.status NOT IN ('merged','deprecated')
                   AND ($3 OR native_user_can_access_artist($1,artist.id))
                 LIMIT 1 FOR SHARE OF artist",
        )
        .bind(owner_user_id)
        .bind(id)
        .bind(is_admin)
        .fetch_optional(&mut *connection)
        .await?
        .unwrap_or(false)),
        _ => Ok(false),
    }
}

async fn share_dto(state: &NativeState, row: ShareRow) -> Result<ShareDto, NativeApiError> {
    let name = resource_name(state, row.share_type, row.resource_public_api_id).await?;
    let owner = user_dto(state, row.owner_user_id).await?;
    let image_id = format!(
        "{}_{}",
        share_image_kind(row.share_type)?,
        row.resource_public_api_id
    );
    let image = format!("{}/images/{image_id}/1024", state.public_origin);
    let thumbnail = format!("{}/images/{image_id}/80", state.public_origin);
    Ok(ShareDto {
        id: row.public_api_id,
        share_url: format!("{}/share/{}", state.public_origin, row.share_unique_id),
        share_type: share_type_name(row.share_type)?,
        resource_id: row.resource_public_api_id,
        resource_name: name,
        resource_thumbnail_url: thumbnail,
        resource_image_url: image,
        description: row.description,
        is_downloadable: row.is_downloadable,
        visit_count: row.visit_count,
        owner,
        created_at: row.created_at.to_string(),
        expires_at: row.expires_at.map(|value| value.to_string()),
        last_visited_at: row.last_visited_at.map(|value| value.to_string()),
    })
}

async fn public_share_dto_connection(
    state: &NativeState,
    connection: &mut PgConnection,
    row: &ShareRow,
) -> Result<PublicShareResponse, NativeApiError> {
    let resource_name =
        resource_name_connection(connection, row.share_type, row.resource_public_api_id).await?;
    let image_id = format!(
        "{}_{}",
        share_image_kind(row.share_type)?,
        row.resource_public_api_id
    );
    let thumbnail_url = format!("{}/images/{image_id}/80", state.public_origin);
    let image_url = format!("{}/images/{image_id}/1024", state.public_origin);
    let mut artist = None;
    let mut album = None;
    let mut song = None;
    let mut playlist = None;
    match row.share_type {
        4 => {
            artist = Some(PublicArtistInfo {
                id: row.resource_public_api_id,
                name: resource_name.clone(),
            })
        }
        2 => {
            album = Some(
                load_public_release_connection(state, connection, row.resource_public_api_id)
                    .await?,
            )
        }
        1 => {
            song = Some(
                load_public_track_connection(state, connection, row.resource_public_api_id, 0)
                    .await?,
            )
        }
        3 => {
            playlist = Some(
                load_public_playlist_connection(state, connection, row.resource_public_api_id)
                    .await?,
            )
        }
        _ => return Err(NativeApiError::NotFound),
    }
    Ok(PublicShareResponse {
        share_type: share_type_name(row.share_type)?,
        resource_name,
        description: row.description.clone(),
        thumbnail_url,
        image_url,
        is_downloadable: row.is_downloadable,
        created_at: row.created_at.to_string(),
        expires_at: row.expires_at.map(|v| v.to_string()),
        artist,
        album,
        song,
        playlist,
    })
}

async fn resource_name(state: &NativeState, kind: i16, id: Uuid) -> Result<String, NativeApiError> {
    let mut connection = state.pool.acquire().await?;
    resource_name_connection(&mut connection, kind, id).await
}

async fn resource_name_connection(
    connection: &mut PgConnection,
    kind: i16,
    id: Uuid,
) -> Result<String, NativeApiError> {
    validate_share_type(kind)?;
    let sql = match kind {
        1 => "SELECT title FROM tracks WHERE public_api_id = $1",
        2 => "SELECT title FROM releases WHERE public_api_id = $1",
        3 => "SELECT name FROM native_playlists WHERE public_api_id = $1",
        4 => "SELECT canonical_name FROM artists WHERE public_api_id = $1",
        _ => return Err(NativeApiError::BadRequest),
    };
    sqlx::query_scalar::<_, String>(sql)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(NativeApiError::NotFound)
}

async fn load_public_track_connection(
    state: &NativeState,
    connection: &mut PgConnection,
    id: Uuid,
    fallback_number: i32,
) -> Result<PublicTrackInfo, NativeApiError> {
    let row = sqlx::query_as::<_, (String, i32, i64)>(
        "SELECT title, COALESCE(track_number, ordinal), COALESCE(duration_ms, 0)
         FROM tracks WHERE public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    Ok(PublicTrackInfo {
        id,
        title: row.0,
        track_number: if row.1 < 1 { fallback_number } else { row.1 },
        duration_ms: row.2 as f64,
        stream_url: format!("{}/rest/stream?id=song_{id}", state.public_origin),
    })
}

/// One set-based Track row for public-share assembly.
///
/// Phase 9 WP-01 replaced the per-Track sequential DTO loads with one query
/// per shared Release/Playlist (assembly stays bounded by
/// `MAX_NATIVE_PLAYLIST_ITEMS`, enforced at playlist write time). The mapping
/// preserves the previous shape exactly: `effective_number` is
/// `COALESCE(track_number, ordinal)` and the positional fallback below 1 is
/// retained verbatim even though catalog CHECK constraints (`ordinal > 0`)
/// make it unreachable for valid rows.
#[derive(Debug, FromRow)]
struct PublicTrackRow {
    public_api_id: Uuid,
    title: String,
    effective_number: i32,
    duration_ms: i64,
    fallback_number: i32,
}

impl PublicTrackRow {
    fn into_dto(self, public_origin: &str) -> PublicTrackInfo {
        PublicTrackInfo {
            id: self.public_api_id,
            title: self.title,
            track_number: if self.effective_number < 1 {
                self.fallback_number
            } else {
                self.effective_number
            },
            duration_ms: self.duration_ms as f64,
            stream_url: format!("{public_origin}/rest/stream?id=song_{}", self.public_api_id),
        }
    }
}

async fn load_public_release_connection(
    state: &NativeState,
    connection: &mut PgConnection,
    id: Uuid,
) -> Result<PublicReleaseInfo, NativeApiError> {
    let row = sqlx::query_as::<_, (String, i32, String)>(
        "SELECT release.title, COALESCE(EXTRACT(year FROM release.release_date)::integer, 0),
                COALESCE(release.display_artist, artist.canonical_name)
         FROM releases release
         JOIN LATERAL (
             SELECT billed_artist.canonical_name
             FROM release_artists billing
             JOIN artists billed_artist ON billed_artist.id=billing.artist_id
             WHERE billing.release_id=release.id
             ORDER BY billing.presentation_order,billed_artist.id LIMIT 1
         ) artist ON true
         WHERE release.public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    // `fallback_number` decodes `track_number` as non-null exactly like the
    // previous outer query, so a NULL track_number fails the same way.
    let tracks = sqlx::query_as::<_, PublicTrackRow>(
        "SELECT public_api_id, title, COALESCE(track_number, ordinal) AS effective_number, \
         COALESCE(duration_ms, 0) AS duration_ms, track_number AS fallback_number \
         FROM tracks WHERE release_id = \
         (SELECT id FROM releases WHERE public_api_id = $1) \
         ORDER BY disc_number, track_number, public_api_id",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    let songs = tracks
        .into_iter()
        .map(|track| track.into_dto(&state.public_origin))
        .collect();
    Ok(PublicReleaseInfo {
        id,
        name: row.0,
        release_year: row.1,
        artist_name: row.2,
        songs,
    })
}

async fn load_public_playlist_connection(
    state: &NativeState,
    connection: &mut PgConnection,
    id: Uuid,
) -> Result<PublicPlaylistInfo, NativeApiError> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT name, description FROM native_playlists WHERE public_api_id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    let tracks = sqlx::query_as::<_, PublicTrackRow>(
        "SELECT t.public_api_id, t.title, COALESCE(t.track_number, t.ordinal) AS effective_number, \
         COALESCE(t.duration_ms, 0) AS duration_ms, pi.position AS fallback_number \
         FROM native_playlist_items pi JOIN tracks t ON t.id = pi.track_id \
         WHERE pi.playlist_id = (SELECT id FROM native_playlists WHERE public_api_id = $1) \
         ORDER BY pi.position",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await?;
    let songs = tracks
        .into_iter()
        .map(|track| track.into_dto(&state.public_origin))
        .collect();
    Ok(PublicPlaylistInfo {
        id,
        name: row.0,
        description: row.1,
        songs,
    })
}

async fn user_dto(state: &NativeState, user_id: Uuid) -> Result<UserDto, NativeApiError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            bool,
            bool,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT public_api_id, username, email, is_admin, is_editor, created_at, updated_at \
         FROM native_users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    let roles = sqlx::query_scalar::<_, String>(
        "SELECT role FROM native_user_roles WHERE user_id = $1
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
    Ok(UserDto {
        id: row.0,
        thumbnail_url: format!("{}/images/user_{}/80", state.public_origin, row.0),
        image_url: format!("{}/images/user_{}/1024", state.public_origin, row.0),
        username: row.1,
        email: row.2,
        is_admin: row.3,
        is_editor: row.4,
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
        created_at: row.5.to_string(),
        updated_at: row.6.to_string(),
    })
}

fn request_summary(row: RequestRow) -> Result<RequestSummary, NativeApiError> {
    Ok(RequestSummary {
        api_key: row.public_api_id,
        category: category_name(row.category)?,
        status: status_name(row.status)?,
        description: row.description,
        artist_name: row.artist_name,
        album_title: row.release_title,
        song_title: row.track_title,
        release_year: row.release_year,
        created_at: row.created_at.to_string(),
        updated_at: row.updated_at.to_string(),
        last_activity_at: row.last_activity_at.to_string(),
        last_activity_type: row.last_activity_type,
        comment_count: i32::try_from(row.comment_count).map_err(|_| NativeApiError::Unavailable)?,
        created_by_user: UserSummary {
            api_key: row.creator_public_id,
            user_name: row.creator_name,
        },
    })
}

fn request_detail(row: RequestRow) -> Result<RequestDetail, NativeApiError> {
    let last_activity_user = match (row.activity_user_public_id, row.activity_user_name) {
        (Some(api_key), Some(user_name)) => Some(UserSummary { api_key, user_name }),
        _ => None,
    };
    Ok(RequestDetail {
        api_key: row.public_api_id,
        category: category_name(row.category)?,
        status: status_name(row.status)?,
        description: row.description,
        artist_name: row.artist_name,
        target_artist_api_key: row.target_artist_public_api_id,
        album_title: row.release_title,
        target_album_api_key: row.target_release_public_api_id,
        song_title: row.track_title,
        target_song_api_key: row.target_track_public_api_id,
        release_year: row.release_year,
        external_url: row.external_url,
        notes: row.notes,
        created_at: row.created_at.to_string(),
        updated_at: row.updated_at.to_string(),
        last_activity_at: row.last_activity_at.to_string(),
        last_activity_type: row.last_activity_type,
        comment_count: i32::try_from(row.comment_count).map_err(|_| NativeApiError::Unavailable)?,
        created_by_user: UserSummary {
            api_key: row.creator_public_id,
            user_name: row.creator_name,
        },
        last_activity_user,
    })
}

fn unread_summary(row: RequestRow) -> Result<UnreadRequestSummary, NativeApiError> {
    let activity_user = match (row.activity_user_public_id, row.activity_user_name) {
        (Some(api_key), Some(user_name)) => Some(UserSummary { api_key, user_name }),
        _ => None,
    };
    let description = if row.description.chars().count() > 100 {
        format!(
            "{}...",
            row.description.chars().take(100).collect::<String>()
        )
    } else {
        row.description
    };
    Ok(UnreadRequestSummary {
        api_key: row.public_api_id,
        category: category_name(row.category)?,
        status: status_name(row.status)?,
        description,
        last_activity_at: row.last_activity_at.to_string(),
        last_activity_type: row.last_activity_type,
        last_activity_user: activity_user,
    })
}

fn comment_dto(row: CommentRow) -> RequestComment {
    let created_by_user = match (row.creator_public_id, row.creator_name) {
        (Some(api_key), Some(user_name)) => Some(UserSummary { api_key, user_name }),
        _ => None,
    };
    RequestComment {
        api_key: row.api_key,
        parent_comment_api_key: row.parent_comment_api_key,
        body: row.body,
        is_system: row.is_system,
        created_at: row.created_at.to_string(),
        created_by_user,
    }
}

fn validate_request(category: i16, description: &str) -> Result<(), NativeApiError> {
    if !(1..=5).contains(&category)
        || description.trim().is_empty()
        || description.chars().count() > 10_000
    {
        Err(NativeApiError::BadRequest)
    } else {
        Ok(())
    }
}

fn validate_share_type(value: i16) -> Result<(), NativeApiError> {
    if (1..=4).contains(&value) {
        Ok(())
    } else {
        Err(NativeApiError::BadRequest)
    }
}

fn category_name(value: i16) -> Result<&'static str, NativeApiError> {
    match value {
        1 => Ok("AddAlbum"),
        2 => Ok("AddSong"),
        3 => Ok("ArtistCorrection"),
        4 => Ok("AlbumCorrection"),
        5 => Ok("General"),
        _ => Err(NativeApiError::Unavailable),
    }
}

fn status_name(value: i16) -> Result<&'static str, NativeApiError> {
    match value {
        1 => Ok("Pending"),
        2 => Ok("InProgress"),
        3 => Ok("Completed"),
        4 => Ok("Rejected"),
        _ => Err(NativeApiError::Unavailable),
    }
}

fn share_type_name(value: i16) -> Result<&'static str, NativeApiError> {
    match value {
        1 => Ok("Song"),
        2 => Ok("Album"),
        3 => Ok("Playlist"),
        4 => Ok("Artist"),
        _ => Err(NativeApiError::Unavailable),
    }
}

fn share_image_kind(value: i16) -> Result<&'static str, NativeApiError> {
    match value {
        1 => Ok("song"),
        2 => Ok("album"),
        3 => Ok("playlist"),
        4 => Ok("artist"),
        _ => Err(NativeApiError::Unavailable),
    }
}

fn new_share_unique_id() -> Result<String, NativeApiError> {
    let mut bytes = [0_u8; SHARE_TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| NativeApiError::Unavailable)?;
    Ok(encode_share_token(bytes))
}

fn encode_share_token(bytes: [u8; SHARE_TOKEN_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(SHARE_TOKEN_BYTES * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
}

fn valid_share_token(token: &str) -> bool {
    token.len() == SHARE_TOKEN_BYTES * 2 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn parse_optional_time(value: Option<&str>) -> Result<Option<OffsetDateTime>, NativeApiError> {
    value
        .map(|text| {
            OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .map_err(|_| NativeApiError::BadRequest)
        })
        .transpose()
}

fn page(
    page: Option<i32>,
    size: Option<i32>,
    default_size: i32,
) -> Result<(i32, i32, i64), NativeApiError> {
    let page = page.unwrap_or(1);
    let size = size.unwrap_or(default_size);
    if page < 1 || !(1..=MAX_PAGE_SIZE).contains(&size) {
        return Err(NativeApiError::BadRequest);
    }
    Ok((page, size, i64::from(page - 1) * i64::from(size)))
}

fn metadata(total: i64, page: i32, size: i32) -> Result<PaginationMetadata, NativeApiError> {
    PaginationMetadata::new(
        total,
        u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?,
        u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?,
    )
    .map_err(|_| NativeApiError::BadRequest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_inputs_are_bounded() {
        assert!(page(Some(0), Some(20), 20).is_err());
        assert!(page(Some(1), Some(501), 20).is_err());
        assert!(validate_request(0, "valid").is_err());
        assert!(validate_request(1, "   ").is_err());
        assert!(validate_share_type(5).is_err());
    }

    #[test]
    fn frozen_enum_names_are_stable() {
        assert_eq!(category_name(1).ok(), Some("AddAlbum"));
        assert_eq!(status_name(3).ok(), Some("Completed"));
        assert_eq!(share_type_name(4).ok(), Some("Artist"));
    }

    #[test]
    fn share_tokens_have_128_bits_of_csprng_material() {
        assert_eq!(
            encode_share_token([
                0x00, 0x01, 0x10, 0x11, 0x7f, 0x80, 0xfe, 0xff, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
                0xef, 0x42,
            ]),
            "000110117f80feff23456789abcdef42"
        );
        let tokens = (0..1024)
            .map(|_| new_share_unique_id())
            .collect::<Result<std::collections::HashSet<_>, _>>();
        assert!(tokens.as_ref().is_ok_and(|values| values.len() == 1024));
        assert!(tokens.is_ok_and(|values| {
            values.iter().all(|token| {
                token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        }));
    }

    #[tokio::test]
    async fn share_token_collision_retry_is_bounded_and_deterministic() {
        let mut generated = (0_u8..).map(|value| format!("{value:032x}"));
        let mut attempts = Vec::new();
        let inserted = retry_unique_share_token(
            || generated.next().ok_or(NativeApiError::Unavailable),
            |token| {
                attempts.push(token.clone());
                std::future::ready(Ok((token.ends_with('2')).then_some(token)))
            },
        )
        .await;
        assert_eq!(
            inserted.ok().as_deref(),
            Some("00000000000000000000000000000002")
        );
        assert_eq!(attempts.len(), 3);

        let exhausted = retry_unique_share_token(
            || Ok("00000000000000000000000000000000".to_owned()),
            |_| std::future::ready(Ok::<Option<()>, NativeApiError>(None)),
        )
        .await;
        assert!(matches!(exhausted, Err(NativeApiError::Unavailable)));
    }
}
