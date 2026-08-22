//! Native-v1 podcast subscriptions, playback state, OPML, and discovery.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use quick_xml::{Reader, XmlVersion, events::Event};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;
use uuid::Uuid;

use super::{Authenticated, NativePrincipal, NativeState, error::NativeApiError};

const MAX_PAGE_SIZE: i32 = 500;
const MAX_HISTORY_SIZE: i32 = 500;
const MAX_DISCOVERY_SIZE: i32 = 100;
const MAX_OPML_BYTES: usize = 2 * 1024 * 1024;
const MAX_DISCOVERY_BYTES: usize = 1024 * 1024;
const MAX_DISCOVERY_CONCURRENCY: usize = 8;

/// Bounded HTTPS client for the captured native-v1 iTunes discovery surface.
#[derive(Clone)]
pub struct PodcastDiscoveryClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    gauge: Option<std::sync::Arc<crate::resource_bounds::ResourceGauge>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PodcastDiscoveryConfigError {
    #[error("invalid podcast discovery endpoint")]
    InvalidEndpoint,
    #[error("podcast discovery endpoint must use HTTPS or loopback HTTP")]
    InsecureEndpoint,
    #[error("failed to construct podcast discovery HTTP client")]
    Client(#[source] reqwest::Error),
}

impl PodcastDiscoveryClient {
    pub fn new(endpoint: &str) -> Result<Self, PodcastDiscoveryConfigError> {
        Self::build(endpoint, false)
    }

    #[doc(hidden)]
    pub fn new_for_loopback_fixture(endpoint: &str) -> Result<Self, PodcastDiscoveryConfigError> {
        Self::build(endpoint, true)
    }

    fn build(
        endpoint: &str,
        allow_loopback_http: bool,
    ) -> Result<Self, PodcastDiscoveryConfigError> {
        let mut endpoint = reqwest::Url::parse(endpoint)
            .map_err(|_| PodcastDiscoveryConfigError::InvalidEndpoint)?;
        let is_loopback = endpoint.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "::1"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if endpoint.scheme() != "https"
            && !(allow_loopback_http && endpoint.scheme() == "http" && is_loopback)
        {
            return Err(PodcastDiscoveryConfigError::InsecureEndpoint);
        }
        if !endpoint.path().ends_with('/') {
            let normalized = format!("{}/", endpoint.path());
            endpoint.set_path(&normalized);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melodee/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(PodcastDiscoveryConfigError::Client)?;
        Ok(Self {
            client,
            endpoint,
            permits: std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_DISCOVERY_CONCURRENCY)),
            // the iTunes-discovery class reports utilization and
            // wait-timeouts through the process resource-bounds registry when
            // startup installed one.
            gauge: crate::resource_bounds::ResourceBounds::global().map(|registry| {
                registry.register("podcast.discovery.http", MAX_DISCOVERY_CONCURRENCY)
            }),
        })
    }

    pub fn itunes() -> Result<Self, PodcastDiscoveryConfigError> {
        Self::new("https://itunes.apple.com/")
    }

    async fn request(
        &self,
        resource: &str,
        query: &[(&str, String)],
    ) -> Result<ItunesResponse, NativeApiError> {
        if let Some(gauge) = &self.gauge {
            gauge.begin_wait();
        }
        let acquisition =
            tokio::time::timeout(std::time::Duration::from_secs(8), self.permits.acquire()).await;
        let _permit = match acquisition {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => {
                if let Some(gauge) = &self.gauge {
                    gauge.wait_failed();
                }
                return Err(NativeApiError::CapturedInternal(
                    "An unexpected error occurred",
                ));
            }
        };
        let _guard = self
            .gauge
            .as_ref()
            .map(crate::resource_bounds::ResourceGauge::admit);
        let url = self
            .endpoint
            .join(resource)
            .map_err(|_| NativeApiError::CapturedInternal("An unexpected error occurred"))?;
        let response = self
            .client
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|_| NativeApiError::CapturedInternal("An unexpected error occurred"))?;
        if !response.status().is_success() {
            return Err(NativeApiError::CapturedInternal(
                "An unexpected error occurred",
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_DISCOVERY_BYTES as u64)
        {
            return Err(NativeApiError::CapturedInternal(
                "An unexpected error occurred",
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| NativeApiError::CapturedInternal("An unexpected error occurred"))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_DISCOVERY_BYTES {
                return Err(NativeApiError::CapturedInternal(
                    "An unexpected error occurred",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| NativeApiError::CapturedInternal("An unexpected error occurred"))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelParams {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
    pub order_by: Option<String>,
    pub order_direction: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateChannel {
    pub url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannel {
    pub auto_download_enabled: Option<bool>,
    pub refresh_interval_hours: Option<i32>,
    pub max_downloaded_episodes: Option<i32>,
    pub max_storage_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteChannelParams {
    pub soft_delete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PodcastChannel {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub image_url: String,
    pub feed_url: String,
    pub website: String,
    pub last_sync_at: Option<String>,
    pub created_at: String,
    pub tags: String,
    pub user_starred: bool,
    pub user_rating: i32,
    pub site_url: Option<String>,
    pub episode_count: i32,
    pub last_played_at: Option<String>,
    pub played_count: i32,
    pub unplayed_downloaded_count: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPage {
    pub current_page: i32,
    pub total_count: i32,
    pub total_pages: i32,
    pub additional_client_data: Option<serde_json::Value>,
    pub errors: Option<Vec<String>>,
    pub data: Vec<PodcastChannel>,
    pub is_success: bool,
    pub r#type: i32,
    pub messages: Option<Vec<String>>,
    pub operation_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePageParams {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PodcastEpisode {
    pub id: i64,
    pub api_key: Uuid,
    pub title: String,
    pub description: String,
    pub publish_date: Option<String>,
    pub duration_ms: Option<f64>,
    pub duration_formatted: String,
    pub channel_title: String,
    pub channel_api_key: Uuid,
    pub is_downloaded: bool,
    pub created_at: String,
    pub tags: String,
    pub user_starred: bool,
    pub user_rating: i32,
    pub download_status: String,
    pub download_error: Option<String>,
    pub enclosure_url: Option<String>,
    pub last_played_at: Option<String>,
    pub played_count: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePage {
    pub current_page: i32,
    pub total_count: i32,
    pub total_pages: i32,
    pub additional_client_data: Option<serde_json::Value>,
    pub errors: Option<Vec<String>>,
    pub data: Vec<PodcastEpisode>,
    pub is_success: bool,
    pub r#type: i32,
    pub messages: Option<Vec<String>>,
    pub operation_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEpisodeParams {
    pub query: String,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingParams {
    pub seconds_played: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveBookmark {
    pub position_seconds: i32,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PodcastBookmark {
    pub id: i64,
    pub podcast_episode_id: i64,
    pub position_seconds: i32,
    pub comment: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PodcastHistory {
    pub id: i64,
    pub podcast_episode_id: i64,
    pub played_at: String,
    pub client: String,
    pub by_user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub seconds_played: Option<i32>,
    pub source: i16,
    pub is_now_playing: bool,
    pub last_heartbeat_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpmlImportResult {
    pub imported: i32,
    pub skipped: i32,
    pub failed: i32,
    pub imported_feeds: Vec<String>,
    pub duplicates: Vec<String>,
    pub errors: Vec<String>,
    pub total_processed: i32,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryParams {
    pub query: Option<String>,
    pub limit: Option<i32>,
    pub country: Option<String>,
    pub genre: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastSearchItem {
    pub title: String,
    pub artist: Option<String>,
    pub feed_url: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub genre: Option<String>,
    pub episode_count: Option<i32>,
    pub last_release_date: Option<String>,
    pub itunes_url: Option<String>,
    pub itunes_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastSearchResult {
    pub query: String,
    pub total_results: i32,
    pub results: Vec<PodcastSearchItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesResponse {
    result_count: i32,
    results: Vec<ItunesItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesItem {
    track_name: Option<String>,
    collection_name: Option<String>,
    artist_name: Option<String>,
    feed_url: Option<String>,
    description: Option<String>,
    artwork_url600: Option<String>,
    artwork_url100: Option<String>,
    primary_genre_name: Option<String>,
    track_count: Option<i32>,
    release_date: Option<String>,
    track_view_url: Option<String>,
    collection_view_url: Option<String>,
    collection_id: Option<i64>,
    track_id: Option<i64>,
}

pub fn router() -> Router<NativeState> {
    Router::new()
        .route(
            "/api/v1/podcasts/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/v1/podcasts/channels/{id}",
            axum::routing::patch(update_channel).delete(delete_channel),
        )
        .route(
            "/api/v1/podcasts/channels/{id}/episodes",
            get(list_episodes),
        )
        .route(
            "/api/v1/podcasts/channels/{id}/refresh",
            post(refresh_channel),
        )
        .route("/api/v1/podcasts/episodes/search", get(search_episodes))
        .route(
            "/api/v1/podcasts/episodes/{id}",
            axum::routing::delete(delete_episode),
        )
        .route(
            "/api/v1/podcasts/episodes/{id}/download",
            post(download_episode),
        )
        .route("/api/v1/podcasts/episodes/{id}/play", post(now_playing))
        .route(
            "/api/v1/podcasts/episodes/{id}/bookmark",
            get(get_bookmark).put(save_bookmark).delete(delete_bookmark),
        )
        .route("/api/v1/podcasts/episodes/{id}/history", get(play_history))
        .route("/api/v1/podcasts/opml/export", get(export_opml))
        // The explicit limit pins the effective bound to the documented
        // contract value; without it the route silently inherits axum's
        // implicit 2 MiB default, which today coincides with MAX_OPML_BYTES.
        .route(
            "/api/v1/podcasts/opml/import",
            post(import_opml).layer(DefaultBodyLimit::max(MAX_OPML_BYTES)),
        )
        .route("/api/v1/podcasts/discover/search", get(discover_search))
        .route("/api/v1/podcasts/discover/trending", get(discover_trending))
        .route(
            "/api/v1/podcasts/discover/lookup/{itunes_id}",
            get(discover_lookup),
        )
}

async fn require_podcast_access(
    state: &NativeState,
    principal: NativePrincipal,
) -> Result<(), NativeApiError> {
    if !state.podcast_config.enabled {
        return Err(NativeApiError::NotFound);
    }
    let allowed = principal.is_admin
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_user_roles WHERE user_id = $1 AND role = 'HasPodcastRole')",
        )
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?;
    if allowed {
        Ok(())
    } else {
        Err(NativeApiError::Forbidden)
    }
}

async fn list_channels(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<ChannelParams>,
) -> Result<Json<ChannelPage>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let (page, size, offset) = page(params.page, params.page_size)?;
    let direction = params.order_direction.as_deref().unwrap_or("ASC");
    if !direction.eq_ignore_ascii_case("asc") && !direction.eq_ignore_ascii_case("desc") {
        return Err(NativeApiError::BadRequest);
    }
    if params
        .order_by
        .as_deref()
        .is_some_and(|value| !matches!(value.to_ascii_lowercase().as_str(), "title" | "createdat"))
    {
        return Err(NativeApiError::BadRequest);
    }
    let search = params
        .search
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_podcast_channels WHERE owner_user_id = $1 \
         AND soft_deleted_at IS NULL AND ($2::text IS NULL OR title ILIKE ('%' || $2 || '%'))",
    )
    .bind(principal.user_id)
    .bind(search)
    .fetch_one(&state.pool)
    .await?;
    let rows = sqlx::query_as::<_, ChannelRow>(
        "SELECT c.id, c.public_api_id, c.title, c.description, c.image_url, c.feed_url, c.website, \
         c.last_sync_at, c.created_at, c.tags, \
         (SELECT count(*) FROM native_podcast_episodes e WHERE e.channel_id = c.id AND e.soft_deleted_at IS NULL) AS episode_count, \
         (SELECT max(h.played_at) FROM native_podcast_play_history h JOIN native_podcast_episodes e ON e.id = h.episode_id \
          WHERE e.channel_id = c.id AND h.user_id = $1 AND NOT h.is_now_playing) AS last_played_at, \
         (SELECT count(*) FROM native_podcast_play_history h JOIN native_podcast_episodes e ON e.id = h.episode_id \
          WHERE e.channel_id = c.id AND h.user_id = $1 AND NOT h.is_now_playing) AS played_count, \
         (SELECT count(*) FROM native_podcast_episodes e WHERE e.channel_id = c.id AND e.download_status = 'Downloaded' \
          AND NOT EXISTS(SELECT 1 FROM native_podcast_play_history h WHERE h.episode_id = e.id AND h.user_id = $1 \
          AND NOT h.is_now_playing)) AS unplayed_count \
         FROM native_podcast_channels c WHERE c.owner_user_id = $1 AND c.soft_deleted_at IS NULL \
         AND ($2::text IS NULL OR c.title ILIKE ('%' || $2 || '%')) \
         ORDER BY CASE WHEN $3 THEN c.title END ASC, CASE WHEN NOT $3 THEN c.title END DESC, c.id \
         LIMIT $4 OFFSET $5",
    )
    .bind(principal.user_id).bind(search).bind(direction.eq_ignore_ascii_case("asc"))
    .bind(i64::from(size)).bind(offset).fetch_all(&state.pool).await?;
    let data = rows
        .into_iter()
        .map(channel_dto)
        .collect::<Result<_, _>>()?;
    Ok(Json(ChannelPage {
        current_page: page - 1,
        total_count: count(total)?,
        total_pages: pages(total, size)?,
        additional_client_data: None,
        errors: None,
        data,
        is_success: true,
        r#type: 200,
        messages: None,
        operation_time: None,
    }))
}

async fn create_channel(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<CreateChannel>,
) -> Result<Json<PodcastChannel>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    validate_url(&request.url)?;
    let id = Uuid::now_v7();
    let title = request.url.trim();
    let row = sqlx::query_as::<_, ChannelRow>(
        "INSERT INTO native_podcast_channels (public_api_id, owner_user_id, feed_url, title, created_at, updated_at) \
         VALUES ($1, $2, $3, $3, now(), now()) ON CONFLICT (owner_user_id, (lower(feed_url))) DO UPDATE SET \
         soft_deleted_at = NULL, updated_at = now() RETURNING id, public_api_id, title, description, image_url, \
         feed_url, website, last_sync_at, created_at, tags, 0::bigint AS episode_count, NULL::timestamptz AS last_played_at, \
         0::bigint AS played_count, 0::bigint AS unplayed_count",
    ).bind(id).bind(principal.user_id).bind(title).fetch_one(&state.pool).await?;
    Ok(Json(channel_dto(row)?))
}

async fn update_channel(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Json(request): Json<UpdateChannel>,
) -> Result<Json<PodcastChannel>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    if request
        .refresh_interval_hours
        .is_some_and(|v| !(1..=8760).contains(&v))
        || request
            .max_downloaded_episodes
            .is_some_and(|v| !(0..=100_000).contains(&v))
        || request.max_storage_bytes.is_some_and(|v| v < 0)
    {
        return Err(NativeApiError::BadRequest);
    }
    let changed = sqlx::query(
        "UPDATE native_podcast_channels SET auto_download_enabled = COALESCE($3, auto_download_enabled), \
         refresh_interval_hours = COALESCE($4, refresh_interval_hours), \
         max_downloaded_episodes = COALESCE($5, max_downloaded_episodes), \
         max_storage_bytes = COALESCE($6, max_storage_bytes), updated_at = now() \
         WHERE id = $1 AND owner_user_id = $2 AND soft_deleted_at IS NULL",
    ).bind(id).bind(principal.user_id).bind(request.auto_download_enabled)
    .bind(request.refresh_interval_hours).bind(request.max_downloaded_episodes)
    .bind(request.max_storage_bytes).execute(&state.pool).await?;
    if changed.rows_affected() == 0 {
        return Err(NativeApiError::CapturedNotFound("Channel not found"));
    }
    Ok(Json(channel_dto(
        load_channel(&state, principal.user_id, id).await?,
    )?))
}

async fn delete_channel(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Query(params): Query<DeleteChannelParams>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let changed = if params.soft_delete.unwrap_or(true) {
        sqlx::query_scalar::<_, i64>(
            "WITH deleted AS (
                 UPDATE native_podcast_channels SET soft_deleted_at=now(),updated_at=now()
                 WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL RETURNING id
             ), cancelled AS (
                 UPDATE native_podcast_episodes SET soft_deleted_at=now(),
                     download_status='NotDownloaded',local_path=NULL,downloaded_bytes=NULL,
                     downloaded_at=NULL,download_error=NULL,download_lease_owner=NULL,
                     download_lease_expires_at=NULL,updated_at=now()
                 WHERE channel_id IN (SELECT id FROM deleted) RETURNING id
             ) SELECT count(*) FROM deleted",
        )
        .bind(id)
        .bind(principal.user_id)
        .fetch_one(&state.pool)
        .await?
    } else {
        i64::try_from(
            sqlx::query("DELETE FROM native_podcast_channels WHERE id = $1 AND owner_user_id = $2")
                .bind(id)
                .bind(principal.user_id)
                .execute(&state.pool)
                .await?
                .rows_affected(),
        )
        .map_err(|_| NativeApiError::Unavailable)?
    };
    if changed == 0 {
        return Err(NativeApiError::CapturedBadRequest("Channel not found"));
    }
    Ok(StatusCode::OK)
}

async fn refresh_channel(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    crate::podcast_worker::refresh_channel(
        &state.pool,
        principal.user_id,
        id,
        state.podcast_config.allow_loopback_http,
    )
    .await
    .map_err(|error| match error {
        crate::podcast_worker::PodcastWorkerError::InvalidResponse => {
            NativeApiError::CapturedNotFound("Channel not found")
        }
        crate::podcast_worker::PodcastWorkerError::Database(error) => {
            NativeApiError::Database(error)
        }
        _ => NativeApiError::CapturedInternal("An unexpected error occurred"),
    })?;
    Ok(StatusCode::OK)
}

async fn list_episodes(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Query(params): Query<EpisodePageParams>,
) -> Result<Json<EpisodePage>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    load_channel(&state, principal.user_id, id)
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => NativeApiError::CapturedNotFound("Channel not found"),
            other => other,
        })?;
    episode_page(
        &state,
        principal.user_id,
        Some(id),
        None,
        params.page,
        params.page_size,
    )
    .await
}

async fn search_episodes(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<SearchEpisodeParams>,
) -> Result<Json<EpisodePage>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let query = params.query.trim();
    if query.is_empty() || query.len() > 1000 {
        return Err(NativeApiError::BadRequest);
    }
    episode_page(
        &state,
        principal.user_id,
        None,
        Some(query),
        params.page,
        params.page_size,
    )
    .await
}

async fn episode_page(
    state: &NativeState,
    user_id: Uuid,
    channel_id: Option<i64>,
    search: Option<&str>,
    page_no: Option<i32>,
    page_size: Option<i32>,
) -> Result<Json<EpisodePage>, NativeApiError> {
    let (page, size, offset) = page(page_no, page_size)?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_podcast_episodes e JOIN native_podcast_channels c ON c.id=e.channel_id \
         WHERE c.owner_user_id=$1 AND e.soft_deleted_at IS NULL AND ($2::bigint IS NULL OR e.channel_id=$2) \
         AND ($3::text IS NULL OR to_tsvector('simple', e.title || ' ' || e.description) @@ plainto_tsquery('simple',$3))",
    ).bind(user_id).bind(channel_id).bind(search).fetch_one(&state.pool).await?;
    let rows = sqlx::query_as::<_, EpisodeRow>(
        "SELECT e.id, e.public_api_id, e.title, e.description, e.published_at, e.duration_ms, e.enclosure_url, \
         e.local_path, e.download_status, e.download_error, e.created_at, c.title AS channel_title, c.public_api_id AS channel_api_id, \
         (SELECT max(h.played_at) FROM native_podcast_play_history h WHERE h.episode_id=e.id AND h.user_id=$1 AND NOT h.is_now_playing) AS last_played_at, \
         (SELECT count(*) FROM native_podcast_play_history h WHERE h.episode_id=e.id AND h.user_id=$1 AND NOT h.is_now_playing) AS played_count \
         FROM native_podcast_episodes e JOIN native_podcast_channels c ON c.id=e.channel_id \
         WHERE c.owner_user_id=$1 AND e.soft_deleted_at IS NULL AND ($2::bigint IS NULL OR e.channel_id=$2) \
         AND ($3::text IS NULL OR to_tsvector('simple', e.title || ' ' || e.description) @@ plainto_tsquery('simple',$3)) \
         ORDER BY e.published_at DESC NULLS LAST,e.id DESC LIMIT $4 OFFSET $5",
    ).bind(user_id).bind(channel_id).bind(search).bind(i64::from(size)).bind(offset).fetch_all(&state.pool).await?;
    let data = rows
        .into_iter()
        .map(episode_dto)
        .collect::<Result<_, _>>()?;
    Ok(Json(EpisodePage {
        current_page: page - 1,
        total_count: count(total)?,
        total_pages: pages(total, size)?,
        additional_client_data: None,
        errors: None,
        data,
        is_success: true,
        r#type: 200,
        messages: None,
        operation_time: None,
    }))
}

async fn download_episode(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    mutate_episode(&state, principal.user_id, id, "download")
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => NativeApiError::CapturedBadRequest("Episode not found"),
            other => other,
        })?;
    Ok(StatusCode::OK)
}
async fn delete_episode(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    mutate_episode(&state, principal.user_id, id, "delete")
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => NativeApiError::CapturedBadRequest("Episode not found"),
            other => other,
        })?;
    Ok(StatusCode::OK)
}
async fn mutate_episode(
    state: &NativeState,
    user: Uuid,
    id: i64,
    op: &str,
) -> Result<(), NativeApiError> {
    let sql = if op == "download" {
        "UPDATE native_podcast_episodes e SET download_status='Queued',download_error=NULL,updated_at=now() FROM native_podcast_channels c WHERE e.id=$1 AND c.id=e.channel_id AND c.owner_user_id=$2 AND e.soft_deleted_at IS NULL"
    } else {
        "UPDATE native_podcast_episodes e SET soft_deleted_at=now(),
         download_status='NotDownloaded',local_path=NULL,downloaded_bytes=NULL,downloaded_at=NULL,
         download_error=NULL,download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now()
         FROM native_podcast_channels c WHERE e.id=$1 AND c.id=e.channel_id
         AND c.owner_user_id=$2 AND e.soft_deleted_at IS NULL"
    };
    if sqlx::query(sql)
        .bind(id)
        .bind(user)
        .execute(&state.pool)
        .await?
        .rows_affected()
        == 0
    {
        return Err(NativeApiError::NotFound);
    }
    Ok(())
}

async fn now_playing(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Query(params): Query<NowPlayingParams>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    ensure_episode(&state, principal.user_id, id)
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => {
                NativeApiError::CapturedInternal("An unexpected error occurred")
            }
            other => other,
        })?;
    if params.seconds_played.is_some_and(|v| v < 0) {
        return Err(NativeApiError::BadRequest);
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM native_users WHERE id=$1 FOR UPDATE")
        .bind(principal.user_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE native_podcast_play_history SET is_now_playing=false
         WHERE user_id=$1 AND is_now_playing AND episode_id<>$2",
    )
    .bind(principal.user_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let updated = sqlx::query(
        "UPDATE native_podcast_play_history
         SET seconds_played=$3,last_heartbeat_at=now()
         WHERE user_id=$1 AND episode_id=$2 AND is_now_playing",
    )
    .bind(principal.user_id)
    .bind(id)
    .bind(params.seconds_played)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO native_podcast_play_history(
                user_id,episode_id,played_at,client,seconds_played,is_now_playing,last_heartbeat_at
             ) VALUES($1,$2,now(),'Melodee.Blazor',$3,true,now())",
        )
        .bind(principal.user_id)
        .bind(id)
        .bind(params.seconds_played)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::OK)
}

async fn get_bookmark(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
) -> Result<Json<PodcastBookmark>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    ensure_episode(&state, principal.user_id, id)
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => {
                NativeApiError::CapturedBadRequest("Error retrieving bookmark")
            }
            other => other,
        })?;
    let row=sqlx::query_as::<_,BookmarkRow>("SELECT id,episode_id,position_seconds,comment,created_at,updated_at FROM native_podcast_bookmarks WHERE user_id=$1 AND episode_id=$2")
        .bind(principal.user_id).bind(id).fetch_optional(&state.pool).await?
        .ok_or(NativeApiError::CapturedBadRequest("Error retrieving bookmark"))?;
    Ok(Json(bookmark_dto(row)))
}
async fn save_bookmark(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Json(req): Json<SaveBookmark>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    ensure_episode(&state, principal.user_id, id)
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => {
                NativeApiError::CapturedInternal("An unexpected error occurred")
            }
            other => other,
        })?;
    if req.position_seconds < 0 || req.comment.as_ref().is_some_and(|v| v.len() > 10000) {
        return Err(NativeApiError::BadRequest);
    }
    sqlx::query("INSERT INTO native_podcast_bookmarks(user_id,episode_id,position_seconds,comment,created_at,updated_at) VALUES($1,$2,$3,$4,now(),now()) ON CONFLICT(user_id,episode_id) DO UPDATE SET position_seconds=EXCLUDED.position_seconds,comment=EXCLUDED.comment,updated_at=now()")
        .bind(principal.user_id).bind(id).bind(req.position_seconds).bind(req.comment).execute(&state.pool).await?;
    Ok(StatusCode::OK)
}
async fn delete_bookmark(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
) -> Result<StatusCode, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    ensure_episode(&state, principal.user_id, id)
        .await
        .map_err(|error| match error {
            NativeApiError::NotFound => NativeApiError::CapturedBadRequest("Bookmark not found"),
            other => other,
        })?;
    let deleted =
        sqlx::query("DELETE FROM native_podcast_bookmarks WHERE user_id=$1 AND episode_id=$2")
            .bind(principal.user_id)
            .bind(id)
            .execute(&state.pool)
            .await?;
    if deleted.rows_affected() == 0 {
        return Err(NativeApiError::CapturedBadRequest("Bookmark not found"));
    }
    Ok(StatusCode::OK)
}
async fn play_history(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<i64>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Vec<PodcastHistory>>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    if !(1..=MAX_HISTORY_SIZE).contains(&limit) || offset < 0 {
        return Err(NativeApiError::BadRequest);
    }
    let rows=sqlx::query_as::<_,HistoryRow>("SELECT id,episode_id,played_at,client,by_user_agent,ip_address,seconds_played,source,is_now_playing,last_heartbeat_at FROM native_podcast_play_history WHERE user_id=$1 AND episode_id=$2 ORDER BY played_at DESC,id DESC LIMIT $3 OFFSET $4")
        .bind(principal.user_id).bind(id).bind(i64::from(limit)).bind(i64::from(offset)).fetch_all(&state.pool).await?;
    Ok(Json(rows.into_iter().map(history_dto).collect()))
}

async fn export_opml(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Response, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let rows=sqlx::query_as::<_,(String,String)>("SELECT title,feed_url FROM native_podcast_channels WHERE owner_user_id=$1 AND soft_deleted_at IS NULL ORDER BY title,id").bind(principal.user_id).fetch_all(&state.pool).await?;
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-16\"?>\n<opml version=\"2.0\">\n  <head>\n    <title>Melodee Podcast Subscriptions</title>\n    <dateCreated>",
    );
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc2822)
        .map_err(|_| NativeApiError::Unavailable)?;
    xml.push_str(&xml_escape(&created_at));
    xml.push_str("</dateCreated>\n    <docs>http://opml.org/spec2.opml</docs>\n  </head>\n  <body>\n    <outline text=\"Podcasts\" title=\"Podcasts\"");
    if rows.is_empty() {
        xml.push_str(" />\n");
    } else {
        xml.push_str(">\n");
    }
    for (title, url) in rows {
        xml.push_str("      <outline type=\"rss\" text=\"");
        xml.push_str(&xml_escape(&title));
        xml.push_str("\" xmlUrl=\"");
        xml.push_str(&xml_escape(&url));
        xml.push_str("\" />\n");
    }
    if !xml.ends_with("title=\"Podcasts\" />\n") {
        xml.push_str("    </outline>\n");
    }
    xml.push_str("  </body>\n</opml>");
    Ok(([(header::CONTENT_TYPE, "application/xml")], xml).into_response())
}
async fn import_opml(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    body: Bytes,
) -> Result<Json<OpmlImportResult>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    if body.is_empty() || body.len() > MAX_OPML_BYTES {
        return Err(NativeApiError::BadRequest);
    }
    let text = std::str::from_utf8(&body).map_err(|_| NativeApiError::BadRequest)?;
    let urls = extract_opml_urls(text)?;
    if urls.is_empty() {
        return Err(NativeApiError::CapturedBadRequest(
            "No podcast feeds found in OPML file",
        ));
    }
    let mut imported = Vec::new();
    let mut duplicates = Vec::new();
    let mut errors = Vec::new();
    for url in urls {
        if validate_url(&url).is_err() {
            errors.push(url);
            continue;
        }
        let id = Uuid::now_v7();
        let result=sqlx::query("INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) VALUES($1,$2,$3,$3,now(),now()) ON CONFLICT(owner_user_id,(lower(feed_url))) DO NOTHING").bind(id).bind(principal.user_id).bind(&url).execute(&state.pool).await?;
        if result.rows_affected() == 0 {
            duplicates.push(url)
        } else {
            imported.push(url)
        }
    }
    let result = OpmlImportResult {
        imported: count(imported.len() as i64)?,
        skipped: count(duplicates.len() as i64)?,
        failed: count(errors.len() as i64)?,
        total_processed: count((imported.len() + duplicates.len() + errors.len()) as i64)?,
        imported_feeds: imported,
        duplicates,
        errors,
    };
    Ok(Json(result))
}

async fn discover_search(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<DiscoveryParams>,
) -> Result<Json<PodcastSearchResult>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or(NativeApiError::BadRequest)?;
    validate_discovery(&params)?;
    let limit = params.limit.unwrap_or(25);
    let mut request = vec![
        ("term", query.to_owned()),
        ("media", "podcast".to_owned()),
        ("limit", limit.to_string()),
    ];
    if let Some(country) = params.country.as_deref() {
        request.push(("country", country.to_owned()));
    }
    if let Some(genre) = params.genre.as_deref() {
        request.push(("genre", genre.to_owned()));
    }
    let upstream = state.podcast_discovery.request("search", &request).await?;
    let results = map_discovery_results(upstream, limit)?;
    Ok(Json(PodcastSearchResult {
        query: query.to_owned(),
        total_results: count(results.len() as i64)?,
        results,
    }))
}
async fn discover_trending(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<DiscoveryParams>,
) -> Result<Json<PodcastSearchResult>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    validate_discovery(&params)?;
    let limit = params.limit.unwrap_or(25);
    let mut request = vec![
        ("term", "podcast".to_owned()),
        ("media", "podcast".to_owned()),
        ("limit", limit.to_string()),
    ];
    if let Some(country) = params.country.as_deref() {
        request.push(("country", country.to_owned()));
    }
    if let Some(genre) = params.genre.as_deref() {
        request.push(("genre", genre.to_owned()));
    }
    let upstream = state.podcast_discovery.request("search", &request).await?;
    let results = map_discovery_results(upstream, limit)?;
    Ok(Json(PodcastSearchResult {
        query: "podcast".to_owned(),
        total_results: count(results.len() as i64)?,
        results,
    }))
}
async fn discover_lookup(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(itunes_id): Path<String>,
) -> Result<Json<PodcastSearchItem>, NativeApiError> {
    require_podcast_access(&state, principal).await?;
    if itunes_id.trim().is_empty() || itunes_id.len() > 200 {
        return Err(NativeApiError::BadRequest);
    }
    let upstream = state
        .podcast_discovery
        .request("lookup", &[("id", itunes_id)])
        .await?;
    map_discovery_results(upstream, 1)?
        .into_iter()
        .next()
        .map(Json)
        .ok_or(NativeApiError::CapturedBadRequest("Podcast not found"))
}

fn map_discovery_results(
    upstream: ItunesResponse,
    limit: i32,
) -> Result<Vec<PodcastSearchItem>, NativeApiError> {
    if upstream.result_count < 0
        || upstream.results.len() > MAX_DISCOVERY_SIZE as usize
        || upstream.results.len() > limit as usize
    {
        return Err(NativeApiError::CapturedInternal(
            "An unexpected error occurred",
        ));
    }
    upstream
        .results
        .into_iter()
        .map(|item| {
            let title = item
                .track_name
                .or(item.collection_name)
                .filter(|value| !value.trim().is_empty())
                .ok_or(NativeApiError::CapturedInternal(
                    "An unexpected error occurred",
                ))?;
            Ok(PodcastSearchItem {
                title,
                artist: item.artist_name,
                feed_url: item.feed_url,
                description: item.description,
                image_url: item.artwork_url600.or(item.artwork_url100),
                genre: item.primary_genre_name,
                episode_count: item.track_count,
                last_release_date: item.release_date,
                itunes_url: item.track_view_url.or(item.collection_view_url),
                itunes_id: item
                    .collection_id
                    .or(item.track_id)
                    .map(|value| value.to_string()),
            })
        })
        .collect()
}

#[derive(Debug, FromRow)]
struct ChannelRow {
    id: i64,
    public_api_id: Uuid,
    title: String,
    description: String,
    image_url: String,
    feed_url: String,
    website: String,
    last_sync_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    tags: String,
    episode_count: i64,
    last_played_at: Option<OffsetDateTime>,
    played_count: i64,
    unplayed_count: i64,
}
#[derive(Debug, FromRow)]
struct EpisodeRow {
    id: i64,
    public_api_id: Uuid,
    title: String,
    description: String,
    published_at: Option<OffsetDateTime>,
    duration_ms: Option<i64>,
    enclosure_url: Option<String>,
    local_path: Option<String>,
    download_status: String,
    download_error: Option<String>,
    created_at: OffsetDateTime,
    channel_title: String,
    channel_api_id: Uuid,
    last_played_at: Option<OffsetDateTime>,
    played_count: i64,
}
#[derive(Debug, FromRow)]
struct BookmarkRow {
    id: i64,
    episode_id: i64,
    position_seconds: i32,
    comment: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}
#[derive(Debug, FromRow)]
struct HistoryRow {
    id: i64,
    episode_id: i64,
    played_at: OffsetDateTime,
    client: String,
    by_user_agent: Option<String>,
    ip_address: Option<String>,
    seconds_played: Option<i32>,
    source: i16,
    is_now_playing: bool,
    last_heartbeat_at: Option<OffsetDateTime>,
}

async fn load_channel(
    state: &NativeState,
    user: Uuid,
    id: i64,
) -> Result<ChannelRow, NativeApiError> {
    sqlx::query_as::<_,ChannelRow>("SELECT c.id,c.public_api_id,c.title,c.description,c.image_url,c.feed_url,c.website,c.last_sync_at,c.created_at,c.tags,(SELECT count(*) FROM native_podcast_episodes e WHERE e.channel_id=c.id AND e.soft_deleted_at IS NULL) AS episode_count,(SELECT max(h.played_at) FROM native_podcast_play_history h JOIN native_podcast_episodes e ON e.id=h.episode_id WHERE e.channel_id=c.id AND h.user_id=$1 AND NOT h.is_now_playing) AS last_played_at,(SELECT count(*) FROM native_podcast_play_history h JOIN native_podcast_episodes e ON e.id=h.episode_id WHERE e.channel_id=c.id AND h.user_id=$1 AND NOT h.is_now_playing) AS played_count,(SELECT count(*) FROM native_podcast_episodes e WHERE e.channel_id=c.id AND e.download_status='Downloaded' AND e.soft_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM native_podcast_play_history h WHERE h.episode_id=e.id AND h.user_id=$1 AND NOT h.is_now_playing)) AS unplayed_count FROM native_podcast_channels c WHERE c.id=$2 AND c.owner_user_id=$1 AND c.soft_deleted_at IS NULL").bind(user).bind(id).fetch_optional(&state.pool).await?.ok_or(NativeApiError::NotFound)
}
async fn ensure_episode(state: &NativeState, user: Uuid, id: i64) -> Result<(), NativeApiError> {
    if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM native_podcast_episodes e JOIN native_podcast_channels c ON c.id=e.channel_id WHERE e.id=$1 AND c.owner_user_id=$2 AND e.soft_deleted_at IS NULL)").bind(id).bind(user).fetch_one(&state.pool).await?{Ok(())}else{Err(NativeApiError::NotFound)}
}
fn channel_dto(r: ChannelRow) -> Result<PodcastChannel, NativeApiError> {
    let _ = r.id;
    Ok(PodcastChannel {
        id: r.public_api_id,
        title: r.title,
        description: r.description,
        image_url: r.image_url,
        feed_url: r.feed_url,
        website: r.website.clone(),
        last_sync_at: r.last_sync_at.map(|v| v.to_string()),
        created_at: r.created_at.to_string(),
        tags: r.tags,
        user_starred: false,
        user_rating: 0,
        site_url: if r.website.is_empty() {
            None
        } else {
            Some(r.website)
        },
        episode_count: count(r.episode_count)?,
        last_played_at: r.last_played_at.map(|v| v.to_string()),
        played_count: count(r.played_count)?,
        unplayed_downloaded_count: count(r.unplayed_count)?,
    })
}
fn episode_dto(r: EpisodeRow) -> Result<PodcastEpisode, NativeApiError> {
    Ok(PodcastEpisode {
        id: r.id,
        api_key: r.public_api_id,
        title: r.title,
        description: r.description,
        publish_date: r.published_at.map(|v| v.to_string()),
        duration_ms: r.duration_ms.map(|v| v as f64),
        duration_formatted: format_duration(r.duration_ms),
        channel_title: r.channel_title,
        channel_api_key: r.channel_api_id,
        is_downloaded: r.local_path.is_some(),
        created_at: r.created_at.to_string(),
        tags: String::new(),
        user_starred: false,
        user_rating: 0,
        download_status: r.download_status,
        download_error: r.download_error,
        enclosure_url: r.enclosure_url,
        last_played_at: r.last_played_at.map(|v| v.to_string()),
        played_count: count(r.played_count)?,
    })
}
fn bookmark_dto(r: BookmarkRow) -> PodcastBookmark {
    PodcastBookmark {
        id: r.id,
        podcast_episode_id: r.episode_id,
        position_seconds: r.position_seconds,
        comment: r.comment,
        created_at: r.created_at.to_string(),
        updated_at: r.updated_at.to_string(),
    }
}
fn history_dto(r: HistoryRow) -> PodcastHistory {
    PodcastHistory {
        id: r.id,
        podcast_episode_id: r.episode_id,
        played_at: r.played_at.to_string(),
        client: r.client,
        by_user_agent: r.by_user_agent,
        ip_address: r.ip_address,
        seconds_played: r.seconds_played,
        source: r.source,
        is_now_playing: r.is_now_playing,
        last_heartbeat_at: r.last_heartbeat_at.map(|v| v.to_string()),
    }
}
fn format_duration(ms: Option<i64>) -> String {
    let seconds = ms.unwrap_or(0) / 1000;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}
fn page(page: Option<i32>, size: Option<i32>) -> Result<(i32, i32, i64), NativeApiError> {
    let p = page.unwrap_or(1);
    let s = size.unwrap_or(50);
    if p < 1 || !(1..=MAX_PAGE_SIZE).contains(&s) {
        return Err(NativeApiError::BadRequest);
    }
    Ok((p, s, i64::from(p - 1) * i64::from(s)))
}
fn pages(total: i64, size: i32) -> Result<i32, NativeApiError> {
    count(if total == 0 {
        0
    } else {
        (total + i64::from(size) - 1) / i64::from(size)
    })
}
fn count(v: i64) -> Result<i32, NativeApiError> {
    i32::try_from(v).map_err(|_| NativeApiError::Unavailable)
}
fn validate_url(v: &str) -> Result<(), NativeApiError> {
    if v.trim().len() > 4096 || !(v.starts_with("http://") || v.starts_with("https://")) {
        Err(NativeApiError::CapturedBadRequest("Invalid URL format"))
    } else {
        Ok(())
    }
}
fn validate_discovery(p: &DiscoveryParams) -> Result<(), NativeApiError> {
    let l = p.limit.unwrap_or(25);
    if !(1..=MAX_DISCOVERY_SIZE).contains(&l)
        || p.country.as_ref().is_some_and(|v| v.len() > 8)
        || p.genre.as_ref().is_some_and(|v| v.len() > 200)
    {
        Err(NativeApiError::BadRequest)
    } else {
        Ok(())
    }
}
fn extract_opml_urls(text: &str) -> Result<Vec<String>, NativeApiError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    let mut saw_opml = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == b"opml" {
                    saw_opml = true;
                }
                if element.local_name().as_ref() == b"outline" {
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|_| NativeApiError::BadRequest)?;
                        if attribute.key.as_ref() == b"xmlUrl" {
                            if out.len() >= 1000 {
                                return Err(NativeApiError::BadRequest);
                            }
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|_| NativeApiError::BadRequest)?
                                .into_owned();
                            if !value.trim().is_empty() {
                                out.push(value);
                            }
                        }
                    }
                }
            }
            Ok(Event::DocType(_)) => return Err(NativeApiError::BadRequest),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(NativeApiError::BadRequest),
        }
    }
    if saw_opml {
        Ok(out)
    } else {
        Err(NativeApiError::BadRequest)
    }
}
fn xml_escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn podcast_bounds_and_opml_are_fail_closed() {
        assert!(page(Some(0), Some(50)).is_err());
        assert!(page(Some(1), Some(501)).is_err());
        assert!(validate_url("file:///media").is_err());
        assert!(extract_opml_urls("not xml").is_err());
        assert_eq!(
            extract_opml_urls(
                "<opml><body><outline xmlUrl='https://example.test/feed?a=1&amp;b=2'/></body></opml>"
            )
            .ok(),
            Some(vec!["https://example.test/feed?a=1&b=2".to_owned()])
        );
        assert!(extract_opml_urls("<opml><body><outline xmlUrl='broken'></body></opml>").is_err());
        assert!(extract_opml_urls("<!DOCTYPE opml><opml><body/></opml>").is_err());
    }

    #[test]
    fn discovery_endpoint_requires_https_except_for_loopback_fixtures() {
        assert!(PodcastDiscoveryClient::new("https://itunes.apple.com/").is_ok());
        assert!(PodcastDiscoveryClient::new_for_loopback_fixture("http://127.0.0.1:1234/").is_ok());
        assert!(PodcastDiscoveryClient::new("http://127.0.0.1:1234/").is_err());
        assert!(PodcastDiscoveryClient::new("http://example.com/").is_err());
        assert!(PodcastDiscoveryClient::new("file:///tmp/discovery").is_err());
    }
}
