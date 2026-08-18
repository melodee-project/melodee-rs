//! Native-v1 discovery, statistics, audio-feature, backend, and theme routes.

use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{Authenticated, NativeState, dto::PaginationMetadata, error::NativeApiError};

const MAX_PAGE: i32 = 100;
const MAX_PAGE_OFFSET: i64 = 10_000;
const MAX_CHART_PLAYLIST_TRACKS: i64 = 5_000;
// Each recommendation family inspects at most this many globally-ranked
// candidates. If that window cannot produce the requested result while more
// candidates remain, the route fails closed with 503 instead of returning a
// falsely empty/starved response or scanning an unbounded catalog tail.
const MAX_RECOMMENDATION_CANDIDATES_SCANNED: i64 = 4_096;
const MAX_THEME_ARCHIVE_BYTES: usize = 50 * 1024 * 1024;
const MAX_THEME_EXPANDED_BYTES: u64 = 100 * 1024 * 1024;
const MAX_THEME_ASSET_BYTES: u64 = 10 * 1024 * 1024;
const MAX_THEME_ENTRIES: usize = 256;

pub fn protected_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/analytics/listening", get(listening))
        .route("/api/v1/analytics/top/{period}", get(top_content))
        .route("/api/v1/artist-lookup", post(artist_lookup))
        .route("/api/v1/artist-lookup/providers", get(lookup_providers))
        .route("/api/v1/audio/bpm", get(audio_by_bpm))
        .route("/api/v1/audio/features/{id}", get(audio_features))
        .route("/api/v1/charts", get(charts))
        .route("/api/v1/charts/{id}", get(chart))
        .route("/api/v1/charts/{id}/playlist", get(chart_playlist))
        .route("/api/v1/playback-backend/status", get(backend_status))
        .route(
            "/api/v1/playback-backend/capabilities",
            get(backend_capabilities),
        )
        .route(
            "/api/v1/playback-backend/initialize",
            post(backend_initialize),
        )
        .route("/api/v1/playback-backend/shutdown", post(backend_shutdown))
        .route(
            "/api/v1/playback-backend/register-endpoint",
            post(backend_register),
        )
        .route(
            "/api/v1/playback-backend/heartbeat/{session_id}",
            post(backend_heartbeat),
        )
        .route("/api/v1/recommendations", get(recommendations))
        .route("/api/v1/system/stats", get(system_stats))
}

/// Browser-only routes governed by the legacy default cookie scheme. Keeping
/// these separate prevents bearer JWTs from authorizing UI mutations.
pub fn cookie_protected_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/themes/me", post(set_theme))
        .route("/api/v1/themes/rescan", post(rescan_themes))
        .route("/api/v1/themes/import", post(theme_import))
        .route("/api/v1/themes/{id}/export", get(theme_export))
        .route("/api/v1/themes/{id}", delete(theme_delete))
}

pub fn public_router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/themes", get(themes))
        .route("/api/v1/themes/{id}/file/{*path}", get(theme_asset))
        .route("/api/v1/system/throw", get(synthetic_internal_error))
}

fn bounded_page(
    page: Option<i32>,
    size: Option<i32>,
    default: i32,
) -> Result<(i32, i32, i64), NativeApiError> {
    let page = page.unwrap_or(1);
    let size = size.unwrap_or(default);
    if page < 1 || !(1..=MAX_PAGE).contains(&size) {
        return Err(NativeApiError::BadRequest);
    }
    let offset = i64::from(page - 1)
        .checked_mul(i64::from(size))
        .filter(|offset| *offset <= MAX_PAGE_OFFSET)
        .ok_or(NativeApiError::BadRequest)?;
    Ok((page, size, offset))
}

#[derive(Debug, Deserialize)]
struct Period {
    period: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsItem {
    id: Uuid,
    name: String,
    play_count: i32,
    play_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsGenre {
    name: String,
    play_count: i32,
    play_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListeningByHour {
    hour: i32,
    play_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListeningByDay {
    day: &'static str,
    play_time: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListeningStatistics {
    period: String,
    total_play_time: f64,
    total_tracks_played: i32,
    top_artists: Vec<AnalyticsItem>,
    top_albums: Vec<AnalyticsItem>,
    top_genres: Vec<AnalyticsGenre>,
    listening_by_time_of_day: Vec<ListeningByHour>,
    listening_by_day_of_week: Vec<ListeningByDay>,
}

fn period_days(period: &str) -> Result<Option<i64>, NativeApiError> {
    match period.to_ascii_lowercase().as_str() {
        "day" => Ok(Some(1)),
        "week" => Ok(Some(7)),
        "month" => Ok(Some(30)),
        "year" => Ok(Some(365)),
        "all_time" => Ok(None),
        _ => Err(NativeApiError::BadRequest),
    }
}

async fn listening(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(query): Query<Period>,
) -> Result<Json<ListeningStatistics>, NativeApiError> {
    let period = query.period.unwrap_or_else(|| "week".to_owned());
    let days = period_days(&period)?;
    let (plays, seconds) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*),COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer))",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_one(&state.pool)
    .await?;
    let top_artists = sqlx::query_as::<_, (Uuid, String, i64, i64)>(
        "SELECT artist.public_api_id,artist.canonical_name,count(*),\
         COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         JOIN LATERAL (SELECT billing.artist_id FROM track_artists billing \
           WHERE billing.track_id=track.id AND billing.role='primary' \
           ORDER BY billing.presentation_order,billing.id LIMIT 1) primary_artist ON true \
         JOIN artists artist ON artist.id=primary_artist.artist_id \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer)) \
         GROUP BY artist.id ORDER BY count(*) DESC,artist.public_api_id LIMIT 10",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(id, name, count, duration)| AnalyticsItem {
        id,
        name,
        play_count: i32::try_from(count).unwrap_or(i32::MAX),
        play_time: duration as f64,
    })
    .collect();
    let top_albums = sqlx::query_as::<_, (Uuid, String, i64, i64)>(
        "SELECT release.public_api_id,release.title,count(*),\
         COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         JOIN releases release ON release.id=track.release_id \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer)) \
         GROUP BY release.id ORDER BY count(*) DESC,release.public_api_id LIMIT 10",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(id, name, count, duration)| AnalyticsItem {
        id,
        name,
        play_count: i32::try_from(count).unwrap_or(i32::MAX),
        play_time: duration as f64,
    })
    .collect();
    let top_genres = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT genre.name,count(*),\
         COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         JOIN LATERAL (SELECT btrim(file.value->'metadata'->>'genre') AS name \
           FROM catalog_imports catalog_import \
           JOIN release_work_item_manifests manifest ON manifest.id=catalog_import.manifest_row_id \
           CROSS JOIN LATERAL jsonb_array_elements(COALESCE(manifest.manifest_payload->'files','[]')) file(value) \
           WHERE catalog_import.release_id=track.release_id \
             AND file.value->>'relative_path'=track.source_path \
             AND btrim(COALESCE(file.value->'metadata'->>'genre',''))<>'' \
           ORDER BY catalog_import.imported_at DESC LIMIT 1) genre ON true \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer)) \
         GROUP BY genre.name ORDER BY count(*) DESC,genre.name LIMIT 10",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(name, count, duration)| AnalyticsGenre {
        name,
        play_count: i32::try_from(count).unwrap_or(i32::MAX),
        play_time: duration as f64,
    })
    .collect();
    let hour_rows = sqlx::query_as::<_, (i32, i64)>(
        "SELECT extract(hour FROM play.played_at)::integer,\
         COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer)) \
         GROUP BY 1 ORDER BY 1",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?;
    let mut hour_totals = [0_i64; 24];
    for (hour, seconds) in hour_rows {
        if let Ok(index) = usize::try_from(hour)
            && let Some(total) = hour_totals.get_mut(index)
        {
            *total = seconds;
        }
    }
    let hours = hour_totals
        .into_iter()
        .enumerate()
        .map(|(hour, seconds)| ListeningByHour {
            hour: i32::try_from(hour).unwrap_or_default(),
            play_time: seconds as f64,
        })
        .collect();
    let day_rows = sqlx::query_as::<_, (i32, i64)>(
        "SELECT extract(isodow FROM play.played_at)::integer,\
         COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
         FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
         WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
           AND ($2::bigint IS NULL OR play.played_at >= now()-make_interval(days=>$2::integer)) \
         GROUP BY 1 ORDER BY 1",
    )
    .bind(principal.user_id)
    .bind(days)
    .fetch_all(&state.pool)
    .await?;
    let mut day_totals = [0_i64; 7];
    for (day, seconds) in day_rows {
        if let Ok(index) = usize::try_from(day.saturating_sub(1))
            && let Some(total) = day_totals.get_mut(index)
        {
            *total = seconds;
        }
    }
    let days_of_week = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ]
    .into_iter()
    .zip(day_totals)
    .map(|(day, seconds)| ListeningByDay {
        day,
        play_time: seconds as f64,
    })
    .collect();
    Ok(Json(ListeningStatistics {
        period,
        total_play_time: seconds as f64,
        total_tracks_played: i32::try_from(plays).unwrap_or(i32::MAX),
        top_artists,
        top_albums,
        top_genres,
        listening_by_time_of_day: hours,
        listening_by_day_of_week: days_of_week,
    }))
}

#[derive(Debug, Deserialize)]
struct TopParams {
    r#type: String,
    limit: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TopContentItem {
    id: Uuid,
    name: String,
    play_count: i32,
    play_time: f64,
    rank: i32,
}

#[derive(Debug, Serialize)]
struct TopContentResponse {
    period: String,
    r#type: String,
    items: Vec<TopContentItem>,
}

async fn top_content(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(period): Path<String>,
    Query(query): Query<TopParams>,
) -> Result<Json<TopContentResponse>, NativeApiError> {
    let days = period_days(&period)?;
    let limit = query.limit.unwrap_or(10);
    let content_type = query.r#type.to_ascii_lowercase();
    if !(1..=100).contains(&limit) || !matches!(content_type.as_str(), "song" | "album" | "artist")
    {
        return Err(NativeApiError::BadRequest);
    }
    let rows = match content_type.as_str() {
        "song" => sqlx::query_as::<_, (Uuid, String, i64, i64)>(
            "SELECT track.public_api_id,track.title,count(*),\
             COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
             FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
             WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
               AND ($2::bigint IS NULL OR play.played_at>=now()-make_interval(days=>$2::integer)) \
             GROUP BY track.id ORDER BY count(*) DESC,track.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(days)
        .bind(i64::from(limit))
        .fetch_all(&state.pool)
        .await?,
        "album" => sqlx::query_as::<_, (Uuid, String, i64, i64)>(
            "SELECT release.public_api_id,release.title,count(*),\
             COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
             FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
             JOIN releases release ON release.id=track.release_id \
             WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
               AND ($2::bigint IS NULL OR play.played_at>=now()-make_interval(days=>$2::integer)) \
             GROUP BY release.id ORDER BY count(*) DESC,release.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(days)
        .bind(i64::from(limit))
        .fetch_all(&state.pool)
        .await?,
        "artist" => sqlx::query_as::<_, (Uuid, String, i64, i64)>(
            "SELECT artist.public_api_id,artist.canonical_name,count(*),\
             COALESCE(sum(COALESCE(play.duration_seconds::bigint,track.duration_ms/1000,0)),0)::bigint \
             FROM native_track_plays play JOIN tracks track ON track.id=play.track_id \
             JOIN LATERAL (SELECT billing.artist_id FROM track_artists billing \
               WHERE billing.track_id=track.id AND billing.role='primary' \
               ORDER BY billing.presentation_order,billing.id LIMIT 1) primary_artist ON true \
             JOIN artists artist ON artist.id=primary_artist.artist_id \
             WHERE play.user_id=$1 AND native_user_can_access_release($1,track.release_id) \
               AND ($2::bigint IS NULL OR play.played_at>=now()-make_interval(days=>$2::integer)) \
             GROUP BY artist.id ORDER BY count(*) DESC,artist.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(days)
        .bind(i64::from(limit))
        .fetch_all(&state.pool)
        .await?,
        _ => return Err(NativeApiError::BadRequest),
    };
    let items = rows
        .into_iter()
        .enumerate()
        .map(|(index, (id, name, count, duration))| TopContentItem {
            id,
            name,
            play_count: i32::try_from(count).unwrap_or(i32::MAX),
            play_time: duration as f64,
            rank: i32::try_from(index + 1).unwrap_or(i32::MAX),
        })
        .collect();
    Ok(Json(TopContentResponse {
        period,
        r#type: query.r#type,
        items,
    }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInfo {
    id: String,
    display_name: String,
    is_enabled: bool,
}

const MELODEE_PROVIDER_ID: &str = "018A798D-7B68-4F3E-80CD-1BAF03998C0B";
const SPOTIFY_PROVIDER_ID: &str = "BBAC49B7-0EDF-4D31-8A54-C9126509C2CE";
const ITUNES_PROVIDER_ID: &str = "36E4DB7A-18BB-4A7D-86F7-A3C653A21611";
const LAST_FM_PROVIDER_ID: &str = "3E43D998-2E9C-45B0-8128-EE6F0A41419E";
const DISCOGS_PROVIDER_ID: &str = "5A8D2E5B-3C9D-4E7F-9B7A-A1B2C3D4E5F6";
const WIKIDATA_PROVIDER_ID: &str = "7B9C8E1F-2A3D-4E5B-8C7D-9E0F1A2B3C4D";

fn provider_info(id: &str, display_name: &str, is_enabled: bool) -> ProviderInfo {
    ProviderInfo {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        is_enabled,
    }
}

fn provider_facts() -> Vec<ProviderInfo> {
    vec![
        provider_info(MELODEE_PROVIDER_ID, "Melodee Database", true),
        provider_info(MELODEE_PROVIDER_ID, "Music Brainz Database", true),
        provider_info(SPOTIFY_PROVIDER_ID, "Spotify Service", false),
        provider_info(ITUNES_PROVIDER_ID, "ITunes Search Engine", true),
        provider_info(LAST_FM_PROVIDER_ID, "Last FM Search Engine", true),
        provider_info(DISCOGS_PROVIDER_ID, "Discogs Search Engine", false),
        provider_info(WIKIDATA_PROVIDER_ID, "WikiData Search Engine", false),
    ]
}

fn provider_wire_facts(provider: &str) -> (&'static str, &'static str) {
    match provider.to_ascii_lowercase().as_str() {
        "musicbrainz" | "music_brainz" | "melodee" => {
            ("Music Brainz Database", MELODEE_PROVIDER_ID)
        }
        "spotify" => ("Spotify Service", SPOTIFY_PROVIDER_ID),
        "itunes" | "apple" => ("ITunes Search Engine", ITUNES_PROVIDER_ID),
        "lastfm" | "last_fm" => ("Last FM Search Engine", LAST_FM_PROVIDER_ID),
        "discogs" => ("Discogs Search Engine", DISCOGS_PROVIDER_ID),
        "wikidata" | "wiki_data" => ("WikiData Search Engine", WIKIDATA_PROVIDER_ID),
        _ => ("Melodee Database", MELODEE_PROVIDER_ID),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupRequest {
    artist_name: String,
    limit: Option<i32>,
    provider_ids: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LookupCandidate {
    provider_display_name: String,
    provider_id: Option<String>,
    name: String,
    sort_name: Option<String>,
    image_url: Option<String>,
    thumbnail_url: Option<String>,
    music_brainz_id: Option<Uuid>,
    spotify_id: Option<String>,
    discogs_id: Option<String>,
    amg_id: Option<String>,
    wiki_data_id: Option<String>,
    itunes_id: Option<String>,
    last_fm_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LookupResponse {
    candidates: Vec<LookupCandidate>,
    has_partial_failures: bool,
    providers: Vec<ProviderInfo>,
}

async fn providers(_pool: &sqlx::PgPool) -> Result<Vec<ProviderInfo>, NativeApiError> {
    // These are compatibility capability facts, not canonical identity
    // authorities. Disabled providers remain visible because clients use this
    // list to explain why a lookup source is unavailable.
    Ok(provider_facts())
}

async fn lookup_providers(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<Vec<ProviderInfo>>, NativeApiError> {
    Ok(Json(providers(&state.pool).await?))
}

async fn artist_lookup(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<LookupRequest>,
) -> Result<Json<LookupResponse>, NativeApiError> {
    let limit = request.limit.unwrap_or(20);
    if request.artist_name.trim().is_empty()
        || request.artist_name.chars().count() > 500
        || !(1..=100).contains(&limit)
    {
        return Err(NativeApiError::BadRequest);
    }
    let selected_providers = request.provider_ids.map(|ids| {
        ids.into_iter()
            .filter_map(|id| match id.to_ascii_uppercase().as_str() {
                MELODEE_PROVIDER_ID => Some("musicbrainz".to_owned()),
                SPOTIFY_PROVIDER_ID => Some("spotify".to_owned()),
                ITUNES_PROVIDER_ID => Some("itunes".to_owned()),
                LAST_FM_PROVIDER_ID => Some("lastfm".to_owned()),
                DISCOGS_PROVIDER_ID => Some("discogs".to_owned()),
                WIKIDATA_PROVIDER_ID => Some("wikidata".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    let rows=sqlx::query_as::<_,(String,String,String,String)>(
        "SELECT identity.provider,identity.external_id,artist.canonical_name,artist.sort_name \
         FROM artist_external_identities identity JOIN artists artist ON artist.id=identity.artist_id \
         WHERE artist.status IS DISTINCT FROM 'merged' AND artist.canonical_name ILIKE '%'||$1||'%' \
           AND native_user_can_access_artist($4,artist.id) \
           AND ($2::text[] IS NULL OR lower(identity.provider)=ANY($2)) \
         ORDER BY artist.sort_name,artist.public_api_id,identity.provider LIMIT $3")
        .bind(request.artist_name.trim()).bind(selected_providers).bind(i64::from(limit)).bind(principal.user_id)
        .fetch_all(&state.pool).await?;
    let candidates = rows
        .into_iter()
        .map(|(provider, external, name, sort)| {
            let (provider_display_name, provider_id) = provider_wire_facts(&provider);
            LookupCandidate {
                provider_display_name: provider_display_name.to_owned(),
                provider_id: Some(provider_id.to_owned()),
                name,
                sort_name: Some(sort),
                image_url: None,
                thumbnail_url: None,
                music_brainz_id: if provider.eq_ignore_ascii_case("musicbrainz") {
                    external.parse().ok()
                } else {
                    None
                },
                spotify_id: (provider.eq_ignore_ascii_case("spotify")).then(|| external.clone()),
                discogs_id: (provider.eq_ignore_ascii_case("discogs")).then(|| external.clone()),
                amg_id: None,
                wiki_data_id: (provider.eq_ignore_ascii_case("wikidata")).then(|| external.clone()),
                itunes_id: (provider.eq_ignore_ascii_case("itunes")).then(|| external.clone()),
                last_fm_id: (provider.eq_ignore_ascii_case("lastfm")).then_some(external),
            }
        })
        .collect();
    Ok(Json(LookupResponse {
        candidates,
        has_partial_failures: false,
        providers: providers(&state.pool).await?,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioFeatures {
    id: Uuid,
    tempo: f64,
    key: Option<String>,
    mode: Option<String>,
    time_signature: i32,
    acousticness: f64,
    danceability: f64,
    energy: f64,
    instrumentalness: f64,
    liveness: f64,
    loudness: f64,
    speechiness: f64,
    valence: f64,
}

async fn audio_features(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<AudioFeatures>, NativeApiError> {
    let row = sqlx::query_as::<
        _,
        (
            f64,
            Option<String>,
            Option<String>,
            i32,
            f64,
            f64,
            f64,
            f64,
            f64,
            f64,
            f64,
            f64,
        ),
    >(
        "SELECT feature.tempo,feature.musical_key,feature.mode,feature.time_signature,\
         feature.acousticness,feature.danceability,feature.energy,feature.instrumentalness,\
         feature.liveness,feature.loudness,feature.speechiness,feature.valence \
         FROM tracks track JOIN native_track_audio_features feature ON feature.track_id=track.id \
         WHERE track.public_api_id=$1
           AND native_user_can_access_release($2,track.release_id)",
    )
    .bind(id)
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    Ok(Json(AudioFeatures {
        id,
        tempo: row.0,
        key: row.1,
        mode: row.2,
        time_signature: row.3,
        acousticness: row.4,
        danceability: row.5,
        energy: row.6,
        instrumentalness: row.7,
        liveness: row.8,
        loudness: row.9,
        speechiness: row.10,
        valence: row.11,
    }))
}

#[derive(Debug, Deserialize)]
struct BpmParams {
    min: f64,
    max: f64,
    page: Option<i32>,
    limit: Option<i32>,
}
#[derive(Debug, Serialize)]
struct BpmTrack {
    id: Uuid,
    title: String,
    artist: String,
    bpm: f64,
}
#[derive(Debug, Serialize)]
struct BpmResponse {
    tracks: Vec<BpmTrack>,
    meta: PaginationMetadata,
}

async fn audio_by_bpm(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(q): Query<BpmParams>,
) -> Result<Json<BpmResponse>, NativeApiError> {
    let (page, size, offset) = bounded_page(q.page, q.limit, 50)?;
    if !q.min.is_finite() || !q.max.is_finite() || q.min < 0.0 || q.min > q.max {
        return Err(NativeApiError::BadRequest);
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, f64)>(
        "SELECT track.public_api_id,track.title,\
         COALESCE(track.display_artist,release.display_artist,'Unknown Artist'),feature.tempo \
         FROM native_track_audio_features feature JOIN tracks track ON track.id=feature.track_id \
         JOIN releases release ON release.id=track.release_id WHERE feature.tempo BETWEEN $1 AND $2 \
           AND native_user_can_access_release($5,track.release_id) \
         ORDER BY feature.tempo,track.public_api_id LIMIT $3 OFFSET $4",
    ).bind(q.min).bind(q.max).bind(i64::from(size)).bind(offset).bind(principal.user_id).fetch_all(&state.pool).await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_track_audio_features feature
         JOIN tracks track ON track.id=feature.track_id
         WHERE feature.tempo BETWEEN $1 AND $2
           AND native_user_can_access_release($3,track.release_id)",
    )
    .bind(q.min)
    .bind(q.max)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    let tracks = rows
        .into_iter()
        .map(|(id, title, artist, bpm)| BpmTrack {
            id,
            title,
            artist,
            bpm,
        })
        .collect();
    Ok(Json(BpmResponse {
        tracks,
        meta: PaginationMetadata::new(
            total,
            u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?,
            u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?,
        )
        .map_err(|_| NativeApiError::BadRequest)?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartParams {
    page: Option<i32>,
    page_size: Option<i32>,
    year: Option<i32>,
    source: Option<String>,
    tags: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ChartList {
    id: i32,
    api_key: Uuid,
    slug: String,
    title: String,
    source_name: Option<String>,
    source_url: Option<String>,
    year: Option<i32>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    item_count: i32,
    is_generated_playlist_enabled: bool,
    playlist_id: Option<String>,
    image_url: Option<String>,
}
#[derive(Debug, Serialize)]
struct ChartPage {
    meta: PaginationMetadata,
    data: Vec<ChartList>,
}

async fn charts(
    State(state): State<NativeState>,
    _: Authenticated,
    Query(q): Query<ChartParams>,
) -> Result<Json<ChartPage>, NativeApiError> {
    let (page, size, offset) = bounded_page(q.page, q.page_size, 20)?;
    let tags = q.tags.unwrap_or_default();
    let year = q.year;
    let source = q.source;
    let rows=sqlx::query_as::<_,ChartList>("SELECT chart.legacy_numeric_id AS id,chart.public_api_id AS api_key,chart.slug,chart.title,chart.source_name,chart.source_url,chart.chart_year AS year,chart.description,NULLIF(chart.tags,'{}') AS tags,(SELECT count(*)::integer FROM native_chart_items item WHERE item.chart_id=chart.id) AS item_count,chart.is_generated_playlist_enabled,CASE WHEN chart.is_generated_playlist_enabled THEN 'chart:'||chart.legacy_numeric_id::text END AS playlist_id,$1||'/api/v1/charts/'||chart.public_api_id::text||'/image' AS image_url FROM native_charts chart WHERE chart.is_visible AND ($2::integer IS NULL OR chart.chart_year=$2) AND ($3::text IS NULL OR chart.source_name=$3) AND ($4::text[]='{}' OR chart.tags @> $4) ORDER BY chart.created_at DESC,chart.public_api_id LIMIT $5 OFFSET $6")
        .bind(&state.public_origin).bind(year).bind(&source).bind(&tags).bind(i64::from(size)).bind(offset).fetch_all(&state.pool).await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_charts chart WHERE chart.is_visible \
         AND ($1::integer IS NULL OR chart.chart_year=$1) \
         AND ($2::text IS NULL OR chart.source_name=$2) \
         AND ($3::text[]='{}' OR chart.tags @> $3)",
    )
    .bind(year)
    .bind(source)
    .bind(tags)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ChartPage {
        meta: PaginationMetadata::new(
            total,
            u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?,
            u16::try_from(size).map_err(|_| NativeApiError::BadRequest)?,
        )
        .map_err(|_| NativeApiError::BadRequest)?,
        data: rows,
    }))
}

async fn chart_record(
    pool: &sqlx::PgPool,
    origin: &str,
    id: &str,
) -> Result<ChartList, NativeApiError> {
    sqlx::query_as::<_,ChartList>("SELECT chart.legacy_numeric_id AS id,chart.public_api_id AS api_key,chart.slug,chart.title,chart.source_name,chart.source_url,chart.chart_year AS year,chart.description,NULLIF(chart.tags,'{}') AS tags,(SELECT count(*)::integer FROM native_chart_items item WHERE item.chart_id=chart.id) AS item_count,chart.is_generated_playlist_enabled,CASE WHEN chart.is_generated_playlist_enabled THEN 'chart:'||chart.legacy_numeric_id::text END AS playlist_id,$1||'/api/v1/charts/'||chart.public_api_id::text||'/image' AS image_url FROM native_charts chart WHERE chart.is_visible AND (chart.slug=$2 OR chart.public_api_id::text=$2 OR chart.legacy_numeric_id::text=$2)")
        .bind(origin).bind(id).fetch_optional(pool).await?.ok_or(NativeApiError::NotFound)
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ChartItem {
    id: i32,
    rank: i32,
    artist_name: String,
    album_title: String,
    release_year: Option<i32>,
    link_status: String,
    linked_artist_id: Option<i32>,
    linked_artist_api_key: Option<Uuid>,
    linked_artist_name: Option<String>,
    linked_album_id: Option<i32>,
    linked_album_api_key: Option<Uuid>,
    linked_album_name: Option<String>,
    album_image_url: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChartDetail {
    items: Vec<ChartItem>,
    #[serde(flatten)]
    chart: ChartList,
}

async fn chart(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<String>,
) -> Result<Json<ChartDetail>, NativeApiError> {
    let chart = chart_record(&state.pool, &state.public_origin, &id).await?;
    let items=sqlx::query_as::<_,ChartItem>("SELECT item.rank AS id,item.rank,item.artist_name,item.release_title AS album_title,item.release_year,CASE WHEN release.id IS NULL THEN 'Unlinked' ELSE 'Linked' END AS link_status,NULL::integer AS linked_artist_id,artist.public_api_id AS linked_artist_api_key,artist.canonical_name AS linked_artist_name,NULL::integer AS linked_album_id,release.public_api_id AS linked_album_api_key,release.title AS linked_album_name,CASE WHEN release.id IS NULL THEN NULL ELSE $2||'/images/album_'||release.public_api_id::text||'/80' END AS album_image_url FROM native_chart_items item LEFT JOIN releases release ON release.id=item.linked_release_id AND release.status='active' AND native_user_can_access_release($3,release.id) LEFT JOIN artists artist ON artist.id=item.linked_artist_id AND release.id IS NOT NULL AND native_user_can_access_artist($3,artist.id) WHERE item.chart_id=(SELECT id FROM native_charts WHERE public_api_id=$1) ORDER BY item.rank")
        .bind(chart.api_key).bind(&state.public_origin).bind(principal.user_id).fetch_all(&state.pool).await?;
    Ok(Json(ChartDetail { items, chart }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChartPlaylist {
    chart_id: i32,
    chart_slug: String,
    chart_title: String,
    playlist_id: String,
    track_count: i32,
    tracks: Vec<ChartTrack>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ChartTrack {
    chart_rank: i32,
    song_id: i32,
    song_api_key: Uuid,
    song_title: String,
    album_id: i32,
    album_api_key: Uuid,
    album_name: String,
    artist_id: i32,
    artist_api_key: Uuid,
    artist_name: String,
    song_number: i32,
    duration: f64,
    #[serde(rename = "coverUrl")]
    cover_art_url: Option<String>,
}
async fn chart_playlist(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<String>,
) -> Result<Json<ChartPlaylist>, NativeApiError> {
    let chart = chart_record(&state.pool, &state.public_origin, &id).await?;
    if !chart.is_generated_playlist_enabled {
        return Err(NativeApiError::CapturedBadRequest(
            "Generated playlist is not enabled for this chart.",
        ));
    }
    let tracks = sqlx::query_as::<_, ChartTrack>(
        "SELECT item.rank AS chart_rank,\
         COALESCE((SELECT mapping.legacy_numeric_id::integer FROM legacy_catalog_id_mappings mapping \
           WHERE mapping.entity_type='track' AND mapping.internal_id=track.id \
             AND mapping.legacy_numeric_id BETWEEN 0 AND 2147483647 \
           ORDER BY mapping.imported_at DESC,mapping.id DESC LIMIT 1),0) AS song_id,\
         track.public_api_id AS song_api_key,track.title AS song_title,\
         COALESCE((SELECT mapping.legacy_numeric_id::integer FROM legacy_catalog_id_mappings mapping \
           WHERE mapping.entity_type='release' AND mapping.internal_id=release.id \
             AND mapping.legacy_numeric_id BETWEEN 0 AND 2147483647 \
           ORDER BY mapping.imported_at DESC,mapping.id DESC LIMIT 1),0) AS album_id,\
         release.public_api_id AS album_api_key,release.title AS album_name,\
         COALESCE((SELECT mapping.legacy_numeric_id::integer FROM legacy_catalog_id_mappings mapping \
           WHERE mapping.entity_type='artist' AND mapping.internal_id=artist.id \
             AND mapping.legacy_numeric_id BETWEEN 0 AND 2147483647 \
           ORDER BY mapping.imported_at DESC,mapping.id DESC LIMIT 1),0) AS artist_id,\
         artist.public_api_id AS artist_api_key,artist.canonical_name AS artist_name,\
         COALESCE(track.track_number,track.ordinal) AS song_number,\
         COALESCE(track.duration_ms,0)::double precision/1000 AS duration,\
         $2||'/images/album_'||release.public_api_id::text||'/80' AS cover_art_url \
         FROM native_chart_items item JOIN releases release ON release.id=item.linked_release_id AND release.status='active' \
         JOIN tracks track ON track.release_id=release.id \
         JOIN LATERAL (SELECT billing.artist_id FROM release_artists billing \
           WHERE billing.release_id=release.id AND billing.role='primary' \
           ORDER BY billing.presentation_order,billing.id LIMIT 1) primary_artist ON true \
         JOIN artists artist ON artist.id=primary_artist.artist_id \
         WHERE item.chart_id=(SELECT id FROM native_charts WHERE public_api_id=$1) \
           AND native_user_can_access_release($4,release.id) \
         ORDER BY item.rank,track.ordinal,track.public_api_id LIMIT $3",
    )
    .bind(chart.api_key)
    .bind(&state.public_origin)
    .bind(MAX_CHART_PLAYLIST_TRACKS + 1)
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    if i64::try_from(tracks.len()).unwrap_or(i64::MAX) > MAX_CHART_PLAYLIST_TRACKS {
        return Err(NativeApiError::CapturedBadRequest(
            "Generated playlist exceeds the supported Track limit.",
        ));
    }
    let track_count = i32::try_from(tracks.len()).map_err(|_| NativeApiError::Unavailable)?;
    Ok(Json(ChartPlaylist {
        chart_id: chart.id,
        chart_slug: chart.slug,
        chart_title: chart.title,
        playlist_id: format!("chart:{}", chart.id),
        track_count,
        tracks,
    }))
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct BackendStatus {
    is_playing: bool,
    position_seconds: f64,
    volume: Option<f64>,
    current_item_api_key: Option<Uuid>,
    is_connected: bool,
    status_message: Option<String>,
    error_message: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendCapabilities {
    can_play: bool,
    can_pause: bool,
    can_stop: bool,
    can_seek: bool,
    can_skip: bool,
    can_set_volume: bool,
    can_report_position: bool,
    is_available: bool,
    backend_info: Option<String>,
}

async fn backend_status(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<BackendStatus>, NativeApiError> {
    let row=sqlx::query_as::<_,BackendStatus>("SELECT backend.is_playing,backend.position_seconds,backend.volume,track.public_api_id AS current_item_api_key,backend.initialized AS is_connected,CASE WHEN backend.initialized THEN 'Ready' ELSE 'Stopped' END AS status_message,NULL::text AS error_message FROM native_playback_backend_state backend LEFT JOIN tracks track ON track.id=backend.current_track_id WHERE singleton AND enabled")
        .fetch_optional(&state.pool).await?.ok_or(NativeApiError::CapturedBadRequest("Jukebox is not enabled"))?;
    Ok(Json(row))
}
async fn backend_capabilities(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<BackendCapabilities>, NativeApiError> {
    let enabled=sqlx::query_scalar::<_,bool>("SELECT COALESCE((SELECT enabled FROM native_playback_backend_state WHERE singleton),false)").fetch_one(&state.pool).await?;
    if !enabled {
        return Err(NativeApiError::CapturedBadRequest("Jukebox is not enabled"));
    }
    Ok(Json(BackendCapabilities {
        can_play: enabled,
        can_pause: enabled,
        can_stop: enabled,
        can_seek: enabled,
        can_skip: enabled,
        can_set_volume: enabled,
        can_report_position: enabled,
        is_available: enabled,
        backend_info: enabled.then(|| "Melodee playback backend".to_owned()),
    }))
}
async fn backend_initialize(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<bool>, NativeApiError> {
    let changed=sqlx::query("UPDATE native_playback_backend_state SET initialized=true,updated_at=now() WHERE singleton AND enabled").execute(&state.pool).await?.rows_affected();
    if changed == 0 {
        return Err(NativeApiError::CapturedBadRequest("Jukebox is not enabled"));
    }
    Ok(Json(true))
}
async fn backend_shutdown(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<bool>, NativeApiError> {
    sqlx::query("UPDATE native_playback_backend_state SET initialized=false,is_playing=false,updated_at=now() WHERE singleton").execute(&state.pool).await?;
    Ok(Json(true))
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct EndpointDto {
    api_key: Uuid,
    name: String,
    r#type: String,
    is_shared: bool,
    room: Option<String>,
    last_seen_at: Option<String>,
    capabilities_json: Option<String>,
    is_owner: bool,
}
async fn backend_register(
    State(state): State<NativeState>,
    _: Authenticated,
) -> Result<Json<EndpointDto>, NativeApiError> {
    let enabled=sqlx::query_scalar::<_,bool>("SELECT COALESCE((SELECT enabled FROM native_playback_backend_state WHERE singleton),false)").fetch_one(&state.pool).await?;
    if !enabled {
        return Err(NativeApiError::CapturedBadRequest("Jukebox is not enabled"));
    }
    let id = Uuid::now_v7();
    let row=sqlx::query_as::<_,EndpointDto>("INSERT INTO native_playback_endpoints(id,public_api_id,owner_user_id,name,endpoint_type,is_shared,capabilities_json,last_seen_at,created_at,updated_at) VALUES($1,$1,NULL,'MPV Backend','MpvBackend',true,'{\"canPlay\":true,\"canPause\":true,\"canSkip\":true,\"canSeek\":true,\"canSetVolume\":true,\"canReportPosition\":true}',now(),now(),now()) ON CONFLICT(endpoint_type) WHERE owner_user_id IS NULL DO UPDATE SET last_seen_at=now(),updated_at=now() RETURNING public_api_id AS api_key,name,endpoint_type AS type,is_shared,room,last_seen_at::text,capabilities_json,false AS is_owner")
        .bind(id).fetch_one(&state.pool).await?;
    Ok(Json(row))
}
async fn backend_heartbeat(
    State(state): State<NativeState>,
    _: Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<bool>, NativeApiError> {
    let changed=sqlx::query("UPDATE native_playback_backend_state SET last_heartbeat_at=now(),updated_at=now() WHERE singleton AND session_public_api_id=$1 AND initialized").bind(id).execute(&state.pool).await?.rows_affected();
    if changed == 0 {
        return Err(NativeApiError::NotFound);
    }
    Ok(Json(true))
}

#[derive(Debug, Deserialize)]
struct RecommendationParams {
    limit: Option<i32>,
    r#type: Option<String>,
    category: Option<String>,
}
#[derive(Debug, Serialize)]
struct RecommendationItem {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    artist: Option<String>,
    reason: Option<String>,
    #[serde(rename = "imageUrl")]
    image_url: Option<String>,
}
#[derive(Debug, Serialize)]
struct Recommendations {
    recommendations: Vec<RecommendationItem>,
    category: Option<String>,
}
async fn recommendations(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(q): Query<RecommendationParams>,
) -> Result<Json<Recommendations>, NativeApiError> {
    let limit = q.limit.unwrap_or(20);
    if !(1..=100).contains(&limit)
        || q.r#type
            .as_deref()
            .is_some_and(|v| !matches!(v, "song" | "album" | "artist"))
    {
        return Err(NativeApiError::BadRequest);
    }
    let category = q.category.unwrap_or_else(|| "discover".to_owned());
    if !matches!(
        category.as_str(),
        "discover" | "similar" | "missed" | "based_on_recent"
    ) {
        return Err(NativeApiError::BadRequest);
    }
    let reason = |kind: &str| -> String {
        match (category.as_str(), kind) {
            ("discover", "song") => "Popular among all users",
            ("discover", "album") => "Popular album to explore",
            ("discover", "artist") => "Popular artist to discover",
            ("similar", _) => "Similar to your favorites",
            ("missed", _) => "You might have missed this",
            ("based_on_recent", _) => "Based on your recent listening",
            _ => "Recommended for you",
        }
        .to_owned()
    };
    let mut recommendations = Vec::new();
    let requested_limit = usize::try_from(limit).map_err(|_| NativeApiError::BadRequest)?;
    let requested_type = q.r#type.as_deref();
    let family_limit = requested_limit.saturating_sub(recommendations.len());
    if family_limit > 0 && (requested_type.is_none() || requested_type == Some("song")) {
        let rows = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
            "WITH candidate AS MATERIALIZED ( \
               SELECT entity_id,popularity FROM native_recommendation_candidates \
               WHERE entity_type='track' \
               ORDER BY popularity DESC,entity_id LIMIT $4 \
             ) \
             SELECT track.public_api_id,track.title,COALESCE(track.display_artist,release.display_artist) \
             FROM candidate
             JOIN tracks track ON track.id=candidate.entity_id
             JOIN releases release ON release.id=track.release_id \
             LEFT JOIN native_track_user_states own ON own.track_id=track.id AND own.user_id=$1 \
             WHERE release.status='active'
               AND native_user_can_access_release($1,release.id) \
               AND CASE $2 \
                 WHEN 'discover' THEN own.track_id IS NULL OR own.play_count=0 \
                 WHEN 'missed' THEN (own.track_id IS NULL OR own.play_count=0) AND EXISTS(\
                   SELECT 1 FROM track_artists billing JOIN native_artist_user_states state \
                     ON state.artist_id=billing.artist_id AND state.user_id=$1 AND state.starred \
                   WHERE billing.track_id=track.id) \
                 WHEN 'similar' THEN EXISTS(SELECT 1 FROM track_artists candidate \
                   JOIN track_artists heard ON heard.artist_id=candidate.artist_id \
                   JOIN native_track_plays play ON play.track_id=heard.track_id AND play.user_id=$1 \
                   WHERE candidate.track_id=track.id) \
                 WHEN 'based_on_recent' THEN EXISTS(SELECT 1 FROM track_artists candidate \
                   JOIN track_artists heard ON heard.artist_id=candidate.artist_id \
                   JOIN native_track_plays play ON play.track_id=heard.track_id AND play.user_id=$1 \
                   WHERE candidate.track_id=track.id AND play.played_at>=now()-interval '30 days') \
                 ELSE false END \
             ORDER BY candidate.popularity DESC,candidate.entity_id,
               COALESCE(own.rating,0) DESC,COALESCE(own.play_count,0) DESC,
               track.created_at DESC,track.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(&category)
        .bind(i64::try_from(family_limit).map_err(|_| NativeApiError::BadRequest)?)
        .bind(MAX_RECOMMENDATION_CANDIDATES_SCANNED)
        .fetch_all(&state.pool)
        .await?;
        ensure_recommendation_window_complete(&state.pool, "track", rows.len(), family_limit)
            .await?;
        recommendations.extend(
            rows.into_iter()
                .map(|(id, name, artist)| RecommendationItem {
                    id,
                    name,
                    r#type: "song".to_owned(),
                    artist,
                    reason: Some(reason("song")),
                    image_url: Some(format!(
                        "{}/images/song_{id}/80",
                        state.public_origin.trim_end_matches('/')
                    )),
                }),
        );
    }
    let family_limit = requested_limit.saturating_sub(recommendations.len());
    if family_limit > 0 && (requested_type.is_none() || requested_type == Some("album")) {
        let rows = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
            "WITH candidate AS MATERIALIZED ( \
               SELECT entity_id,popularity FROM native_recommendation_candidates \
               WHERE entity_type='release' \
               ORDER BY popularity DESC,entity_id LIMIT $4 \
             ) \
             SELECT release.public_api_id,release.title,release.display_artist \
             FROM candidate
             JOIN releases release ON release.id=candidate.entity_id
             LEFT JOIN native_release_user_states own \
               ON own.release_id=release.id AND own.user_id=$1 \
             WHERE release.status='active'
               AND native_user_can_access_release($1,release.id) \
               AND CASE $2 WHEN 'discover' THEN own.release_id IS NULL OR own.play_count=0 \
                 WHEN 'missed' THEN (own.release_id IS NULL OR own.play_count=0) AND EXISTS(\
                   SELECT 1 FROM release_artists billing JOIN native_artist_user_states state \
                     ON state.artist_id=billing.artist_id AND state.user_id=$1 AND state.starred \
                   WHERE billing.release_id=release.id) \
                 WHEN 'similar' THEN EXISTS(SELECT 1 FROM release_artists candidate \
                   JOIN track_artists heard ON heard.artist_id=candidate.artist_id \
                   JOIN native_track_plays play ON play.track_id=heard.track_id AND play.user_id=$1 \
                   WHERE candidate.release_id=release.id) \
                 WHEN 'based_on_recent' THEN EXISTS(SELECT 1 FROM tracks heard \
                   JOIN native_track_plays play ON play.track_id=heard.id AND play.user_id=$1 \
                   WHERE heard.release_id=release.id AND play.played_at>=now()-interval '30 days') \
                 ELSE false END \
             ORDER BY candidate.popularity DESC,candidate.entity_id,
               COALESCE(own.rating,0) DESC,COALESCE(own.play_count,0) DESC,
               release.created_at DESC,release.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(&category)
        .bind(i64::try_from(family_limit).map_err(|_| NativeApiError::BadRequest)?)
        .bind(MAX_RECOMMENDATION_CANDIDATES_SCANNED)
        .fetch_all(&state.pool)
        .await?;
        ensure_recommendation_window_complete(&state.pool, "release", rows.len(), family_limit)
            .await?;
        recommendations.extend(
            rows.into_iter()
                .map(|(id, name, artist)| RecommendationItem {
                    id,
                    name,
                    r#type: "album".to_owned(),
                    artist,
                    reason: Some(reason("album")),
                    image_url: Some(format!(
                        "{}/images/album_{id}/80",
                        state.public_origin.trim_end_matches('/')
                    )),
                }),
        );
    }
    let family_limit = requested_limit.saturating_sub(recommendations.len());
    if family_limit > 0 && (requested_type.is_none() || requested_type == Some("artist")) {
        let rows = sqlx::query_as::<_, (Uuid, String)>(
            "WITH candidate AS MATERIALIZED ( \
               SELECT entity_id,popularity FROM native_recommendation_candidates \
               WHERE entity_type='artist' \
               ORDER BY popularity DESC,entity_id LIMIT $4 \
             ) \
             SELECT artist.public_api_id,artist.canonical_name
             FROM candidate
             JOIN artists artist ON artist.id=candidate.entity_id \
             LEFT JOIN native_artist_user_states own ON own.artist_id=artist.id AND own.user_id=$1 \
             WHERE artist.status IS DISTINCT FROM 'merged'
               AND (EXISTS(SELECT 1 FROM release_artists billing \
                 JOIN releases visible_release ON visible_release.id=billing.release_id AND visible_release.status='active' \
                 WHERE billing.artist_id=artist.id AND native_user_can_access_release($1,visible_release.id)) \
               OR EXISTS(SELECT 1 FROM track_artists billing JOIN tracks candidate ON candidate.id=billing.track_id \
                 JOIN releases visible_release ON visible_release.id=candidate.release_id AND visible_release.status='active' \
                 WHERE billing.artist_id=artist.id AND native_user_can_access_release($1,visible_release.id))) \
               AND CASE $2 WHEN 'discover' THEN own.artist_id IS NULL OR own.play_count=0 \
                 WHEN 'missed' THEN own.starred \
                 WHEN 'similar' THEN own.starred OR own.rating>0 \
                 WHEN 'based_on_recent' THEN EXISTS(SELECT 1 FROM track_artists billing \
                   JOIN tracks heard ON heard.id=billing.track_id \
                   JOIN releases visible_release ON visible_release.id=heard.release_id AND visible_release.status='active' \
                   JOIN native_track_plays play ON play.track_id=heard.id AND play.user_id=$1 \
                   WHERE billing.artist_id=artist.id AND native_user_can_access_release($1,visible_release.id) \
                     AND play.played_at>=now()-interval '30 days') \
                 ELSE false END \
             ORDER BY candidate.popularity DESC,candidate.entity_id,
               COALESCE(own.rating,0) DESC,COALESCE(own.play_count,0) DESC,
               artist.created_at DESC,artist.public_api_id LIMIT $3",
        )
        .bind(principal.user_id)
        .bind(&category)
        .bind(i64::try_from(family_limit).map_err(|_| NativeApiError::BadRequest)?)
        .bind(MAX_RECOMMENDATION_CANDIDATES_SCANNED)
        .fetch_all(&state.pool)
        .await?;
        ensure_recommendation_window_complete(&state.pool, "artist", rows.len(), family_limit)
            .await?;
        recommendations.extend(rows.into_iter().map(|(id, name)| RecommendationItem {
            id,
            name,
            r#type: "artist".to_owned(),
            artist: None,
            reason: Some(reason("artist")),
            image_url: Some(format!(
                "{}/images/artist_{id}/80",
                state.public_origin.trim_end_matches('/')
            )),
        }));
    }
    recommendations.truncate(requested_limit);
    Ok(Json(Recommendations {
        recommendations,
        category: Some(category),
    }))
}

async fn ensure_recommendation_window_complete(
    pool: &sqlx::PgPool,
    entity_type: &str,
    eligible_count: usize,
    requested_limit: usize,
) -> Result<(), NativeApiError> {
    if eligible_count >= requested_limit {
        return Ok(());
    }
    let has_more = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
           SELECT candidate.entity_id FROM native_recommendation_candidates candidate \
           WHERE candidate.entity_type=$1 \
           ORDER BY candidate.popularity DESC,candidate.entity_id \
           OFFSET $2 LIMIT 1)",
    )
    .bind(entity_type)
    .bind(MAX_RECOMMENDATION_CANDIDATES_SCANNED)
    .fetch_one(pool)
    .await?;
    if has_more {
        tracing::warn!(
            entity_type,
            scanned = MAX_RECOMMENDATION_CANDIDATES_SCANNED,
            requested_limit,
            eligible_count,
            "native recommendation candidate scan budget exhausted"
        );
        return Err(NativeApiError::Unavailable);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Statistic {
    r#type: i32,
    title: &'static str,
    data: String,
    description: Option<&'static str>,
    sort_order: Option<i16>,
}
async fn system_stats(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Json<Vec<Statistic>>, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::NotFound);
    }
    let(artists,releases,playlists,tracks)=sqlx::query_as::<_,(i64,i64,i64,i64)>("SELECT (SELECT count(*) FROM artists WHERE status IS DISTINCT FROM 'merged'),(SELECT count(*) FROM releases WHERE status='active'),(SELECT count(*) FROM native_playlists),(SELECT count(*) FROM tracks)").fetch_one(&state.pool).await?;
    Ok(Json(vec![
        Statistic {
            r#type: 4,
            title: "Albums",
            data: releases.to_string(),
            description: None,
            sort_order: Some(1),
        },
        Statistic {
            r#type: 4,
            title: "Artists",
            data: artists.to_string(),
            description: None,
            sort_order: Some(2),
        },
        Statistic {
            r#type: 4,
            title: "Playlists",
            data: playlists.to_string(),
            description: None,
            sort_order: Some(6),
        },
        Statistic {
            r#type: 4,
            title: "Songs",
            data: tracks.to_string(),
            description: None,
            sort_order: Some(9),
        },
    ]))
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct ThemeInfo {
    id: String,
    name: String,
    author: Option<String>,
    version: Option<String>,
    description: Option<String>,
    is_built_in: bool,
    preview_image: Option<String>,
    has_warnings: bool,
    warning_details: Option<Vec<String>>,
}
async fn themes(State(state): State<NativeState>) -> Result<Json<Vec<ThemeInfo>>, NativeApiError> {
    Ok(Json(sqlx::query_as::<_,ThemeInfo>("SELECT id,name,author,version,description,is_built_in,preview_image,has_warnings,CASE WHEN warning_details IS NULL THEN NULL ELSE ARRAY[warning_details] END AS warning_details FROM native_themes ORDER BY is_built_in DESC,CASE id WHEN 'light' THEN 0 WHEN 'dark' THEN 1 ELSE 2 END,name,id LIMIT 1000").fetch_all(&state.pool).await?))
}

async fn rescan_themes(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Json<Vec<ThemeInfo>>, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::NotFound);
    }
    themes(State(state)).await
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeChoice {
    theme_id: String,
}
async fn set_theme(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(choice): Json<ThemeChoice>,
) -> Result<Json<serde_json::Value>, NativeApiError> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM native_themes WHERE id=$1)")
            .bind(&choice.theme_id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(NativeApiError::NotFound);
    }
    sqlx::query("UPDATE native_users SET preferred_theme=$2,auth_revision=auth_revision+1,updated_at=now() WHERE id=$1").bind(principal.user_id).bind(&choice.theme_id).execute(&state.pool).await?;
    Ok(Json(
        serde_json::json!({"message":"Theme preference updated","themeId":choice.theme_id}),
    ))
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeMetadata {
    id: String,
    name: String,
    author: Option<String>,
    version: Option<String>,
    description: Option<String>,
    preview_image: Option<String>,
}

#[derive(Debug)]
struct ParsedThemeArchive {
    metadata: ThemeMetadata,
    assets: BTreeMap<String, (String, Vec<u8>)>,
}

fn valid_theme_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && id.len() <= 128
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn theme_asset_mime(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension)?;
    match extension.to_ascii_lowercase().as_str() {
        "css" => Some("text/css"),
        "json" => Some("application/json"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "ico" => Some("image/x-icon"),
        "svg" => Some("image/svg+xml"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        _ => None,
    }
}

fn safe_theme_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1000
        && !name.starts_with('/')
        && !name.contains('\\')
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn parse_theme_archive(bytes: Vec<u8>) -> Result<ParsedThemeArchive, &'static str> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Invalid ZIP archive")?;
    if archive.is_empty() || archive.len() > MAX_THEME_ENTRIES {
        return Err("Theme archive contains an invalid number of entries");
    }
    let mut expanded = 0_u64;
    let mut assets = BTreeMap::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| "Invalid ZIP archive entry")?;
        if entry.encrypted() {
            return Err("Encrypted theme archives are not supported");
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err("Unsupported ZIP compression method");
        }
        if entry.is_dir() {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 != 0o100_000)
        {
            return Err("Theme archive links and special files are not allowed");
        }
        let name = entry.name().to_owned();
        if entry.enclosed_name().is_none() || !safe_theme_entry_name(&name) {
            return Err("Theme archive contains an unsafe path");
        }
        let mime =
            theme_asset_mime(&name).ok_or("Theme archive contains a disallowed file type")?;
        if entry.size() == 0 || entry.size() > MAX_THEME_ASSET_BYTES {
            return Err("Theme archive entry exceeds the size limit");
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or("Theme archive expanded size is invalid")?;
        if expanded > MAX_THEME_EXPANDED_BYTES {
            return Err("Theme archive exceeds the expanded size limit");
        }
        let declared_size = entry.size();
        let read_limit = declared_size
            .checked_add(1)
            .ok_or("Theme archive entry size is invalid")?;
        let mut content = Vec::with_capacity(
            usize::try_from(declared_size).map_err(|_| "Theme archive entry is too large")?,
        );
        entry
            .take(read_limit)
            .read_to_end(&mut content)
            .map_err(|_| "Theme archive entry could not be read")?;
        if u64::try_from(content.len()).map_err(|_| "Theme archive entry is too large")?
            != declared_size
        {
            return Err("Theme archive entry size does not match its declaration");
        }
        if assets.insert(name, (mime.to_owned(), content)).is_some() {
            return Err("Theme archive contains duplicate paths");
        }
    }
    let metadata_bytes = assets
        .get("theme.json")
        .map(|(_, bytes)| bytes)
        .ok_or("Required file 'theme.json' not found")?;
    if !assets.contains_key("theme.css") {
        return Err("Required file 'theme.css' not found");
    }
    let metadata: ThemeMetadata =
        serde_json::from_slice(metadata_bytes).map_err(|_| "Invalid theme.json format")?;
    if !valid_theme_id(&metadata.id)
        || metadata.name.trim().is_empty()
        || metadata.name.chars().count() > 500
        || metadata
            .author
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500)
        || metadata
            .version
            .as_ref()
            .is_some_and(|value| value.chars().count() > 100)
        || metadata
            .description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 10_000)
        || metadata
            .preview_image
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500 || !safe_theme_entry_name(value))
    {
        return Err("Theme metadata missing or invalid");
    }
    if metadata
        .preview_image
        .as_ref()
        .is_some_and(|path| !assets.contains_key(path))
    {
        return Err("Theme preview image is missing from the archive");
    }
    Ok(ParsedThemeArchive { metadata, assets })
}

async fn theme_import(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    mut multipart: Multipart,
) -> Result<Response, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::NotFound);
    }
    let mut archive_bytes = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| NativeApiError::BadRequest)?
    {
        if field.name() != Some("file") {
            continue;
        }
        if archive_bytes.is_some()
            || !field
                .file_name()
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".zip"))
        {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":"File must be a .zip archive"})),
            )
                .into_response());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| NativeApiError::BadRequest)?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_THEME_ARCHIVE_BYTES {
                return Ok((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error":"Theme pack exceeds maximum size of 50MB"})),
                )
                    .into_response());
            }
            bytes.extend_from_slice(&chunk);
        }
        archive_bytes = Some(bytes);
    }
    let Some(bytes) = archive_bytes.filter(|bytes| !bytes.is_empty()) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":"No file provided"})),
        )
            .into_response());
    };
    let parsed = match tokio::task::spawn_blocking(move || parse_theme_archive(bytes))
        .await
        .map_err(|_| NativeApiError::Unavailable)?
    {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":error})),
            )
                .into_response());
        }
    };
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM native_themes WHERE id=$1)")
            .bind(&parsed.metadata.id)
            .fetch_one(&state.pool)
            .await?;
    if exists {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":format!("Theme '{}' already exists",parsed.metadata.id)})),
        )
            .into_response());
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO native_themes(id,name,author,version,description,is_built_in,preview_image,\
         has_warnings,created_at,updated_at) VALUES($1,$2,$3,$4,$5,false,$6,false,now(),now())",
    )
    .bind(&parsed.metadata.id)
    .bind(parsed.metadata.name.trim())
    .bind(parsed.metadata.author.as_deref().unwrap_or("Unknown"))
    .bind(parsed.metadata.version.as_deref().unwrap_or("1.0.0"))
    .bind(parsed.metadata.description.as_deref())
    .bind(parsed.metadata.preview_image.as_deref())
    .execute(&mut *tx)
    .await?;
    for (path, (mime, bytes)) in parsed.assets {
        sqlx::query(
            "INSERT INTO native_theme_assets(theme_id,relative_path,mime_type,bytes) \
             VALUES($1,$2,$3,$4)",
        )
        .bind(&parsed.metadata.id)
        .bind(path)
        .bind(mime)
        .bind(bytes)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "themeId":parsed.metadata.id,
        "message":format!("Theme '{}' imported successfully",parsed.metadata.id)
    }))
    .into_response())
}

fn build_theme_zip(assets: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, ()> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (path, bytes) in assets {
        writer.start_file(path, options).map_err(|_| ())?;
        writer.write_all(&bytes).map_err(|_| ())?;
    }
    Ok(writer.finish().map_err(|_| ())?.into_inner())
}

async fn theme_export(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<String>,
) -> Result<Response, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::NotFound);
    }
    let assets = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT asset.relative_path,asset.bytes FROM native_themes theme \
         JOIN native_theme_assets asset ON asset.theme_id=theme.id \
         WHERE theme.id=$1 ORDER BY asset.relative_path",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    if assets.is_empty() || assets.len() > MAX_THEME_ENTRIES {
        return Err(NativeApiError::NotFound);
    }
    let expanded = assets.iter().try_fold(0_usize, |total, (_, bytes)| {
        total
            .checked_add(bytes.len())
            .filter(|sum| *sum <= usize::try_from(MAX_THEME_EXPANDED_BYTES).unwrap_or(usize::MAX))
    });
    if expanded.is_none() {
        return Err(NativeApiError::Unavailable);
    }
    let bytes = tokio::task::spawn_blocking(move || build_theme_zip(assets))
        .await
        .map_err(|_| NativeApiError::Unavailable)?
        .map_err(|()| NativeApiError::Unavailable)?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{id}.zip\""))
        .map_err(|_| NativeApiError::Unavailable)?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    Ok(response)
}
async fn theme_delete(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, NativeApiError> {
    if !principal.is_admin {
        return Err(NativeApiError::NotFound);
    }
    let changed = sqlx::query("DELETE FROM native_themes WHERE id=$1 AND NOT is_built_in")
        .bind(&id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(NativeApiError::BadRequest);
    }
    Ok(Json(
        serde_json::json!({"message":format!("Theme '{id}' deleted successfully")}),
    ))
}
async fn theme_asset(
    State(state): State<NativeState>,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, NativeApiError> {
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains('\\')
        || theme_asset_mime(&path).is_none()
    {
        return Err(NativeApiError::BadRequest);
    }
    let row = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT mime_type,bytes FROM native_theme_assets WHERE theme_id=$1 AND relative_path=$2",
    )
    .bind(id)
    .bind(path)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    let content_type = HeaderValue::from_str(&row.0).map_err(|_| NativeApiError::Unavailable)?;
    let mut response = Response::new(Body::from(row.1));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public,max-age=31536000"),
    );
    Ok(response)
}
async fn synthetic_internal_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"code":"INTERNAL_ERROR","message":"An unexpected error occurred","correlationId":Uuid::now_v7()}))).into_response()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn discovery_page_offsets_are_checked_and_bounded() {
        assert!(matches!(
            bounded_page(Some(2), Some(100), 20),
            Ok((2, 100, 100))
        ));
        assert!(bounded_page(Some(i32::MAX), Some(100), 20).is_err());
        assert!(bounded_page(Some(102), Some(100), 20).is_err());
    }

    fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, bytes, mode) in entries {
            writer
                .start_file(
                    *path,
                    SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Deflated)
                        .unix_permissions(*mode),
                )
                .expect("start ZIP entry");
            writer.write_all(bytes).expect("write ZIP entry");
        }
        writer.finish().expect("finish ZIP").into_inner()
    }

    #[test]
    fn theme_archive_accepts_a_bounded_database_owned_pack() {
        let bytes = archive(&[
            (
                "theme.json",
                br#"{"id":"aurora","name":"Aurora","author":"Melodee","version":"1"}"#,
                0o644,
            ),
            ("theme.css", b":root { --melodee-background: #fff; }", 0o644),
        ]);
        let parsed = parse_theme_archive(bytes).expect("valid theme archive");
        assert_eq!(parsed.metadata.id, "aurora");
        assert_eq!(parsed.assets.len(), 2);
    }

    #[test]
    fn theme_archive_rejects_traversal_duplicates_links_and_missing_contract_files() {
        let metadata = br#"{"id":"aurora","name":"Aurora"}"#;
        assert_eq!(
            parse_theme_archive(archive(&[
                ("theme.json", metadata, 0o644),
                ("../theme.css", b"body{}", 0o644),
            ]))
            .unwrap_err(),
            "Theme archive contains an unsafe path"
        );
        let mut duplicate_writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        duplicate_writer
            .start_file("theme.css", options)
            .expect("first path");
        duplicate_writer.write_all(b"body{}").expect("first body");
        assert!(
            duplicate_writer.start_file("theme.css", options).is_err(),
            "the archive dependency must reject an ambiguous duplicate path before import"
        );
        let mut link_writer = ZipWriter::new(Cursor::new(Vec::new()));
        link_writer
            .start_file("theme.json", options)
            .expect("theme metadata");
        link_writer.write_all(metadata).expect("metadata body");
        link_writer
            .add_symlink("theme.css", "outside.css", options)
            .expect("symlink entry");
        let link_archive = link_writer.finish().expect("finish ZIP").into_inner();
        assert_eq!(
            parse_theme_archive(link_archive).unwrap_err(),
            "Theme archive links and special files are not allowed"
        );
        assert_eq!(
            parse_theme_archive(archive(&[("theme.json", metadata, 0o644)])).unwrap_err(),
            "Required file 'theme.css' not found"
        );
    }
}
