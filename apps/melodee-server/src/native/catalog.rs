//! PostgreSQL-backed native-v1 catalog browse/detail adapters.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{AssertSqlSafe, Row};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use melodee_persistence::{NativeUserStateMutation, NativeUserStateStore};

use super::{
    Authenticated, NativePrincipal, NativeState,
    dto::{
        AdminUserInfoDto, AdvancedSearchRequest, AdvancedSearchResponse, AdvancedSearchResults,
        AlbumDto, ArtistDto, GenreDto, GenreListResponse, GenreSongsResponse, HistoryItemDto,
        HistoryResponse, LyricsDto, PagedResponse, PaginationMetadata, SearchRequest,
        SearchResultDto, SearchResultResponse, SearchSuggestResponse, SearchSuggestionDto,
        SimilarItemDto, SimilarResponse, SongDto, TimeSeriesDataPointDto, TimeSeriesResponse,
        TopItemResponse, TopItemsResponse, UserStatsResponse, duration_formatted,
    },
    error::NativeApiError,
};

const DEFAULT_PAGE_SIZE: u16 = 50;
const MAX_PAGE_SIZE: u16 = 500;
const MAX_PAGE_OFFSET: i64 = 1_000_000;
const MAX_QUERY_CHARS: usize = 256;
// The legacy native-v1 SongDataInfo serializer emits this display value for
// every song. OpenSubsonic and Jellyfin have separate serializers and retain
// the persisted media bitrate instead.
const LEGACY_NATIVE_DISPLAY_BITRATE_KBPS: i32 = 320;
const LEGACY_UNKNOWN_CREATED_AT: &str = "9999-01-01 00:00:00";
const LEGACY_UNKNOWN_UPDATED_AT: &str = "---";
const RANDOM_CANDIDATE_WINDOW: i64 = 10_000;
const RANDOM_ELIGIBILITY: &str = "release.status='active'
  AND ($1::uuid IS NULL OR EXISTS (
    SELECT 1 FROM artist_track_associations scoped
    JOIN artists scoped_artist ON scoped_artist.id=scoped.artist_id
    WHERE scoped.track_id=track.id AND scoped_artist.public_api_id=$1))
  AND ($2::uuid IS NULL OR release.public_api_id=$2)
  AND ($3::integer IS NULL OR extract(year FROM release.release_date) >= $3)
  AND ($4::integer IS NULL OR extract(year FROM release.release_date) <= $4)
  AND ($5::text IS NULL OR EXISTS (
    SELECT 1 FROM catalog_genre_terms genre
    WHERE genre.track_id=track.id AND genre.surface='native'
      AND genre.display_value ILIKE $5 ESCAPE E'\\\\'))
  AND (NOT $7 OR native_user_can_access_release($8,release.id))";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    pub q: Option<String>,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
    pub order_by: Option<String>,
    pub order_direction: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecentParams {
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongParams {
    pub count: Option<i32>,
    pub artist_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
    pub genre: Option<String>,
    pub from_year: Option<i32>,
    pub to_year: Option<i32>,
}

/// Native-v1 random-song response. The legacy controller wraps the selected
/// songs with the returned count instead of returning the array directly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongsResponse {
    pub count: usize,
    pub data: Vec<SongDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSongsParams {
    pub q: Option<String>,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
    pub filter_by_artist_api_key: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreParams {
    pub page: Option<i32>,
    pub limit: Option<i32>,
    pub order_by: Option<String>,
    pub order_direction: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListParams {
    pub page: Option<i32>,
    pub limit: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SuggestParams {
    pub q: Option<String>,
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SimilarParams {
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct StatsParams {
    pub days: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct LastFmAuthParams {
    pub callback: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LastFmSessionRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrobbleRequest {
    pub song_id: Uuid,
    pub player_name: String,
    pub scrobble_type: String,
    pub timestamp: Option<f64>,
    pub played_duration: Option<f64>,
    pub scrobble_type_value: Option<i32>,
}

#[derive(Debug, Clone, Copy)]
struct Page {
    number: u32,
    size: u16,
    offset: i64,
}

impl Page {
    fn parse(page: Option<i32>, size: Option<i32>) -> Result<Self, NativeApiError> {
        let page = page.unwrap_or(1);
        let size = size.unwrap_or(i32::from(DEFAULT_PAGE_SIZE));
        if page < 1 || size < 1 || size > i32::from(MAX_PAGE_SIZE) {
            return Err(NativeApiError::BadRequest);
        }
        let number = u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?;
        let size = u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?;
        let offset = i64::from(number.saturating_sub(1)) * i64::from(size);
        if offset > MAX_PAGE_OFFSET {
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

fn escaped_like_value(value: &str) -> Result<String, NativeApiError> {
    if value.chars().count() > MAX_QUERY_CHARS {
        return Err(NativeApiError::BadRequest);
    }
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(escaped)
}

fn contains_search_pattern(value: Option<&str>) -> Result<Option<String>, NativeApiError> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| escaped_like_value(value).map(|value| format!("%{value}%")))
        .transpose()
}

#[derive(Debug)]
struct ArtistRecord {
    internal_id: Uuid,
    public_id: Uuid,
    name: String,
    album_count: i64,
    song_count: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(Debug)]
struct AlbumRecord {
    internal_id: Uuid,
    public_id: Uuid,
    name: String,
    release_year: i32,
    song_count: i64,
    duration_ms: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    artist: ArtistRecord,
}

#[derive(Debug)]
struct SongRecord {
    internal_id: Uuid,
    public_id: Uuid,
    title: String,
    duration_ms: i64,
    song_number: i32,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    artist: ArtistRecord,
    album: AlbumRecord,
}

pub fn router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/artists", get(list_artists))
        .route("/api/v1/artists/recent", get(recent_artists))
        .route("/api/v1/artists/{id}", get(get_artist))
        .route("/api/v1/artists/{id}/albums", get(artist_albums))
        .route("/api/v1/artists/{id}/songs", get(artist_songs))
        .route("/api/v1/artists/starred/{id}/{value}", post(star_artist))
        .route("/api/v1/artists/hated/{id}/{value}", post(hate_artist))
        .route("/api/v1/artists/setrating/{id}/{rating}", post(rate_artist))
        .route("/api/v1/albums", get(list_albums))
        .route("/api/v1/albums/recent", get(recent_albums))
        .route("/api/v1/albums/{id}", get(get_album))
        .route("/api/v1/albums/{id}/songs", get(album_songs))
        .route("/api/v1/albums/starred/{id}/{value}", post(star_album))
        .route("/api/v1/albums/hated/{id}/{value}", post(hate_album))
        .route("/api/v1/albums/setrating/{id}/{rating}", post(rate_album))
        .route("/api/v1/songs", get(list_songs))
        .route("/api/v1/songs/recent", get(recent_songs))
        .route("/api/v1/songs/random", get(random_songs))
        .route("/api/v1/songs/{id}", get(get_song))
        .route("/api/v1/songs/{id}/lyrics", get(song_lyrics))
        .route("/api/v1/songs/starred/{id}/{value}", post(star_song))
        .route("/api/v1/songs/hated/{id}/{value}", post(hate_song))
        .route("/api/v1/songs/setrating/{id}/{rating}", post(rate_song))
        .route("/api/v1/search", post(search))
        .route("/api/v1/search/advanced", post(advanced_search))
        .route("/api/v1/search/suggest", get(search_suggest))
        .route("/api/v1/search/similar/{id}/{type}", get(similar))
        .route("/api/v1/search/songs", get(search_songs))
        .route("/api/v1/genres", get(list_genres))
        .route("/api/v1/genres/{id}/songs", get(genre_songs))
        .route("/api/v1/genres/starred", get(starred_genres))
        .route("/api/v1/genres/hated", get(hated_genres))
        .route("/api/v1/genres/starred/{id}/{value}", post(star_genre))
        .route("/api/v1/genres/hated/{id}/{value}", post(hate_genre))
        .route("/api/v1/scrobble", post(scrobble))
        .route("/api/v1/scrobble/lastfm/auth-url", get(lastfm_auth_url))
        .route("/api/v1/scrobble/lastfm/session", post(lastfm_session))
        .route(
            "/api/v1/scrobble/lastfm/disconnect",
            post(lastfm_disconnect),
        )
        .route("/api/v1/user/artists/liked", get(user_artists_liked))
        .route("/api/v1/user/artists/disliked", get(user_artists_disliked))
        .route("/api/v1/user/artists/rated", get(user_artists_rated))
        .route(
            "/api/v1/user/artists/top-rated",
            get(user_artists_top_rated),
        )
        .route(
            "/api/v1/user/artists/recently-played",
            get(user_artists_recent),
        )
        .route("/api/v1/user/albums/liked", get(user_albums_liked))
        .route("/api/v1/user/albums/disliked", get(user_albums_disliked))
        .route("/api/v1/user/albums/rated", get(user_albums_rated))
        .route("/api/v1/user/albums/top-rated", get(user_albums_top_rated))
        .route(
            "/api/v1/user/albums/recently-played",
            get(user_albums_recent),
        )
        .route("/api/v1/user/songs/liked", get(user_songs_liked))
        .route("/api/v1/user/songs/disliked", get(user_songs_disliked))
        .route("/api/v1/user/songs/rated", get(user_songs_rated))
        .route("/api/v1/user/songs/top-rated", get(user_songs_top_rated))
        .route("/api/v1/user/songs/recently-played", get(user_songs_recent))
        .route("/api/v1/user/last-played", get(user_last_played))
        .route("/api/v1/user/stats", get(user_stats))
        .route("/api/v1/user/stats/top-songs", get(user_top_songs))
        .route("/api/v1/user/stats/top-genres", get(user_top_genres))
        .route("/api/v1/user/stats/plays-per-day", get(user_plays_per_day))
        .route("/api/v1/user/stats/history", get(user_history))
        .route("/api/v1/admin/users", get(admin_users))
}

pub async fn star_artist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_artist_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_artist(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Starred(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn hate_artist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_artist_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_artist(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Hated(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn rate_artist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, rating)): Path<(Uuid, i16)>,
) -> Result<(), NativeApiError> {
    ensure_artist_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_artist(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Rating(rating),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn star_album(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_release_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_release(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Starred(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn hate_album(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_release_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_release(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Hated(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn rate_album(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, rating)): Path<(Uuid, i16)>,
) -> Result<(), NativeApiError> {
    ensure_release_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_release(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Rating(rating),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn star_song(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_track_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_track(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Starred(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn hate_song(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(Uuid, bool)>,
) -> Result<(), NativeApiError> {
    ensure_track_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_track(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Hated(value),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn rate_song(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, rating)): Path<(Uuid, i16)>,
) -> Result<(), NativeApiError> {
    ensure_track_access(&state, principal.user_id, id).await?;
    NativeUserStateStore::mutate_track(
        &state.pool,
        principal.user_id,
        id,
        NativeUserStateMutation::Rating(rating),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn scrobble(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Json(request): Json<ScrobbleRequest>,
) -> Result<(), NativeApiError> {
    if request.player_name.trim().is_empty() || !request.timestamp.is_none_or(f64::is_finite) {
        return Err(NativeApiError::BadRequest);
    }
    ensure_track_access(&state, principal.user_id, request.song_id).await?;
    let now = OffsetDateTime::now_utc();
    let timestamp = request.timestamp.unwrap_or(now.unix_timestamp() as f64);
    let played_at = OffsetDateTime::from_unix_timestamp(timestamp.round() as i64)
        .map_err(|_| NativeApiError::BadRequest)?;
    let event_id = format!(
        "{}:{}:{}",
        request.player_name,
        request.song_id,
        timestamp.to_bits()
    );
    match request.scrobble_type.to_ascii_lowercase().as_str() {
        "nowplaying" => {
            NativeUserStateStore::record_now_playing(
                &state.pool,
                principal.user_id,
                request.song_id,
                &event_id,
                played_at,
                now,
            )
            .await?;
            // fire-and-record Last.fm dispatch. Never blocks or fails
            // the write path; a no-op unless scrobbling.lastFm is enabled and
            // the user linked (legacy ADR-0004).
            crate::lastfm::LastfmDispatch::new(
                state.pool.clone(),
                state.lastfm_api_base_url.as_deref(),
            )
            .spawn_now_playing(principal.user_id, request.song_id);
        }
        "played" => {
            let duration = request
                .played_duration
                .map(|value| {
                    if !value.is_finite() || value < 0.0 || value > f64::from(i32::MAX) {
                        return Err(NativeApiError::BadRequest);
                    }
                    Ok(value.round() as i32)
                })
                .transpose()?;
            let result = NativeUserStateStore::record_scrobble(
                &state.pool,
                principal.user_id,
                request.song_id,
                &event_id,
                played_at,
                now,
                duration,
            )
            .await?;
            // Dispatch only on a freshly applied play: an adopted idempotent
            // replay must not double-scrobble Last.fm.
            if result == melodee_persistence::NativeScrobbleResult::Applied {
                crate::lastfm::LastfmDispatch::new(
                    state.pool.clone(),
                    state.lastfm_api_base_url.as_deref(),
                )
                .spawn_scrobble(principal.user_id, request.song_id);
            }
        }
        _ => return Err(NativeApiError::BadRequest),
    }
    Ok(())
}

pub async fn lastfm_auth_url(
    State(state): State<NativeState>,
    _authenticated: Authenticated,
    Query(params): Query<LastFmAuthParams>,
) -> Result<Json<serde_json::Value>, NativeApiError> {
    let valid_callback = params.callback.as_deref().is_some_and(|callback| {
        callback.parse::<axum::http::Uri>().ok().is_some_and(|uri| {
            matches!(uri.scheme_str(), Some("http" | "https")) && uri.authority().is_some()
        })
    });
    if !valid_callback {
        return Err(NativeApiError::CapturedBadRequest("Invalid callback url"));
    }
    let callback = params.callback.as_deref().unwrap_or_default();
    // the captured unconfigured shape remains the disabled-state
    // contract — blank apiKey (the default) reproduces the frozen golden.
    let api_key = match crate::lastfm::resolve_lastfm_settings(&state.pool).await {
        Ok(settings) => settings.api_key,
        Err(error) => {
            tracing::warn!(%error, "last.fm settings resolution failed");
            None
        }
    };
    let Some(api_key) = api_key else {
        return Err(NativeApiError::CapturedBadRequest("Last.fm not configured"));
    };
    Ok(Json(
        serde_json::json!({"url": crate::lastfm::build_auth_url(&api_key, callback)}),
    ))
}

pub async fn lastfm_session(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Json(request): Json<LastFmSessionRequest>,
) -> Result<Json<serde_json::Value>, NativeApiError> {
    if request.token.trim().is_empty() || request.token.chars().count() > 4_096 {
        return Err(NativeApiError::BadRequest);
    }
    // every exchange failure — unconfigured, transport, API error,
    // malformed response — keeps the captured 400 shape (evidence doc §3).
    let settings = crate::lastfm::resolve_lastfm_settings(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "last.fm settings resolution failed");
            NativeApiError::CapturedBadRequest("Unable to exchange Last.fm session key")
        })?;
    let (Some(api_key), Some(shared_secret)) = (settings.api_key, settings.shared_secret) else {
        return Err(NativeApiError::CapturedBadRequest(
            "Unable to exchange Last.fm session key",
        ));
    };
    let client = crate::lastfm::LastfmClient::new(
        api_key,
        shared_secret,
        state.lastfm_api_base_url.as_deref(),
    )
    .map_err(|error| {
        tracing::warn!(%error, "last.fm client construction failed");
        NativeApiError::CapturedBadRequest("Unable to exchange Last.fm session key")
    })?;
    let session = client.get_session(&request.token).await.map_err(|error| {
        tracing::warn!(%error, "last.fm session exchange failed");
        NativeApiError::CapturedBadRequest("Unable to exchange Last.fm session key")
    })?;
    // The session key is sealed + digested before it reaches the store; the
    // raw key is never persisted, logged, or returned (ADR-0134 §7 amended).
    crate::lastfm::store_linked_session(&state.pool, principal.user_id, &session)
        .await
        .map_err(|_| NativeApiError::CapturedBadRequest("Unable to save Last.fm session key"))?;
    Ok(Json(serde_json::json!({"message": "Last.fm linked"})))
}

pub async fn lastfm_disconnect(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
) -> Json<serde_json::Value> {
    // Idempotent: succeeds unconfigured and unlinked (captured golden).
    crate::lastfm::unlink_user(&state.pool, principal.user_id).await;
    Json(serde_json::json!({"message": "Last.fm disconnected"}))
}

fn canonical_order(
    value: Option<&str>,
    default: &'static str,
) -> Result<&'static str, NativeApiError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value.to_ascii_lowercase().as_str() {
        "name" | "title" => Ok("name"),
        "releasedate" | "release_date" => Ok("release_date"),
        "createdat" | "created_at" => Ok("created_at"),
        _ => Err(NativeApiError::BadRequest),
    }
}

fn canonical_direction(value: Option<&str>) -> Result<&'static str, NativeApiError> {
    match value.map(str::to_ascii_lowercase).as_deref() {
        None | Some("asc") => Ok("asc"),
        Some("desc") => Ok("desc"),
        Some(_) => Err(NativeApiError::BadRequest),
    }
}

fn artist_order_sql(order: &str, direction: &str, searching: bool) -> &'static str {
    match (order, direction) {
        ("name", "desc") => {
            "artist.sort_name DESC, artist.created_at ASC, artist.public_api_id ASC"
        }
        ("created_at", "asc") => "artist.created_at ASC, artist.public_api_id ASC",
        ("created_at", "desc") => "artist.created_at DESC, artist.public_api_id ASC",
        // The legacy search service has its own deterministic tie break for
        // normalized-name collisions. Preserve that captured order without
        // changing the ordinary browse ordering contract.
        ("name", "asc") if searching => "artist.sort_name ASC, artist.public_api_id ASC",
        _ => "artist.sort_name ASC, artist.created_at ASC, artist.public_api_id ASC",
    }
}

fn album_order_sql(order: &str, direction: &str) -> &'static str {
    match (order, direction) {
        ("name", "desc") => "release.sort_title DESC, release.public_api_id ASC",
        ("release_date", "asc") => "release.release_date ASC NULLS LAST, release.public_api_id ASC",
        ("release_date", "desc") => {
            "release.release_date DESC NULLS LAST, release.public_api_id ASC"
        }
        ("created_at", "asc") => "release.created_at ASC, release.public_api_id ASC",
        ("created_at", "desc") => "release.created_at DESC, release.public_api_id ASC",
        _ => "release.sort_title ASC, release.public_api_id ASC",
    }
}

fn song_order_sql(scoped: bool) -> &'static str {
    if scoped {
        // The legacy album/artist song queries did not add a title ordering.
        // Import order is represented by the preserved source timestamp, with
        // the public id providing a deterministic tie-breaker.
        "track.created_at ASC, track.public_api_id ASC"
    } else {
        "track.sort_title ASC, track.public_api_id ASC"
    }
}

fn timestamp(value: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

/// Render the legacy nullable LastUpdatedAt convention after migration.
///
/// The Rust catalog schema requires `updated_at`, so the importer clamps a
/// missing legacy value to `created_at` to preserve the invariant. Equality is
/// therefore the durable marker for the legacy null case at this wire
/// boundary; a genuinely later update remains a normal timestamp.
fn imported_timestamp(created: OffsetDateTime, updated: OffsetDateTime) -> String {
    if created == updated {
        LEGACY_UNKNOWN_UPDATED_AT.to_string()
    } else {
        timestamp(updated)
    }
}

fn timestamp_iso(value: OffsetDateTime) -> Result<String, NativeApiError> {
    value
        .format(&Rfc3339)
        .map_err(|_| NativeApiError::Unavailable)
}

fn image_url(origin: &str, kind: &str, id: Uuid, size: u16) -> String {
    format!("{}/images/{kind}_{id}/{size}", origin.trim_end_matches('/'))
}

fn bounded_i32(value: i64) -> Result<i32, NativeApiError> {
    i32::try_from(value).map_err(|_| NativeApiError::Unavailable)
}

async fn ensure_artist_access(
    state: &NativeState,
    user_id: Uuid,
    public_id: Uuid,
) -> Result<(), NativeApiError> {
    let visible = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM artists artist
           WHERE artist.public_api_id = $1
             AND native_user_can_access_artist($2, artist.id)
         )",
    )
    .bind(public_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    visible.then_some(()).ok_or(NativeApiError::NotFound)
}

async fn ensure_release_access(
    state: &NativeState,
    user_id: Uuid,
    public_id: Uuid,
) -> Result<(), NativeApiError> {
    let visible = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM releases release
           WHERE release.public_api_id = $1 AND release.status = 'active'
             AND native_user_can_access_release($2, release.id)
         )",
    )
    .bind(public_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    visible.then_some(()).ok_or(NativeApiError::NotFound)
}

async fn ensure_track_access(
    state: &NativeState,
    user_id: Uuid,
    public_id: Uuid,
) -> Result<(), NativeApiError> {
    let visible = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM tracks track JOIN releases release ON release.id = track.release_id
           WHERE track.public_api_id = $1 AND release.status = 'active'
             AND native_user_can_access_release($2, release.id)
         )",
    )
    .bind(public_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    visible.then_some(()).ok_or(NativeApiError::NotFound)
}

async fn artist_dto(
    state: &NativeState,
    principal: NativePrincipal,
    mut record: ArtistRecord,
) -> Result<ArtistDto, NativeApiError> {
    scope_artist_counts(
        &state.pool,
        principal.user_id,
        std::slice::from_mut(&mut record),
    )
    .await?;
    let annotation = state
        .user_state
        .artist_annotation(principal.user_id, record.internal_id)
        .await?;
    artist_dto_with_annotation(state, record, annotation)
}

fn artist_dto_with_annotation(
    state: &NativeState,
    record: ArtistRecord,
    annotation: super::NativeAnnotation,
) -> Result<ArtistDto, NativeApiError> {
    Ok(ArtistDto {
        id: record.public_id,
        thumbnail_url: image_url(&state.public_origin, "artist", record.public_id, 80),
        image_url: image_url(&state.public_origin, "artist", record.public_id, 1024),
        name: record.name,
        user_starred: annotation.starred,
        user_rating: annotation.rating,
        album_count: bounded_i32(record.album_count)?,
        song_count: bounded_i32(record.song_count)?,
        created_at: timestamp(record.created_at),
        updated_at: imported_timestamp(record.created_at, record.updated_at),
        biography: None,
        genres: None,
    })
}

fn embedded_artist(origin: &str, record: &ArtistRecord) -> ArtistDto {
    ArtistDto {
        id: record.public_id,
        thumbnail_url: image_url(origin, "artist", record.public_id, 80),
        image_url: image_url(origin, "artist", record.public_id, 1024),
        name: record.name.clone(),
        user_starred: false,
        user_rating: 0,
        album_count: 0,
        song_count: 0,
        created_at: String::new(),
        updated_at: String::new(),
        biography: None,
        genres: None,
    }
}

fn embedded_album_artist(origin: &str, record: &ArtistRecord) -> ArtistDto {
    let mut artist = embedded_artist(origin, record);
    artist.created_at = LEGACY_UNKNOWN_CREATED_AT.to_string();
    artist.updated_at = LEGACY_UNKNOWN_UPDATED_AT.to_string();
    artist
}

async fn album_dto(
    state: &NativeState,
    principal: NativePrincipal,
    record: AlbumRecord,
) -> Result<AlbumDto, NativeApiError> {
    let annotation = state
        .user_state
        .release_annotation(principal.user_id, record.internal_id)
        .await?;
    album_dto_with_annotation(state, record, annotation)
}

fn album_dto_with_annotation(
    state: &NativeState,
    record: AlbumRecord,
    annotation: super::NativeAnnotation,
) -> Result<AlbumDto, NativeApiError> {
    Ok(AlbumDto {
        id: record.public_id,
        artist: embedded_album_artist(&state.public_origin, &record.artist),
        thumbnail_url: image_url(&state.public_origin, "album", record.public_id, 80),
        image_url: image_url(&state.public_origin, "album", record.public_id, 1024),
        name: record.name,
        release_year: record.release_year,
        user_starred: annotation.starred,
        user_rating: annotation.rating,
        song_count: bounded_i32(record.song_count)?,
        duration_ms: record.duration_ms,
        duration_formatted: duration_formatted(record.duration_ms),
        created_at: timestamp(record.created_at),
        updated_at: imported_timestamp(record.created_at, record.updated_at),
        description: None,
        genre: None,
    })
}

async fn song_dto(
    state: &NativeState,
    principal: NativePrincipal,
    client_binding: &str,
    record: SongRecord,
) -> Result<SongDto, NativeApiError> {
    let annotation = state
        .user_state
        .track_annotation(principal.user_id, record.internal_id)
        .await?;
    let stream_url = state
        .stream_url_signer
        .stream_url(record.public_id, principal.public_api_id, client_binding)
        .await
        .map_err(|()| NativeApiError::Unavailable)?;
    song_dto_with_facts(state, record, annotation, stream_url)
}

fn song_dto_with_facts(
    state: &NativeState,
    record: SongRecord,
    annotation: super::NativeAnnotation,
    stream_url: String,
) -> Result<SongDto, NativeApiError> {
    let album_id = record.album.public_id;
    let album_name = record.album.name.clone();
    let album_year = record.album.release_year;
    let album = AlbumDto {
        id: album_id,
        artist: ArtistDto {
            id: Uuid::nil(),
            thumbnail_url: String::new(),
            image_url: String::new(),
            name: String::new(),
            user_starred: false,
            user_rating: 0,
            album_count: 0,
            song_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
            biography: None,
            genres: None,
        },
        thumbnail_url: image_url(&state.public_origin, "album", album_id, 80),
        image_url: image_url(&state.public_origin, "album", album_id, 1024),
        name: album_name,
        release_year: album_year,
        user_starred: false,
        user_rating: 0,
        song_count: 0,
        duration_ms: 0,
        duration_formatted: String::new(),
        created_at: String::new(),
        updated_at: String::new(),
        description: None,
        genre: None,
    };
    Ok(SongDto {
        id: record.public_id,
        artist: embedded_artist(&state.public_origin, &record.artist),
        album,
        stream_url,
        thumbnail_url: image_url(&state.public_origin, "song", record.public_id, 80),
        image_url: image_url(&state.public_origin, "song", record.public_id, 1024),
        title: record.title,
        duration_ms: record.duration_ms,
        duration_formatted: duration_formatted(record.duration_ms),
        user_starred: annotation.starred,
        user_rating: annotation.rating,
        song_number: record.song_number,
        bitrate: LEGACY_NATIVE_DISPLAY_BITRATE_KBPS,
        play_count: annotation.play_count,
        created_at: timestamp(record.created_at),
        updated_at: imported_timestamp(record.created_at, record.updated_at),
        genre: None,
    })
}

fn artist_from_row(row: &sqlx::postgres::PgRow, start: usize) -> Result<ArtistRecord, sqlx::Error> {
    Ok(ArtistRecord {
        internal_id: row.try_get(start)?,
        public_id: row.try_get(start + 1)?,
        name: row.try_get(start + 2)?,
        album_count: row.try_get(start + 3)?,
        song_count: row.try_get(start + 4)?,
        created_at: row.try_get(start + 5)?,
        updated_at: row.try_get(start + 6)?,
    })
}

fn album_from_row(row: &sqlx::postgres::PgRow, start: usize) -> Result<AlbumRecord, sqlx::Error> {
    Ok(AlbumRecord {
        internal_id: row.try_get(start)?,
        public_id: row.try_get(start + 1)?,
        name: row.try_get(start + 2)?,
        release_year: row.try_get(start + 3)?,
        song_count: row.try_get(start + 4)?,
        duration_ms: row.try_get(start + 5)?,
        created_at: row.try_get(start + 6)?,
        updated_at: row.try_get(start + 7)?,
        artist: artist_from_row(row, start + 8)?,
    })
}

fn song_from_row(row: &sqlx::postgres::PgRow) -> Result<SongRecord, sqlx::Error> {
    Ok(SongRecord {
        internal_id: row.try_get(0)?,
        public_id: row.try_get(1)?,
        title: row.try_get(2)?,
        duration_ms: row.try_get(3)?,
        song_number: row.try_get(4)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        artist: artist_from_row(row, 8)?,
        album: album_from_row(row, 15)?,
    })
}

async fn artist_dtos(
    state: &NativeState,
    principal: NativePrincipal,
    mut records: Vec<ArtistRecord>,
) -> Result<Vec<ArtistDto>, NativeApiError> {
    scope_artist_counts(&state.pool, principal.user_id, &mut records).await?;
    let annotations = state
        .user_state
        .artist_annotations(
            principal.user_id,
            records.iter().map(|record| record.internal_id).collect(),
        )
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    records
        .into_iter()
        .map(|record| {
            let annotation = annotations
                .get(&record.internal_id)
                .copied()
                .unwrap_or_default();
            artist_dto_with_annotation(state, record, annotation)
        })
        .collect()
}

async fn album_dtos(
    state: &NativeState,
    principal: NativePrincipal,
    records: Vec<AlbumRecord>,
) -> Result<Vec<AlbumDto>, NativeApiError> {
    let annotations = state
        .user_state
        .release_annotations(
            principal.user_id,
            records.iter().map(|record| record.internal_id).collect(),
        )
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    records
        .into_iter()
        .map(|record| {
            let annotation = annotations
                .get(&record.internal_id)
                .copied()
                .unwrap_or_default();
            album_dto_with_annotation(state, record, annotation)
        })
        .collect()
}

async fn song_dtos(
    state: &NativeState,
    principal: NativePrincipal,
    client_binding: &str,
    records: Vec<SongRecord>,
) -> Result<Vec<SongDto>, NativeApiError> {
    let annotations = state
        .user_state
        .track_annotations(
            principal.user_id,
            records.iter().map(|record| record.internal_id).collect(),
        )
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let urls = state
        .stream_url_signer
        .stream_urls(
            records.iter().map(|record| record.public_id).collect(),
            principal.public_api_id,
            client_binding,
        )
        .await
        .map_err(|()| NativeApiError::Unavailable)?;
    if urls.len() != records.len() {
        return Err(NativeApiError::Unavailable);
    }
    records
        .into_iter()
        .zip(urls)
        .map(|(record, stream_url)| {
            let annotation = annotations
                .get(&record.internal_id)
                .copied()
                .unwrap_or_default();
            song_dto_with_facts(state, record, annotation, stream_url)
        })
        .collect()
}

async fn has_library_bindings(pool: &sqlx::PgPool) -> Result<bool, NativeApiError> {
    Ok(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_library_group_libraries)",
        )
        .fetch_one(pool)
        .await?,
    )
}

async fn scope_artist_counts(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    records: &mut [ArtistRecord],
) -> Result<(), NativeApiError> {
    if records.is_empty() || !has_library_bindings(pool).await? {
        return Ok(());
    }
    let artist_ids = records
        .iter()
        .map(|record| record.internal_id)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, i64, i64)>(
        // Native-v1's Artist counters come from the legacy Artist-owned
        // Album/Song navigation. Performance and other credit rows remain
        // first-class domain relationships, but must not inflate these
        // compatibility counters.
        "SELECT artist.id,
                COALESCE(releases.album_count, 0)::bigint,
                COALESCE(tracks.song_count, 0)::bigint
         FROM artists artist
         LEFT JOIN (
           SELECT release_artist.artist_id,
                  count(DISTINCT release_artist.release_id)::bigint AS album_count
           FROM release_artists release_artist
           JOIN releases release ON release.id=release_artist.release_id
            AND release.status='active'
            AND native_user_can_access_release($2,release.id)
           WHERE release_artist.role='primary'
             AND release_artist.artist_id=ANY($1::uuid[])
           GROUP BY release_artist.artist_id
         ) releases ON releases.artist_id=artist.id
         LEFT JOIN (
           SELECT association.artist_id,
                  count(DISTINCT association.track_id)::bigint AS song_count
           FROM artist_track_associations association
           JOIN tracks track ON track.id=association.track_id
           JOIN releases release ON release.id=track.release_id
            AND release.status='active'
            AND native_user_can_access_release($2,release.id)
           WHERE association.artist_id=ANY($1::uuid[])
           GROUP BY association.artist_id
         ) tracks ON tracks.artist_id=artist.id
         WHERE artist.id=ANY($1::uuid[]) ",
    )
    .bind(&artist_ids)
    .bind(user_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(artist_id, album_count, song_count)| (artist_id, (album_count, song_count)))
    .collect::<HashMap<_, _>>();
    for record in records {
        let (album_count, song_count) = rows.get(&record.internal_id).copied().unwrap_or_default();
        record.album_count = album_count;
        record.song_count = song_count;
    }
    Ok(())
}

const ARTIST_COLUMNS: &str = "artist.id, artist.public_api_id, artist.canonical_name,
    COALESCE(aggregate.release_count, 0), COALESCE(aggregate.billed_track_count, 0),
    artist.created_at, artist.updated_at";

const ALBUM_COLUMNS: &str = "release.id, release.public_api_id, release.title,
    COALESCE(extract(year FROM release.release_date)::integer, 0),
    COALESCE(release_aggregate.track_count, 0), COALESCE(release_aggregate.duration_ms, 0),
    release.created_at, release.updated_at,
    primary_artist.id, primary_artist.public_api_id, primary_artist.canonical_name,
    COALESCE(primary_aggregate.release_count, 0), COALESCE(primary_aggregate.billed_track_count, 0),
    primary_artist.created_at, primary_artist.updated_at";

const ALBUM_JOINS: &str = "LEFT JOIN release_catalog_aggregates release_aggregate
      ON release_aggregate.release_id = release.id
    JOIN LATERAL (
      SELECT artist.* FROM release_artists billing
      JOIN artists artist ON artist.id = billing.artist_id
      WHERE billing.release_id = release.id AND billing.role = 'primary'
      ORDER BY billing.presentation_order, billing.id LIMIT 1
    ) primary_artist ON true
    LEFT JOIN artist_catalog_aggregates primary_aggregate
      ON primary_aggregate.artist_id = primary_artist.id";

const SONG_SELECT: &str = "SELECT track.id, track.public_api_id, track.title,
    COALESCE(track.duration_ms, media.duration_ms, 0),
    CASE WHEN COALESCE(track.disc_number, 1) > 1
      THEN COALESCE(track.disc_number, 1) * 100 + COALESCE(track.track_number, track.ordinal)
      ELSE COALESCE(track.track_number, track.ordinal) END,
    COALESCE((media.bitrate_bps / 1000)::integer, 0), track.created_at, track.updated_at,
    track_artist.id, track_artist.public_api_id, track_artist.canonical_name,
    COALESCE(track_artist_aggregate.release_count, 0),
    COALESCE(track_artist_aggregate.billed_track_count, 0),
    track_artist.created_at, track_artist.updated_at,
    release.id, release.public_api_id, release.title,
    COALESCE(extract(year FROM release.release_date)::integer, 0),
    COALESCE(release_aggregate.track_count, 0), COALESCE(release_aggregate.duration_ms, 0),
    release.created_at, release.updated_at,
    release_artist.id, release_artist.public_api_id, release_artist.canonical_name,
    COALESCE(release_artist_aggregate.release_count, 0),
    COALESCE(release_artist_aggregate.billed_track_count, 0),
    release_artist.created_at, release_artist.updated_at
  FROM tracks track
  JOIN releases release ON release.id = track.release_id AND release.status = 'active'
  JOIN media_assets media ON media.id = track.playable_media_asset_id
  LEFT JOIN release_catalog_aggregates release_aggregate ON release_aggregate.release_id = release.id
  JOIN LATERAL (
    SELECT artist.* FROM track_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.track_id = track.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) track_artist ON true
  LEFT JOIN artist_catalog_aggregates track_artist_aggregate
    ON track_artist_aggregate.artist_id = track_artist.id
  JOIN LATERAL (
    SELECT artist.* FROM release_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.release_id = release.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) release_artist ON true
  LEFT JOIN artist_catalog_aggregates release_artist_aggregate
    ON release_artist_aggregate.artist_id = release_artist.id";

const SONG_FROM_SELECTED: &str = "SELECT track.id, track.public_api_id, track.title,
    COALESCE(track.duration_ms, media.duration_ms, 0),
    CASE WHEN COALESCE(track.disc_number, 1) > 1
      THEN COALESCE(track.disc_number, 1) * 100 + COALESCE(track.track_number, track.ordinal)
      ELSE COALESCE(track.track_number, track.ordinal) END,
    COALESCE((media.bitrate_bps / 1000)::integer, 0), track.created_at, track.updated_at,
    track_artist.id, track_artist.public_api_id, track_artist.canonical_name,
    COALESCE(track_artist_aggregate.release_count, 0),
    COALESCE(track_artist_aggregate.billed_track_count, 0),
    track_artist.created_at, track_artist.updated_at,
    release.id, release.public_api_id, release.title,
    COALESCE(extract(year FROM release.release_date)::integer, 0),
    COALESCE(release_aggregate.track_count, 0), COALESCE(release_aggregate.duration_ms, 0),
    release.created_at, release.updated_at,
    release_artist.id, release_artist.public_api_id, release_artist.canonical_name,
    COALESCE(release_artist_aggregate.release_count, 0),
    COALESCE(release_artist_aggregate.billed_track_count, 0),
    release_artist.created_at, release_artist.updated_at
  FROM selected
  JOIN tracks track ON track.id = selected.id
  JOIN releases release ON release.id = track.release_id AND release.status = 'active'
  JOIN media_assets media ON media.id = track.playable_media_asset_id
  LEFT JOIN release_catalog_aggregates release_aggregate ON release_aggregate.release_id = release.id
  JOIN LATERAL (
    SELECT artist.* FROM track_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.track_id = track.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) track_artist ON true
  LEFT JOIN artist_catalog_aggregates track_artist_aggregate ON track_artist_aggregate.artist_id = track_artist.id
  JOIN LATERAL (
    SELECT artist.* FROM release_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.release_id = release.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) release_artist ON true
  LEFT JOIN artist_catalog_aggregates release_artist_aggregate ON release_artist_aggregate.artist_id = release_artist.id";

/// Resolve an ordered list of Track public IDs to the same frozen Song DTOs
/// used by catalog routes. Missing projection/catalog rows are skipped so a
/// stale smart-search hit cannot fail an otherwise valid response.
pub(crate) async fn song_dtos_by_public_ids(
    state: &NativeState,
    principal: NativePrincipal,
    client_binding: &str,
    public_ids: &[Uuid],
) -> Result<Vec<SongDto>, NativeApiError> {
    if public_ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = format!(
        "{SONG_SELECT} WHERE track.public_api_id = ANY($1::uuid[])
           AND native_user_can_access_release($2, release.id)
         ORDER BY track.public_api_id"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(public_ids)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let mut by_id = HashMap::with_capacity(rows.len());
    for row in rows {
        let record = song_from_row(&row)?;
        let public_id = record.public_id;
        by_id.insert(
            public_id,
            song_dto(state, principal, client_binding, record).await?,
        );
    }
    Ok(public_ids
        .iter()
        .filter_map(|public_id| by_id.get(public_id).cloned())
        .collect())
}

pub(crate) async fn artist_dtos_by_public_ids(
    state: &NativeState,
    principal: NativePrincipal,
    public_ids: &[Uuid],
) -> Result<Vec<ArtistDto>, NativeApiError> {
    if public_ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = format!(
        "SELECT {ARTIST_COLUMNS} FROM artists artist
         LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
         WHERE artist.public_api_id = ANY($1::uuid[])
           AND artist.status NOT IN ('merged', 'deprecated')
           AND native_user_can_access_artist($2, artist.id)
         ORDER BY artist.public_api_id"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(public_ids)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let records = rows
        .iter()
        .map(|row| artist_from_row(row, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let by_id = artist_dtos(state, principal, records)
        .await?
        .into_iter()
        .map(|dto| (dto.id, dto))
        .collect::<HashMap<_, _>>();
    Ok(public_ids
        .iter()
        .filter_map(|public_id| by_id.get(public_id).cloned())
        .collect())
}

pub(crate) async fn album_dtos_by_public_ids(
    state: &NativeState,
    principal: NativePrincipal,
    public_ids: &[Uuid],
) -> Result<Vec<AlbumDto>, NativeApiError> {
    if public_ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = format!(
        "SELECT {ALBUM_COLUMNS} FROM releases release {ALBUM_JOINS}
         WHERE release.public_api_id = ANY($1::uuid[]) AND release.status = 'active'
           AND native_user_can_access_release($2, release.id)
         ORDER BY release.public_api_id"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(public_ids)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let mut by_id = HashMap::with_capacity(rows.len());
    for row in rows {
        let record = album_from_row(&row, 0)?;
        let public_id = record.public_id;
        by_id.insert(public_id, album_dto(state, principal, record).await?);
    }
    Ok(public_ids
        .iter()
        .filter_map(|public_id| by_id.get(public_id).cloned())
        .collect())
}

async fn fetch_artists(
    state: &NativeState,
    principal: NativePrincipal,
    params: &ListParams,
    recent: bool,
) -> Result<PagedResponse<ArtistDto>, NativeApiError> {
    let page = Page::parse(params.page, params.page_size)?;
    let order = canonical_order(
        params.order_by.as_deref(),
        if recent { "created_at" } else { "name" },
    )?;
    let direction = canonical_direction(
        params
            .order_direction
            .as_deref()
            .or(recent.then_some("desc")),
    )?;
    let restricted = has_library_bindings(&state.pool).await?;
    let search_pattern = contains_search_pattern(params.q.as_deref())?;
    let query = format!(
        "SELECT {ARTIST_COLUMNS}
         FROM artists artist
         LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
         WHERE artist.status NOT IN ('merged', 'deprecated')
           AND (NOT $4 OR native_user_can_access_artist($5, artist.id))
           AND ($1::text IS NULL OR artist.normalized_name ILIKE $1 ESCAPE E'\\\\'
             OR EXISTS (SELECT 1 FROM artist_aliases alias
                        WHERE alias.artist_id = artist.id
                          AND alias.alias ILIKE $1 ESCAPE E'\\\\'))
         ORDER BY {} LIMIT $2 OFFSET $3",
        artist_order_sql(order, direction, search_pattern.is_some()),
    );
    // The dynamic string contains only module-owned SQL constants; all
    // request values remain bind parameters.
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(search_pattern.as_deref())
        .bind(i64::from(page.size))
        .bind(page.offset)
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM artists artist
         WHERE artist.status NOT IN ('merged','deprecated')
           AND (NOT $2 OR native_user_can_access_artist($3,artist.id))
           AND ($1::text IS NULL OR artist.normalized_name ILIKE $1 ESCAPE E'\\\\'
             OR EXISTS(SELECT 1 FROM artist_aliases alias
                       WHERE alias.artist_id=artist.id
                         AND alias.alias ILIKE $1 ESCAPE E'\\\\'))",
    )
    .bind(search_pattern.as_deref())
    .bind(restricted)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    let records = rows
        .iter()
        .map(|row| artist_from_row(row, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let data = artist_dtos(state, principal, records).await?;
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

pub async fn list_artists(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<ListParams>,
) -> Result<Json<PagedResponse<ArtistDto>>, NativeApiError> {
    Ok(Json(
        fetch_artists(&state, principal, &params, false).await?,
    ))
}

pub async fn recent_artists(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<RecentParams>,
) -> Result<Json<PagedResponse<ArtistDto>>, NativeApiError> {
    let limit = params.limit.unwrap_or(50);
    let list = ListParams {
        q: None,
        page: Some(1),
        page_size: Some(limit),
        order_by: Some("createdAt".to_string()),
        order_direction: Some("desc".to_string()),
    };
    Ok(Json(fetch_artists(&state, principal, &list, true).await?))
}

async fn fetch_artist_record(
    pool: &sqlx::PgPool,
    public_id: Uuid,
    user_id: Uuid,
) -> Result<ArtistRecord, NativeApiError> {
    let query = format!(
        "SELECT {ARTIST_COLUMNS} FROM artists artist
         LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
         WHERE artist.public_api_id = $1 AND artist.status NOT IN ('merged', 'deprecated')
           AND native_user_can_access_artist($2, artist.id)"
    );
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(public_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.as_ref()
        .map(|row| artist_from_row(row, 0))
        .transpose()?
        .ok_or(NativeApiError::NotFound)
}

pub async fn get_artist(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<ArtistDto>, NativeApiError> {
    let record = fetch_artist_record(&state.pool, id, principal.user_id).await?;
    Ok(Json(artist_dto(&state, principal, record).await?))
}

async fn fetch_albums(
    state: &NativeState,
    principal: NativePrincipal,
    params: &ListParams,
    artist_id: Option<Uuid>,
    recent: bool,
) -> Result<PagedResponse<AlbumDto>, NativeApiError> {
    let page = Page::parse(params.page, params.page_size)?;
    let order = canonical_order(
        params.order_by.as_deref(),
        if recent { "created_at" } else { "name" },
    )?;
    let direction = canonical_direction(
        params
            .order_direction
            .as_deref()
            .or(recent.then_some("desc")),
    )?;
    let restricted = has_library_bindings(&state.pool).await?;
    let search_pattern = contains_search_pattern(params.q.as_deref())?;
    let query = format!(
        "SELECT {ALBUM_COLUMNS}
         FROM releases release {ALBUM_JOINS}
         WHERE release.status = 'active'
           AND (NOT $5 OR native_user_can_access_release($6, release.id))
           AND ($1::text IS NULL OR release.normalized_title ILIKE $1 ESCAPE E'\\\\')
           AND ($2::uuid IS NULL OR EXISTS (
             SELECT 1 FROM release_artists scoped
             JOIN artists scoped_artist ON scoped_artist.id = scoped.artist_id
             WHERE scoped.release_id = release.id AND scoped_artist.public_api_id = $2))
         ORDER BY {} LIMIT $3 OFFSET $4",
        album_order_sql(order, direction),
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(search_pattern.as_deref())
        .bind(artist_id)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM releases release
         WHERE release.status='active'
           AND (NOT $3 OR native_user_can_access_release($4,release.id))
           AND ($1::text IS NULL OR release.normalized_title ILIKE $1 ESCAPE E'\\\\')
           AND ($2::uuid IS NULL OR EXISTS(
             SELECT 1 FROM release_artists scoped JOIN artists scoped_artist ON scoped_artist.id=scoped.artist_id
             WHERE scoped.release_id=release.id AND scoped_artist.public_api_id=$2))",
    )
    .bind(search_pattern.as_deref())
    .bind(artist_id)
    .bind(restricted)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    let records = rows
        .iter()
        .map(|row| album_from_row(row, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let data = album_dtos(state, principal, records).await?;
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

pub async fn list_albums(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<ListParams>,
) -> Result<Json<PagedResponse<AlbumDto>>, NativeApiError> {
    Ok(Json(
        fetch_albums(&state, principal, &params, None, false).await?,
    ))
}

pub async fn recent_albums(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<RecentParams>,
) -> Result<Json<PagedResponse<AlbumDto>>, NativeApiError> {
    let list = ListParams {
        q: None,
        page: Some(1),
        page_size: Some(params.limit.unwrap_or(50)),
        order_by: Some("createdAt".to_string()),
        order_direction: Some("desc".to_string()),
    };
    Ok(Json(
        fetch_albums(&state, principal, &list, None, true).await?,
    ))
}

pub async fn artist_albums(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
    Query(params): Query<ListParams>,
) -> Result<Json<PagedResponse<AlbumDto>>, NativeApiError> {
    Ok(Json(
        fetch_albums(&state, principal, &params, Some(id), false).await?,
    ))
}

async fn fetch_album_record(
    pool: &sqlx::PgPool,
    public_id: Uuid,
    user_id: Uuid,
) -> Result<AlbumRecord, NativeApiError> {
    let query = format!(
        "SELECT {ALBUM_COLUMNS} FROM releases release {ALBUM_JOINS}
         WHERE release.public_api_id = $1 AND release.status = 'active'
           AND native_user_can_access_release($2, release.id)"
    );
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(public_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.as_ref()
        .map(|row| album_from_row(row, 0))
        .transpose()?
        .ok_or(NativeApiError::NotFound)
}

pub async fn get_album(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<AlbumDto>, NativeApiError> {
    let record = fetch_album_record(&state.pool, id, principal.user_id).await?;
    Ok(Json(album_dto(&state, principal, record).await?))
}

async fn fetch_songs(
    state: &NativeState,
    principal: NativePrincipal,
    client_binding: &str,
    params: &SearchSongsParams,
    release_id: Option<Uuid>,
    artist_id: Option<Uuid>,
) -> Result<PagedResponse<SongDto>, NativeApiError> {
    let page = Page::parse(params.page, params.page_size)?;
    let restricted = has_library_bindings(&state.pool).await?;
    let search_pattern = contains_search_pattern(params.q.as_deref())?;
    let scoped_artist = artist_id.or(params.filter_by_artist_api_key);
    let order = song_order_sql(release_id.is_some() || scoped_artist.is_some());
    let query = format!(
        "{SONG_SELECT}
         WHERE ($1::text IS NULL OR track.normalized_title ILIKE $1 ESCAPE E'\\\\')
           AND ($2::uuid IS NULL OR release.public_api_id = $2)
           AND ($3::uuid IS NULL OR EXISTS (
             SELECT 1 FROM artist_track_associations scoped
             JOIN artists scoped_artist ON scoped_artist.id = scoped.artist_id
             WHERE scoped.track_id = track.id AND scoped_artist.public_api_id = $3))
           AND (NOT $6 OR native_user_can_access_release($7, release.id))
         ORDER BY {order} LIMIT $4 OFFSET $5"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(search_pattern.as_deref())
        .bind(release_id)
        .bind(scoped_artist)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let total = if search_pattern.is_none()
        && release_id.is_none()
        && scoped_artist.is_none()
        && !restricted
    {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(sum(aggregate.track_count),0)::bigint
         FROM release_catalog_aggregates aggregate
         JOIN releases release ON release.id=aggregate.release_id
         WHERE release.status='active'",
        )
        .fetch_one(&state.pool)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tracks track JOIN releases release ON release.id=track.release_id
             WHERE release.status='active'
               AND ($1::text IS NULL OR track.normalized_title ILIKE $1 ESCAPE E'\\\\')
               AND ($2::uuid IS NULL OR release.public_api_id=$2)
               AND ($3::uuid IS NULL OR EXISTS(
                 SELECT 1 FROM artist_track_associations scoped JOIN artists scoped_artist ON scoped_artist.id=scoped.artist_id
                 WHERE scoped.track_id=track.id AND scoped_artist.public_api_id=$3))
               AND (NOT $4 OR native_user_can_access_release($5,release.id))",
        )
        .bind(search_pattern.as_deref())
        .bind(release_id)
        .bind(scoped_artist)
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?
    };
    let records = rows
        .iter()
        .map(song_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let data = song_dtos(state, principal, client_binding, records).await?;
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

pub async fn list_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Query(params): Query<SearchSongsParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    Ok(Json(
        fetch_songs(&state, principal, &binding.0, &params, None, None).await?,
    ))
}

pub async fn recent_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Query(params): Query<RecentParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    let page = Page::parse(Some(1), params.limit)?;
    let restricted = has_library_bindings(&state.pool).await?;
    let query = format!(
        "{SONG_SELECT}
         WHERE (NOT $2 OR native_user_can_access_release($3, release.id))
         ORDER BY track.created_at DESC, track.public_api_id ASC LIMIT $1"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(i64::from(page.size))
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let total = if restricted {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tracks track JOIN releases release ON release.id=track.release_id
             WHERE release.status='active'
               AND native_user_can_access_release($1,release.id)",
        )
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(sum(aggregate.track_count),0)::bigint
             FROM release_catalog_aggregates aggregate
             JOIN releases release ON release.id=aggregate.release_id
             WHERE release.status='active'",
        )
        .fetch_one(&state.pool)
        .await?
    };
    let records = rows
        .iter()
        .map(song_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let data = song_dtos(&state, principal, &binding.0, records).await?;
    Ok(Json(PagedResponse {
        meta: page.metadata(total)?,
        data,
    }))
}

pub async fn random_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Query(params): Query<RandomSongParams>,
) -> Result<Json<RandomSongsResponse>, NativeApiError> {
    let count = params.count.unwrap_or(20);
    if !(1..=500).contains(&count)
        || params
            .from_year
            .zip(params.to_year)
            .is_some_and(|(from, to)| from > to)
    {
        return Err(NativeApiError::BadRequest);
    }
    let genre_pattern = params
        .genre
        .as_deref()
        .map(escaped_like_value)
        .transpose()?;
    // Start from a random point in an indexed, stable hash ordering. The
    // circular window keeps work fixed while allowing every Track in the
    // catalog to participate across requests; ordering by the raw UUID would
    // permanently exclude the catalog tail.
    let restricted = has_library_bindings(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    // PostgreSQL otherwise reorders the selected-ID relation behind the wide
    // compatibility joins and evaluates them for the entire catalog. This
    // transaction-local setting makes the explicit bounded join order below
    // authoritative without changing global server behavior.
    sqlx::query("SET LOCAL join_collapse_limit = 1")
        .execute(&mut *tx)
        .await?;
    let is_unfiltered = params.artist_id.is_none()
        && params.album_id.is_none()
        && params.genre.is_none()
        && params.from_year.is_none()
        && params.to_year.is_none()
        && !restricted;
    if is_unfiltered {
        let query = format!(
            "WITH selected AS MATERIALIZED (
               SELECT track.id,random() AS random_key
               FROM tracks track TABLESAMPLE SYSTEM_ROWS({RANDOM_CANDIDATE_WINDOW})
               JOIN releases release ON release.id=track.release_id
               WHERE release.status='active'
               ORDER BY random_key LIMIT $1
             )
             {SONG_FROM_SELECTED}
             ORDER BY selected.random_key"
        );
        let rows = sqlx::query(AssertSqlSafe(query))
            .bind(i64::from(count))
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        let records = rows
            .iter()
            .map(song_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let data = song_dtos(&state, principal, &binding.0, records).await?;
        return Ok(Json(RandomSongsResponse {
            count: data.len(),
            data,
        }));
    }
    let query = format!(
        "WITH pivot AS MATERIALIZED (
           SELECT hashtextextended(random()::text,0) AS random_hash
         ), hash_window AS MATERIALIZED (
           SELECT id FROM (
             (SELECT track.id, hashtextextended(track.public_api_id::text,0) AS item_hash,
                     0 AS segment
              FROM tracks track
              JOIN releases release ON release.id=track.release_id
              CROSS JOIN pivot
              WHERE hashtextextended(track.public_api_id::text,0) >= pivot.random_hash
                AND {RANDOM_ELIGIBILITY}
              ORDER BY item_hash,track.public_api_id LIMIT {RANDOM_CANDIDATE_WINDOW})
             UNION ALL
             (SELECT track.id, hashtextextended(track.public_api_id::text,0) AS item_hash,
                     1 AS segment
              FROM tracks track
              JOIN releases release ON release.id=track.release_id
              CROSS JOIN pivot
              WHERE hashtextextended(track.public_api_id::text,0) < pivot.random_hash
                AND {RANDOM_ELIGIBILITY}
              ORDER BY item_hash,track.public_api_id LIMIT {RANDOM_CANDIDATE_WINDOW})
           ) circular
           ORDER BY segment,item_hash LIMIT {RANDOM_CANDIDATE_WINDOW}
         ), selected AS MATERIALIZED (
           SELECT id,random() AS random_key FROM hash_window
           ORDER BY random_key LIMIT $6
         )
         {SONG_FROM_SELECTED}
         ORDER BY selected.random_key"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(params.artist_id)
        .bind(params.album_id)
        .bind(params.from_year)
        .bind(params.to_year)
        .bind(genre_pattern.as_deref())
        .bind(i64::from(count))
        .bind(restricted)
        .bind(principal.user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let records = rows
        .iter()
        .map(song_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let songs = song_dtos(&state, principal, &binding.0, records).await?;
    Ok(Json(RandomSongsResponse {
        count: songs.len(),
        data: songs,
    }))
}

pub async fn search_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Query(params): Query<SearchSongsParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    if params
        .q
        .as_deref()
        .is_none_or(|query| query.trim().chars().count() < 2)
    {
        let page = Page::parse(params.page, params.page_size)?;
        return Ok(Json(PagedResponse {
            meta: page.metadata(0)?,
            data: Vec::new(),
        }));
    }
    Ok(Json(
        fetch_songs(&state, principal, &binding.0, &params, None, None).await?,
    ))
}

pub async fn artist_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<Uuid>,
    Query(params): Query<SearchSongsParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    Ok(Json(
        fetch_songs(&state, principal, &binding.0, &params, None, Some(id)).await?,
    ))
}

pub async fn album_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<Uuid>,
    Query(_params): Query<SearchSongsParams>,
) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
    let album = fetch_album_record(&state.pool, id, principal.user_id).await?;
    // The legacy album controller has no paging parameters on this route. Its
    // response uses the album's stored SongCount as pageSize and returns all
    // album songs in one page.
    let params = SearchSongsParams {
        q: None,
        page: Some(1),
        page_size: Some(
            i32::try_from(album.song_count.max(1)).map_err(|_| NativeApiError::BadRequest)?,
        ),
        filter_by_artist_api_key: None,
    };
    Ok(Json(
        fetch_songs(&state, principal, &binding.0, &params, Some(id), None).await?,
    ))
}

pub async fn get_song(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<SongDto>, NativeApiError> {
    let query = format!(
        "{SONG_SELECT} WHERE track.public_api_id = $1
           AND native_user_can_access_release($2, release.id)"
    );
    let row = sqlx::query(AssertSqlSafe(query))
        .bind(id)
        .bind(principal.user_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(NativeApiError::NotFound)?;
    Ok(Json(
        song_dto(&state, principal, &binding.0, song_from_row(&row)?).await?,
    ))
}

/// Extracts plain-text lyrics from one manifest payload, preferring embedded
/// tag text and falling back to a parsed lyrics sidecar (unchanged precedence).
fn lyrics_from_manifest(manifest: &serde_json::Value) -> Option<String> {
    let files = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for file in files {
        let metadata = file.get("metadata");
        let candidate = metadata
            .and_then(|metadata| metadata.get("raw"))
            .and_then(|raw| raw.get("lyrics"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                metadata
                    .and_then(|metadata| metadata.get("lyrics"))
                    .and_then(serde_json::Value::as_str)
            });
        if let Some(value) = candidate.filter(|value| !value.is_empty()) {
            return Some(value.to_owned());
        }
    }
    let sidecars = manifest
        .get("normalization")
        .and_then(|normalization| normalization.get("parsed_sidecars"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    sidecars
        .iter()
        .filter(|sidecar| {
            sidecar
                .get("sidecar_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| matches!(kind, "txt" | "lyrics" | "lrc"))
        })
        .find_map(|sidecar| {
            let parsed = sidecar.get("parsed");
            parsed
                .and_then(|parsed| parsed.get("value"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    parsed
                        .and_then(|parsed| parsed.get("content"))
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

pub async fn song_lyrics(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<LyricsDto>, NativeApiError> {
    // ADR-0213: lyrics are large manifest evidence. This is an authorized
    // single-Track detail request, so it reads exactly one evidence object and
    // never touches a browse or listing path.
    let row = sqlx::query(
        "SELECT track.title, COALESCE(track.display_artist, primary_artist.canonical_name),
                catalog_import.manifest_row_id
         FROM tracks track
         JOIN releases release ON release.id=track.release_id AND release.status='active'
         JOIN LATERAL (
           SELECT artist.canonical_name FROM track_artists billing
           JOIN artists artist ON artist.id = billing.artist_id
           WHERE billing.track_id = track.id AND billing.role = 'primary'
           ORDER BY billing.presentation_order, billing.id LIMIT 1
         ) primary_artist ON true
         LEFT JOIN catalog_imports catalog_import ON catalog_import.release_id = track.release_id
         WHERE track.public_api_id = $1
           AND native_user_can_access_release($2, track.release_id)
         ORDER BY catalog_import.imported_at DESC NULLS LAST LIMIT 1",
    )
    .bind(id)
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::CapturedNotFound("Song not found"))?;
    let manifest_row_id: Option<i64> = row.try_get(2)?;
    let plain_text = match manifest_row_id {
        Some(manifest_row_id) => {
            let manifest =
                melodee_persistence::manifest_evidence::load_manifest(&state.pool, manifest_row_id)
                    .await
                    .map_err(|_| NativeApiError::CapturedNotFound("Lyrics not found"))?;
            lyrics_from_manifest(&manifest)
        }
        None => None,
    };
    let plain_text = plain_text.ok_or(NativeApiError::CapturedNotFound("Lyrics not found"))?;
    Ok(Json(LyricsDto {
        song_id: id,
        language: "und".to_string(),
        is_synced: false,
        plain_text: Some(plain_text),
        lines: None,
        display_artist: row.try_get(1)?,
        display_title: row.try_get(0)?,
        offset: None,
    }))
}

pub async fn search(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResultResponse>, NativeApiError> {
    if request.query.trim().is_empty() {
        return Err(NativeApiError::BadRequest);
    }
    let size = request.page_size.unwrap_or(50);
    let artist_params = ListParams {
        q: Some(request.query.clone()),
        page: Some(i32::from(request.artist_page.unwrap_or(1))),
        page_size: Some(i32::from(size)),
        // The legacy combined-search path orders its Artist result by source
        // creation when no explicit sort is supplied; the standalone artist
        // search route has a different normalized-name tie order.
        order_by: request
            .sort_by
            .clone()
            .or_else(|| Some("createdAt".to_string())),
        order_direction: request.sort_order.clone(),
    };
    let album_params = ListParams {
        q: Some(request.query.clone()),
        page: Some(i32::from(request.album_page.unwrap_or(1))),
        page_size: Some(i32::from(size)),
        order_by: request.sort_by,
        order_direction: request.sort_order,
    };
    let song_params = SearchSongsParams {
        q: Some(request.query),
        page: Some(i32::from(request.song_page.unwrap_or(1))),
        page_size: Some(i32::from(size)),
        filter_by_artist_api_key: request.filter_by_artist_id,
    };
    let artists = fetch_artists(&state, principal, &artist_params, false).await?;
    let albums = fetch_albums(&state, principal, &album_params, None, false).await?;
    let songs = fetch_songs(&state, principal, &binding.0, &song_params, None, None).await?;
    let total = artists.meta.total_count + albums.meta.total_count + songs.meta.total_count;
    let meta = PaginationMetadata::new(
        i64::from(total),
        1,
        u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?,
    )
    .map_err(|_| NativeApiError::BadRequest)?;
    Ok(Json(SearchResultResponse {
        meta,
        data: SearchResultDto {
            total_count: total,
            total_artists: artists.meta.total_count,
            artists: artists.data,
            total_albums: albums.meta.total_count,
            albums: albums.data,
            total_songs: songs.meta.total_count,
            songs: songs.data,
            playlists: Vec::new(),
            total_playlists: 0,
        },
    }))
}

pub async fn search_suggest(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<SuggestParams>,
) -> Result<Json<SearchSuggestResponse>, NativeApiError> {
    let limit = params.limit.unwrap_or(10);
    let limit = if (1..=50).contains(&limit) { limit } else { 10 };
    let Some(query) = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| value.chars().count() >= 2)
    else {
        return Ok(Json(SearchSuggestResponse {
            artists: Vec::new(),
            albums: Vec::new(),
            songs: Vec::new(),
            playlists: Vec::new(),
        }));
    };
    // The legacy SearchService applies a normalized CONTAINS filter to each
    // category before the suggestion projection.  Keep the same wildcard
    // semantics here rather than turning autocomplete into a prefix-only
    // search; e.g. `Ne` must find both `Dune` and `Sandstone`.
    let query = format!("%{}%", escaped_like_value(query)?);
    let artist_rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT artist.public_api_id, artist.canonical_name FROM artists artist
         WHERE status NOT IN ('merged', 'deprecated')
           AND (normalized_name ILIKE $1 ESCAPE E'\\\\'
             OR EXISTS (SELECT 1 FROM artist_aliases alias
                        WHERE alias.artist_id = artist.id
                          AND alias.normalized_alias ILIKE $1 ESCAPE E'\\\\'))
           AND native_user_can_access_artist($3, artist.id)
         ORDER BY canonical_name, public_api_id LIMIT $2",
    )
    .bind(&query)
    .bind(i64::from(limit))
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    let album_rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT release.public_api_id, release.title, artist.canonical_name
         FROM releases release
         JOIN LATERAL (
           SELECT artist.canonical_name FROM release_artists billing
           JOIN artists artist ON artist.id = billing.artist_id
           WHERE billing.release_id = release.id AND billing.role = 'primary'
           ORDER BY billing.presentation_order, billing.id LIMIT 1
         ) artist ON true
         WHERE release.status = 'active'
           AND release.normalized_title ILIKE $1 ESCAPE E'\\\\'
           AND native_user_can_access_release($3, release.id)
         ORDER BY release.title, release.public_api_id LIMIT $2",
    )
    .bind(&query)
    .bind(i64::from(limit))
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    let song_rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT track.public_api_id, track.title, artist.canonical_name
         FROM tracks track
         JOIN releases release ON release.id = track.release_id
         JOIN LATERAL (
           SELECT artist.canonical_name FROM release_artists billing
           JOIN artists artist ON artist.id = billing.artist_id
           WHERE billing.release_id = release.id AND billing.role = 'primary'
           ORDER BY billing.presentation_order, billing.id LIMIT 1
         ) artist ON true
         WHERE release.status='active'
           AND track.normalized_title ILIKE $1 ESCAPE E'\\\\'
           AND native_user_can_access_release($3, release.id)
         ORDER BY artist.canonical_name, release.title, track.public_api_id LIMIT $2",
    )
    .bind(&query)
    .bind(i64::from(limit))
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(SearchSuggestResponse {
        artists: artist_rows
            .into_iter()
            .map(|(id, name)| SearchSuggestionDto {
                id,
                name,
                r#type: "artist".to_string(),
                thumbnail_url: image_url(&state.public_origin, "artist", id, 80),
            })
            .collect(),
        albums: album_rows
            .into_iter()
            .map(|(id, name, artist)| SearchSuggestionDto {
                id,
                name: format!("{name} - {artist}"),
                r#type: "album".to_string(),
                thumbnail_url: image_url(&state.public_origin, "album", id, 80),
            })
            .collect(),
        songs: song_rows
            .into_iter()
            .map(|(id, title, artist)| SearchSuggestionDto {
                id,
                name: format!("{title} - {artist}"),
                r#type: "song".to_string(),
                thumbnail_url: image_url(&state.public_origin, "song", id, 80),
            })
            .collect(),
        // The frozen legacy SearchService never populated its playlist
        // collection, so this compatibility route intentionally remains
        // empty even when native playlists exist.
        playlists: Vec::new(),
    }))
}

fn validate_range<T: PartialOrd>(range: Option<&super::dto::NumericRange<T>>) -> bool {
    range
        .and_then(|value| value.min.as_ref().zip(value.max.as_ref()))
        .is_none_or(|(min, max)| min <= max)
}

pub async fn advanced_search(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Json(request): Json<AdvancedSearchRequest>,
) -> Result<Json<AdvancedSearchResponse>, NativeApiError> {
    let filters = request.filters.as_ref();
    if !validate_range(filters.and_then(|value| value.year.as_ref()))
        || !validate_range(filters.and_then(|value| value.bpm.as_ref()))
        || !validate_range(filters.and_then(|value| value.duration.as_ref()))
    {
        return Err(NativeApiError::BadRequest);
    }
    if request.sort_by.as_deref().is_some_and(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "relevance" | "date" | "popularity" | "rating"
        )
    }) || request
        .sort_order
        .as_deref()
        .is_some_and(|value| !matches!(value.to_ascii_lowercase().as_str(), "asc" | "desc"))
    {
        return Err(NativeApiError::BadRequest);
    }
    let page = Page::parse(request.page, request.limit)?;
    let default_types = vec![
        "song".to_string(),
        "album".to_string(),
        "artist".to_string(),
        "playlist".to_string(),
    ];
    let types = request.types.as_deref().unwrap_or(&default_types);
    if types.iter().any(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "song" | "album" | "artist" | "playlist"
        )
    }) {
        return Err(NativeApiError::BadRequest);
    }
    // The frozen legacy implementation validates advanced filters but sends
    // its normalized text query through the common search service. Preserve
    // that wire behavior until a captured, accepted v1 normalization exists.
    let query = request.query.unwrap_or_default();
    let list = ListParams {
        q: Some(query.clone()),
        page: Some(i32::try_from(page.number).map_err(|_| NativeApiError::BadRequest)?),
        page_size: Some(i32::from(page.size)),
        order_by: None,
        order_direction: None,
    };
    let song_params = SearchSongsParams {
        q: Some(query),
        page: Some(i32::try_from(page.number).map_err(|_| NativeApiError::BadRequest)?),
        page_size: Some(i32::from(page.size)),
        filter_by_artist_api_key: None,
    };
    let include = |kind: &str| types.iter().any(|value| value.eq_ignore_ascii_case(kind));
    let artists = if include("artist") {
        fetch_artists(&state, principal, &list, false).await?
    } else {
        PagedResponse {
            meta: page.metadata(0)?,
            data: Vec::new(),
        }
    };
    let albums = if include("album") {
        fetch_albums(&state, principal, &list, None, false).await?
    } else {
        PagedResponse {
            meta: page.metadata(0)?,
            data: Vec::new(),
        }
    };
    let songs = if include("song") {
        fetch_songs(&state, principal, &binding.0, &song_params, None, None).await?
    } else {
        PagedResponse {
            meta: page.metadata(0)?,
            data: Vec::new(),
        }
    };
    let total =
        i64::from(artists.meta.total_count + albums.meta.total_count + songs.meta.total_count);
    Ok(Json(AdvancedSearchResponse {
        results: AdvancedSearchResults {
            songs: songs.data,
            albums: albums.data,
            artists: artists.data,
            playlists: Vec::new(),
        },
        meta: page.metadata(total)?,
    }))
}

pub async fn similar(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, kind)): Path<(Uuid, String)>,
    Query(params): Query<SimilarParams>,
) -> Result<Json<SimilarResponse>, NativeApiError> {
    let limit = params.limit.unwrap_or(10);
    let limit = if (1..=100).contains(&limit) {
        limit
    } else {
        10
    };
    let normalized_kind = kind.to_ascii_lowercase();
    let rows = match normalized_kind.as_str() {
        // The frozen controller's Artist genre resolver returns no genres, so
        // a valid Artist deterministically has no similar results.
        "artist" => {
            ensure_artist_access(&state, principal.user_id, id).await?;
            Vec::new()
        }
        "album" => {
            let rows = sqlx::query_as::<_, (Uuid, String)>(
                "WITH source AS MATERIALIZED (
                   SELECT release.id FROM releases release
                   WHERE release.public_api_id=$1 AND release.status='active'
                     AND native_user_can_access_release($3,release.id)
                 ), source_genres AS MATERIALIZED (
                   SELECT DISTINCT genre.normalized_value AS genre
                   FROM source
                   JOIN catalog_genre_terms genre ON genre.release_id=source.id
                    AND genre.surface='native'
                 )
                 SELECT candidate.public_api_id,candidate.title
                 FROM releases candidate,source
                 WHERE candidate.id<>source.id AND candidate.status='active'
                   AND native_user_can_access_release($3,candidate.id)
                   AND EXISTS (
                     SELECT 1 FROM catalog_genre_terms candidate_genre
                     WHERE candidate_genre.release_id=candidate.id
                       AND candidate_genre.surface='native'
                       AND candidate_genre.normalized_value IN (
                         SELECT genre FROM source_genres))
                 ORDER BY (
                   SELECT count(*) FROM native_track_plays play
                   JOIN tracks played_track ON played_track.id=play.track_id
                   WHERE played_track.release_id=candidate.id) DESC,
                   candidate.title,candidate.public_api_id LIMIT $2",
            )
            .bind(id)
            .bind(i64::from(limit))
            .bind(principal.user_id)
            .fetch_all(&state.pool)
            .await?;
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM releases release
                   WHERE release.public_api_id=$1 AND release.status='active'
                     AND native_user_can_access_release($2,release.id))",
            )
            .bind(id)
            .bind(principal.user_id)
            .fetch_one(&state.pool)
            .await?;
            if !exists {
                return Err(NativeApiError::NotFound);
            }
            rows
        }
        "song" => {
            let rows = sqlx::query_as::<_, (Uuid, String)>(
                "WITH source AS MATERIALIZED (
                   SELECT track.id,track.release_id,feature.tempo
                   FROM tracks track
                   JOIN releases source_release
                     ON source_release.id=track.release_id AND source_release.status='active'
                   LEFT JOIN native_track_audio_features feature ON feature.track_id=track.id
                   WHERE track.public_api_id=$1
                     AND native_user_can_access_release($3,track.release_id)
                 ), source_genres AS MATERIALIZED (
                   SELECT DISTINCT genre.normalized_value AS genre
                   FROM source
                   JOIN catalog_genre_terms genre ON genre.track_id=source.id
                    AND genre.surface='native'
                 )
                 SELECT candidate.public_api_id,candidate.title
                 FROM tracks candidate
                 JOIN releases release ON release.id=candidate.release_id
                 LEFT JOIN native_track_audio_features feature ON feature.track_id=candidate.id
                 CROSS JOIN source
                 WHERE candidate.id<>source.id AND release.status='active'
                   AND native_user_can_access_release($3,candidate.release_id)
                   AND EXISTS (
                     SELECT 1 FROM catalog_genre_terms candidate_genre
                     WHERE candidate_genre.track_id=candidate.id
                       AND candidate_genre.surface='native'
                       AND candidate_genre.normalized_value IN (
                         SELECT genre FROM source_genres))
                   AND (source.tempo IS NULL OR source.tempo<=0
                     OR feature.tempo BETWEEN source.tempo-20 AND source.tempo+20)
                 ORDER BY (
                   SELECT count(*) FROM native_track_plays play
                   WHERE play.track_id=candidate.id) DESC,
                   candidate.title,candidate.public_api_id LIMIT $2",
            )
            .bind(id)
            .bind(i64::from(limit))
            .bind(principal.user_id)
            .fetch_all(&state.pool)
            .await?;
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM tracks track
                   JOIN releases release ON release.id=track.release_id
                   WHERE track.public_api_id=$1 AND release.status='active'
                     AND native_user_can_access_release($2,track.release_id))",
            )
            .bind(id)
            .bind(principal.user_id)
            .fetch_one(&state.pool)
            .await?;
            if !exists {
                return Err(NativeApiError::NotFound);
            }
            rows
        }
        _ => return Err(NativeApiError::BadRequest),
    };
    Ok(Json(SimilarResponse {
        similar: rows
            .into_iter()
            .zip(0_u32..)
            .map(|((item_id, name), index)| SimilarItemDto {
                id: item_id,
                name,
                r#type: normalized_kind.clone(),
                similarity_score: 1.0 - f64::from(index) * 0.05,
                image_url: Some(format!(
                    "{}/images/{item_id}/80",
                    state.public_origin.trim_end_matches('/')
                )),
            })
            .collect(),
    }))
}

fn genre_id(name: &str) -> String {
    // Captured IDs are URL-safe base64 without padding. Keeping this helper
    // local avoids introducing a new runtime dependency for one wire detail.
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = name.as_bytes();
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        result.push(char::from(TABLE[usize::from(a >> 2)]));
        result.push(char::from(TABLE[usize::from(((a & 0x03) << 4) | (b >> 4))]));
        if chunk.len() > 1 {
            result.push(char::from(TABLE[usize::from(((b & 0x0f) << 2) | (c >> 6))]));
        }
        if chunk.len() > 2 {
            result.push(char::from(TABLE[usize::from(c & 0x3f)]));
        }
    }
    result
}

async fn resolve_genre_name(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    id: &str,
) -> Result<String, NativeApiError> {
    if id.is_empty() || id.len() > 1_000 {
        return Err(NativeApiError::BadRequest);
    }
    sqlx::query_scalar(
        "SELECT DISTINCT genre.display_value AS name
         FROM catalog_genre_terms genre
         JOIN tracks track ON track.id=genre.track_id
         JOIN releases release ON release.id=track.release_id
         WHERE genre.surface='native'
           AND release.status='active'
           AND native_user_can_access_release($2, track.release_id)
           AND translate(rtrim(encode(convert_to(genre.display_value, 'UTF8'), 'base64'), '='), '+/', '-_') = $1
         ORDER BY name LIMIT 1",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(NativeApiError::NotFound)
}

pub async fn genre_songs(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Path(id): Path<String>,
    Query(params): Query<UserListParams>,
) -> Result<Json<GenreSongsResponse>, NativeApiError> {
    let genre = resolve_genre_name(&state.pool, principal.user_id, &id).await?;
    let page = user_page(&params)?;
    let query = format!(
        "SELECT song.*, count(*) OVER() FROM ({SONG_SELECT}
         WHERE EXISTS (
           SELECT 1 FROM catalog_genre_terms genre
           WHERE genre.track_id=track.id AND genre.surface='native'
             AND genre.display_value ILIKE $1)
           AND native_user_can_access_release($4, release.id)
         ORDER BY track.sort_title, track.public_api_id LIMIT $2 OFFSET $3) song"
    );
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(&genre)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .bind(principal.user_id)
        .fetch_all(&state.pool)
        .await?;
    let total = rows
        .first()
        .map_or(Ok(0), |row| row.try_get::<i64, _>(30))?;
    let mut songs = Vec::with_capacity(rows.len());
    for row in rows {
        songs.push(song_dto(&state, principal, &binding.0, song_from_row(&row)?).await?);
    }
    Ok(Json(GenreSongsResponse {
        songs,
        total_count: bounded_i32(total)?,
        page: i32::try_from(page.number).map_err(|_| NativeApiError::BadRequest)?,
        limit: i32::from(page.size),
    }))
}

async fn mutate_genre(
    state: &NativeState,
    principal: NativePrincipal,
    id: &str,
    mutation: NativeUserStateMutation,
) -> Result<(), NativeApiError> {
    let name = resolve_genre_name(&state.pool, principal.user_id, id).await?;
    NativeUserStateStore::mutate_genre(
        &state.pool,
        principal.user_id,
        &name,
        mutation,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

pub async fn star_genre(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(String, bool)>,
) -> Result<(), NativeApiError> {
    mutate_genre(
        &state,
        principal,
        &id,
        NativeUserStateMutation::Starred(value),
    )
    .await
}

pub async fn hate_genre(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Path((id, value)): Path<(String, bool)>,
) -> Result<(), NativeApiError> {
    mutate_genre(
        &state,
        principal,
        &id,
        NativeUserStateMutation::Hated(value),
    )
    .await
}

pub async fn starred_genres(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
) -> Result<Json<Vec<String>>, NativeApiError> {
    Ok(Json(
        accessible_genre_preferences(&state.pool, principal.user_id, "starred").await?,
    ))
}

pub async fn hated_genres(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
) -> Result<Json<Vec<String>>, NativeApiError> {
    Ok(Json(
        accessible_genre_preferences(&state.pool, principal.user_id, "hated").await?,
    ))
}

async fn accessible_genre_preferences(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    column: &'static str,
) -> Result<Vec<String>, NativeApiError> {
    let query = AssertSqlSafe(format!(
        "SELECT state.genre_name FROM native_genre_user_states state
         WHERE state.user_id = $1 AND state.{column}
           AND EXISTS (
             SELECT 1 FROM catalog_genre_terms genre
             JOIN tracks track ON track.id=genre.track_id
             JOIN releases release ON release.id=track.release_id
             WHERE genre.surface='native'
               AND release.status='active'
               AND genre.normalized_value = state.normalized_genre_name
               AND native_user_can_access_release($1, track.release_id)
           )
         ORDER BY state.normalized_genre_name"
    ));
    Ok(sqlx::query_scalar(query)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
}

pub async fn list_genres(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<GenreParams>,
) -> Result<Json<GenreListResponse>, NativeApiError> {
    let page = Page::parse(params.page, params.limit)?;
    let direction = canonical_direction(params.order_direction.as_deref())?;
    if params
        .order_by
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("name"))
    {
        return Err(NativeApiError::BadRequest);
    }
    let restricted = has_library_bindings(&state.pool).await?;
    let rows = sqlx::query(
        "WITH grouped AS (
           SELECT aggregate.display_value AS name, aggregate.song_count,
                  aggregate.release_count AS album_count
           FROM catalog_genre_aggregates aggregate
           WHERE NOT $5 AND aggregate.surface='native'
           UNION ALL
           SELECT genre.display_value AS name, count(*)::bigint AS song_count,
                  count(DISTINCT genre.release_id)::bigint AS album_count
           FROM catalog_genre_terms genre
           JOIN releases release ON release.id=genre.release_id
           WHERE $5 AND genre.surface='native' AND release.status='active'
             AND native_user_can_access_release($4, genre.release_id)
           GROUP BY genre.display_value
         )
         SELECT name, song_count, album_count, count(*) OVER()
         FROM grouped
         ORDER BY CASE WHEN $3 = 'asc' THEN name END ASC,
                  CASE WHEN $3 = 'desc' THEN name END DESC, name ASC
         LIMIT $1 OFFSET $2",
    )
    .bind(i64::from(page.size))
    .bind(page.offset)
    .bind(direction)
    .bind(principal.user_id)
    .bind(restricted)
    .fetch_all(&state.pool)
    .await?;
    let total = rows.first().map_or(Ok(0), |row| row.try_get::<i64, _>(3))?;
    let mut genres = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get(0)?;
        genres.push(GenreDto {
            id: genre_id(&name),
            name,
            song_count: bounded_i32(row.try_get(1)?)?,
            album_count: bounded_i32(row.try_get(2)?)?,
        });
    }
    Ok(Json(GenreListResponse {
        genres,
        total_count: bounded_i32(total)?,
        page: i32::try_from(page.number).map_err(|_| NativeApiError::BadRequest)?,
        limit: i32::from(page.size),
    }))
}

#[derive(Debug, Clone, Copy)]
enum UserListMode {
    Liked,
    Disliked,
    Rated,
    TopRated,
    Recent,
}

impl UserListMode {
    const fn predicate(self) -> &'static str {
        match self {
            Self::Liked => "state.starred",
            Self::Disliked => "state.hated",
            Self::Rated | Self::TopRated => "state.rating > 0",
            Self::Recent => "state.last_played_at IS NOT NULL",
        }
    }

    const fn order(self) -> &'static str {
        match self {
            Self::Liked | Self::Disliked => "state.updated_at DESC",
            Self::Rated | Self::TopRated => "state.rating DESC, state.updated_at DESC",
            Self::Recent => "state.last_played_at DESC",
        }
    }
}

fn user_page(params: &UserListParams) -> Result<Page, NativeApiError> {
    Page::parse(params.page, params.limit.or(params.page_size))
}

fn user_song_order_sql(mode: UserListMode) -> &'static str {
    match mode {
        UserListMode::Recent => {
            "state.last_played_at DESC, track.created_at ASC, track.public_api_id"
        }
        _ => mode.order(),
    }
}

async fn fetch_user_artists(
    state: &NativeState,
    principal: NativePrincipal,
    params: &UserListParams,
    mode: UserListMode,
) -> Result<PagedResponse<ArtistDto>, NativeApiError> {
    let page = user_page(params)?;
    let query = if matches!(mode, UserListMode::Recent) {
        AssertSqlSafe(format!(
            "SELECT {ARTIST_COLUMNS}, count(*) OVER()
             FROM (
               SELECT association.artist_id, max(state.last_played_at) AS last_played_at
               FROM native_track_user_states state
               JOIN tracks track ON track.id = state.track_id
               JOIN releases release ON release.id = track.release_id
               JOIN artist_track_associations association ON association.track_id = track.id
               WHERE state.user_id = $1 AND state.last_played_at IS NOT NULL
                 AND release.status = 'active'
                 AND native_user_can_access_release($1, release.id)
               GROUP BY association.artist_id
             ) recent
             JOIN artists artist ON artist.id = recent.artist_id
             LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
             WHERE artist.status NOT IN ('merged', 'deprecated')
               AND native_user_can_access_artist($1, artist.id)
             ORDER BY recent.last_played_at DESC, artist.created_at ASC, artist.public_api_id
             LIMIT $2 OFFSET $3"
        ))
    } else {
        AssertSqlSafe(format!(
            "SELECT {ARTIST_COLUMNS}, count(*) OVER()
             FROM native_artist_user_states state
             JOIN artists artist ON artist.id = state.artist_id
             LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
             WHERE state.user_id = $1 AND {} AND artist.status NOT IN ('merged', 'deprecated')
               AND native_user_can_access_artist($1, artist.id)
             ORDER BY {}, artist.public_api_id LIMIT $2 OFFSET $3",
            mode.predicate(),
            mode.order()
        ))
    };
    let rows = sqlx::query(query)
        .bind(principal.user_id)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .fetch_all(&state.pool)
        .await?;
    let total = rows.first().map_or(Ok(0), |row| row.try_get::<i64, _>(7))?;
    let mut data = Vec::with_capacity(rows.len());
    for row in rows {
        data.push(artist_dto(state, principal, artist_from_row(&row, 0)?).await?);
    }
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

async fn fetch_user_albums(
    state: &NativeState,
    principal: NativePrincipal,
    params: &UserListParams,
    mode: UserListMode,
) -> Result<PagedResponse<AlbumDto>, NativeApiError> {
    let page = user_page(params)?;
    let query = AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS}, count(*) OVER()
         FROM native_release_user_states state
         JOIN releases release ON release.id = state.release_id
         {ALBUM_JOINS}
         WHERE state.user_id = $1 AND {} AND release.status = 'active'
           AND native_user_can_access_release($1, release.id)
         ORDER BY {}, release.public_api_id LIMIT $2 OFFSET $3",
        mode.predicate(),
        mode.order()
    ));
    let rows = sqlx::query(query)
        .bind(principal.user_id)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .fetch_all(&state.pool)
        .await?;
    let total = rows
        .first()
        .map_or(Ok(0), |row| row.try_get::<i64, _>(15))?;
    let mut data = Vec::with_capacity(rows.len());
    for row in rows {
        data.push(album_dto(state, principal, album_from_row(&row, 0)?).await?);
    }
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

async fn fetch_user_songs(
    state: &NativeState,
    principal: NativePrincipal,
    binding: &str,
    params: &UserListParams,
    mode: UserListMode,
) -> Result<PagedResponse<SongDto>, NativeApiError> {
    let page = user_page(params)?;
    let query = AssertSqlSafe(format!(
        "SELECT song.*, count(*) OVER() FROM ({SONG_SELECT}
         JOIN native_track_user_states state ON state.track_id = track.id
         WHERE state.user_id = $1 AND {} AND release.status='active'
           AND native_user_can_access_release($1, release.id)
         ORDER BY {}, track.public_api_id LIMIT $2 OFFSET $3) song",
        mode.predicate(),
        user_song_order_sql(mode)
    ));
    let rows = sqlx::query(query)
        .bind(principal.user_id)
        .bind(i64::from(page.size))
        .bind(page.offset)
        .fetch_all(&state.pool)
        .await?;
    let total = rows
        .first()
        .map_or(Ok(0), |row| row.try_get::<i64, _>(30))?;
    let mut data = Vec::with_capacity(rows.len());
    for row in rows {
        data.push(song_dto(state, principal, binding, song_from_row(&row)?).await?);
    }
    Ok(PagedResponse {
        meta: page.metadata(total)?,
        data,
    })
}

macro_rules! artist_user_handler {
    ($name:ident, $mode:expr) => {
        pub async fn $name(
            State(state): State<NativeState>,
            Authenticated(principal, _binding): Authenticated,
            Query(params): Query<UserListParams>,
        ) -> Result<Json<PagedResponse<ArtistDto>>, NativeApiError> {
            Ok(Json(
                fetch_user_artists(&state, principal, &params, $mode).await?,
            ))
        }
    };
}

artist_user_handler!(user_artists_liked, UserListMode::Liked);
artist_user_handler!(user_artists_disliked, UserListMode::Disliked);
artist_user_handler!(user_artists_rated, UserListMode::Rated);
artist_user_handler!(user_artists_top_rated, UserListMode::TopRated);
artist_user_handler!(user_artists_recent, UserListMode::Recent);

macro_rules! album_user_handler {
    ($name:ident, $mode:expr) => {
        pub async fn $name(
            State(state): State<NativeState>,
            Authenticated(principal, _binding): Authenticated,
            Query(params): Query<UserListParams>,
        ) -> Result<Json<PagedResponse<AlbumDto>>, NativeApiError> {
            Ok(Json(
                fetch_user_albums(&state, principal, &params, $mode).await?,
            ))
        }
    };
}

album_user_handler!(user_albums_liked, UserListMode::Liked);
album_user_handler!(user_albums_disliked, UserListMode::Disliked);
album_user_handler!(user_albums_rated, UserListMode::Rated);
album_user_handler!(user_albums_top_rated, UserListMode::TopRated);
album_user_handler!(user_albums_recent, UserListMode::Recent);

macro_rules! song_user_handler {
    ($name:ident, $mode:expr) => {
        pub async fn $name(
            State(state): State<NativeState>,
            Authenticated(principal, binding): Authenticated,
            Query(params): Query<UserListParams>,
        ) -> Result<Json<PagedResponse<SongDto>>, NativeApiError> {
            Ok(Json(
                fetch_user_songs(&state, principal, &binding.0, &params, $mode).await?,
            ))
        }
    };
}

song_user_handler!(user_songs_liked, UserListMode::Liked);
song_user_handler!(user_songs_disliked, UserListMode::Disliked);
song_user_handler!(user_songs_rated, UserListMode::Rated);
song_user_handler!(user_songs_top_rated, UserListMode::TopRated);
song_user_handler!(user_songs_recent, UserListMode::Recent);

/// One last-played entry: the play event plus the Song it played (ADR-0188).
///
/// Distinct from the analytics `HistoryItemDto` in `dto`, which is a compact
/// `{songId, name, playedAt}` projection for the listening-history endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastPlayedItemDto {
    /// Play-event identity, so a Track played twice appears twice.
    pub id: Uuid,
    pub song: SongDto,
    pub played_at: String,
    pub duration: i32,
}

/// Last-played history feed.
///
/// The frozen envelope reported a bounded row count with an empty `data` array
/// because the legacy query did not eager-load the Song navigation for the rows it
/// counted. Serving that literally makes a real history look empty, and the
/// reference client renders a history feed (event id, Song, played-at, duration)
/// from this route, so the route now answers with the plays it counted.
pub async fn user_last_played(
    State(state): State<NativeState>,
    Authenticated(principal, binding): Authenticated,
    Query(params): Query<UserListParams>,
) -> Result<Json<PagedResponse<LastPlayedItemDto>>, NativeApiError> {
    let page = Page::parse(params.page, params.page_size.or(Some(3)))?;
    let rows = sqlx::query_as::<_, (Uuid, Uuid, OffsetDateTime, Option<i32>, i64)>(
        "SELECT play.id, track.public_api_id, play.played_at, play.duration_seconds,
                count(*) OVER()
           FROM native_track_plays play
           JOIN tracks track ON track.id = play.track_id
           JOIN releases release ON release.id = track.release_id
          WHERE play.user_id = $1 AND release.status = 'active'
            AND native_user_can_access_release($1, release.id)
          ORDER BY play.played_at DESC, play.id DESC
          LIMIT $2 OFFSET $3",
    )
    .bind(principal.user_id)
    .bind(i64::from(page.size))
    .bind(page.offset)
    .fetch_all(&state.pool)
    .await?;
    let total = rows.first().map_or(0, |row| row.4);
    let public_ids = rows.iter().map(|row| row.1).collect::<Vec<_>>();
    let songs = song_dtos_by_public_ids(&state, principal, &binding.0, &public_ids).await?;
    let mut by_id = songs
        .into_iter()
        .map(|song| (song.id, song))
        .collect::<HashMap<_, _>>();
    let mut data = Vec::with_capacity(rows.len());
    for (play_id, public_id, played_at, duration, _) in rows {
        let Some(song) = by_id.remove(&public_id) else {
            continue;
        };
        data.push(LastPlayedItemDto {
            id: play_id,
            song,
            played_at: played_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            duration: duration.unwrap_or(0),
        });
    }
    Ok(Json(PagedResponse {
        meta: page.metadata(total)?,
        data,
    }))
}

fn stats_days(days: Option<i32>) -> i32 {
    days.unwrap_or(30).clamp(1, 365)
}

fn stats_limit(limit: Option<i32>, default: i32, max: i32) -> i32 {
    limit.unwrap_or(default).clamp(1, max)
}

pub async fn user_stats(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<StatsParams>,
) -> Result<Json<UserStatsResponse>, NativeApiError> {
    let days = stats_days(params.days);
    let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64)>(
        "SELECT
           (SELECT count(*) FROM native_track_plays play JOIN tracks track ON track.id = play.track_id
             JOIN releases release ON release.id=track.release_id
             WHERE play.user_id = $1 AND play.played_at >= clock_timestamp() - ($2 * interval '1 day')
               AND release.status='active'
               AND native_user_can_access_release($1, track.release_id)),
           (SELECT count(*) FROM native_track_user_states state JOIN tracks track ON track.id = state.track_id
             JOIN releases release ON release.id=track.release_id
             WHERE state.user_id = $1 AND state.starred
               AND release.status='active'
               AND native_user_can_access_release($1, track.release_id)),
           (SELECT count(*) FROM native_release_user_states state
             JOIN releases release ON release.id=state.release_id
             WHERE state.user_id = $1 AND state.starred
               AND release.status='active'
               AND native_user_can_access_release($1, state.release_id)),
           (SELECT count(*) FROM native_artist_user_states state
             WHERE state.user_id = $1 AND state.starred
               AND native_user_can_access_artist($1, state.artist_id)),
           (SELECT count(*) FROM native_track_user_states state JOIN tracks track ON track.id = state.track_id
             JOIN releases release ON release.id=track.release_id
             WHERE state.user_id = $1 AND state.rating > 0
               AND release.status='active'
               AND native_user_can_access_release($1, track.release_id))",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(UserStatsResponse {
        period_days: days,
        total_plays: bounded_i32(row.0)?,
        favorite_songs: bounded_i32(row.1)?,
        favorite_albums: bounded_i32(row.2)?,
        favorite_artists: bounded_i32(row.3)?,
        rated_songs: bounded_i32(row.4)?,
    }))
}

pub async fn user_top_songs(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<StatsParams>,
) -> Result<Json<TopItemsResponse>, NativeApiError> {
    let days = stats_days(params.days);
    let limit = stats_limit(params.limit, 10, 100);
    let rows = sqlx::query_as::<_, (String, i64, Uuid)>(
        "SELECT track.title, count(*) AS play_count, track.public_api_id
         FROM native_track_plays play
         JOIN tracks track ON track.id = play.track_id
         JOIN releases release ON release.id=track.release_id
         WHERE play.user_id = $1
           AND play.played_at >= clock_timestamp() - ($2 * interval '1 day')
           AND release.status='active'
           AND native_user_can_access_release($1, track.release_id)
         GROUP BY track.id, track.title, track.public_api_id
         ORDER BY play_count DESC, track.sort_title, track.public_api_id LIMIT $3",
    )
    .bind(principal.user_id)
    .bind(days)
    .bind(i64::from(limit))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(TopItemsResponse {
        period_days: days,
        data: rows
            .into_iter()
            .map(|(name, play_count, song_id)| {
                Ok(TopItemResponse {
                    name,
                    play_count: bounded_i32(play_count)?,
                    song_id: Some(song_id),
                })
            })
            .collect::<Result<_, NativeApiError>>()?,
    }))
}

pub async fn user_top_genres(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<StatsParams>,
) -> Result<Json<TopItemsResponse>, NativeApiError> {
    let days = stats_days(params.days);
    let limit = stats_limit(params.limit, 10, 100);
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT genre.display_value, count(*) AS play_count
         FROM native_track_plays play
         JOIN tracks track ON track.id = play.track_id
         JOIN releases release ON release.id=track.release_id
         JOIN catalog_genre_terms genre ON genre.track_id=track.id
           AND genre.surface='native'
         WHERE play.user_id = $1
           AND play.played_at >= clock_timestamp() - ($2 * interval '1 day')
           AND release.status='active'
           AND native_user_can_access_release($1, track.release_id)
         GROUP BY genre.display_value
         ORDER BY play_count DESC, genre.display_value LIMIT $3",
    )
    .bind(principal.user_id)
    .bind(days)
    .bind(i64::from(limit))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(TopItemsResponse {
        period_days: days,
        data: rows
            .into_iter()
            .map(|(name, play_count)| {
                Ok(TopItemResponse {
                    name,
                    play_count: bounded_i32(play_count)?,
                    song_id: None,
                })
            })
            .collect::<Result<_, NativeApiError>>()?,
    }))
}

pub async fn user_plays_per_day(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<StatsParams>,
) -> Result<Json<TimeSeriesResponse>, NativeApiError> {
    let days = stats_days(params.days);
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT to_char(played_at AT TIME ZONE 'UTC', 'YYYY-MM-DD'), count(*)
         FROM native_track_plays play
         JOIN tracks track ON track.id = play.track_id
         JOIN releases release ON release.id=track.release_id
         WHERE play.user_id = $1 AND play.played_at >= clock_timestamp() - ($2 * interval '1 day')
           AND release.status='active'
           AND native_user_can_access_release($1, track.release_id)
         GROUP BY 1 ORDER BY 1",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?;
    let mut counts = HashMap::with_capacity(rows.len());
    for (date, plays) in rows {
        counts.insert(date, plays);
    }
    Ok(Json(TimeSeriesResponse {
        period_days: days,
        data: zero_fill_daily_series(days, counts)?,
    }))
}

/// The legacy chart includes both endpoints of its date interval, including
/// zero-play days. Keep the query fact-derived, then fill the presentation
/// series at the API boundary so sparse storage does not change its shape.
fn zero_fill_daily_series(
    period_days: i32,
    counts: HashMap<String, i64>,
) -> Result<Vec<TimeSeriesDataPointDto>, NativeApiError> {
    let end = OffsetDateTime::now_utc().date();
    let start = end - Duration::days(i64::from(period_days));
    (0..=period_days)
        .map(|offset| {
            let date = start + Duration::days(i64::from(offset));
            let date_key = date.to_string();
            Ok(TimeSeriesDataPointDto {
                plays: bounded_i32(counts.get(&date_key).copied().unwrap_or(0))?,
                date: format!(
                    "{}, {} {}, {}",
                    date.weekday(),
                    date.month(),
                    date.day(),
                    date.year()
                ),
            })
        })
        .collect()
}

pub async fn user_history(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    Query(params): Query<StatsParams>,
) -> Result<Json<HistoryResponse>, NativeApiError> {
    let limit = stats_limit(params.limit, 50, 500);
    let rows = sqlx::query_as::<_, (Uuid, String, OffsetDateTime)>(
        "SELECT track.public_api_id, track.title, play.played_at
         FROM native_track_plays play
         JOIN tracks track ON track.id = play.track_id
         JOIN releases release ON release.id=track.release_id
         WHERE play.user_id = $1
           AND release.status='active'
           AND native_user_can_access_release($1, track.release_id)
         ORDER BY play.played_at DESC, play.id DESC LIMIT $2",
    )
    .bind(principal.user_id)
    .bind(i64::from(limit))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(HistoryResponse {
        data: rows
            .into_iter()
            .map(|(song_id, name, played_at)| HistoryItemDto {
                song_id: Some(song_id),
                name,
                played_at: timestamp(played_at),
            })
            .collect(),
    }))
}

pub async fn admin_users(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
) -> Result<Json<Vec<AdminUserInfoDto>>, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::Forbidden);
    }
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            bool,
            bool,
            bool,
            bool,
            OffsetDateTime,
            Option<OffsetDateTime>,
        ),
    >(
        "SELECT native_user.public_api_id, native_user.username, native_user.email,
                native_user.is_admin, native_user.enabled, native_user.locked,
                native_user.blacklisted, native_user.created_at,
                max(family.session_started_at)
         FROM native_users native_user
         LEFT JOIN native_refresh_families family ON family.user_id = native_user.id
         GROUP BY native_user.id, native_user.public_api_id, native_user.username,
                  native_user.email, native_user.is_admin, native_user.enabled,
                  native_user.locked, native_user.blacklisted, native_user.created_at
         ORDER BY lower(native_user.username), native_user.public_api_id LIMIT 1000",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, username, email, is_admin, enabled, locked, blacklisted, created, login)| {
                    Ok(AdminUserInfoDto {
                        id,
                        username,
                        email: Some(email),
                        is_admin,
                        is_enabled: enabled && !locked && !blacklisted,
                        created_at: timestamp_iso(created)?,
                        last_login_at: login.map(timestamp_iso).transpose()?,
                    })
                },
            )
            .collect::<Result<Vec<_>, NativeApiError>>()?,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn query_bounds_reject_zero_negative_and_unbounded_pages() {
        assert!(Page::parse(Some(0), Some(50)).is_err());
        assert!(Page::parse(Some(1), Some(0)).is_err());
        assert!(Page::parse(Some(1), Some(501)).is_err());
        let page = Page::parse(Some(2), Some(5)).expect("valid page");
        assert_eq!(page.offset, 5);
    }

    #[test]
    fn genre_ids_match_the_frozen_url_safe_base64_contract() {
        assert_eq!(genre_id("Rock"), "Um9jaw");
        assert_eq!(genre_id("Post-Rock"), "UG9zdC1Sb2Nr");
        assert_eq!(genre_id("環境音楽"), "55Kw5aKD6Z-z5qW9");
    }

    #[test]
    fn native_wire_adapter_preserves_legacy_display_conventions() {
        assert_eq!(LEGACY_NATIVE_DISPLAY_BITRATE_KBPS, 320);
        assert_eq!(
            imported_timestamp(
                OffsetDateTime::from_unix_timestamp(0).expect("epoch"),
                OffsetDateTime::from_unix_timestamp(0).expect("epoch"),
            ),
            LEGACY_UNKNOWN_UPDATED_AT
        );
        assert_eq!(
            imported_timestamp(
                OffsetDateTime::from_unix_timestamp(0).expect("epoch"),
                OffsetDateTime::from_unix_timestamp(1).expect("epoch"),
            ),
            "1970-01-01 00:00:01"
        );
    }

    #[test]
    fn scoped_song_lists_preserve_import_order() {
        assert_eq!(
            song_order_sql(true),
            "track.created_at ASC, track.public_api_id ASC"
        );
        assert_eq!(
            song_order_sql(false),
            "track.sort_title ASC, track.public_api_id ASC"
        );
    }

    #[test]
    fn recent_user_song_lists_break_play_timestamp_ties_by_import_order() {
        assert_eq!(
            user_song_order_sql(UserListMode::Recent),
            "state.last_played_at DESC, track.created_at ASC, track.public_api_id"
        );
        assert_eq!(
            user_song_order_sql(UserListMode::Liked),
            "state.updated_at DESC"
        );
    }

    #[test]
    fn daily_play_series_includes_zero_play_days_and_both_endpoints() {
        let series = zero_fill_daily_series(
            2,
            HashMap::from([(OffsetDateTime::now_utc().date().to_string(), 3)]),
        )
        .expect("bounded test counts");
        assert_eq!(series.len(), 3);
        assert_eq!(series[0].plays, 0);
        assert_eq!(series[1].plays, 0);
        assert_eq!(series[2].plays, 3);
        assert!(series[2].date.contains(", "));
    }
}
