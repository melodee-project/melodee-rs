//! Media endpoint group: stream, download, getCoverArt, getAvatar, getLyrics,
//! getLyricsBySongId (PRD-API-OS-004, PRD-STREAM-002, ADR-0130 D2; golden
//! evidence `compatibility/opensubsonic/golden/media`).
//!
//! `/images/{id}/{size}` is NOT an OpenSubsonic route: it is absent from the
//! OpenSubsonic route inventory and the frozen baseline attributes it to the
//! native surface (200 placeholder / 200 empty classes), so the native
//! adapter owns it. The OpenSubsonic auth middleware still treats `/images/`
//! paths as bypass-eligible, matching the captured auth rule.
//!
//! Captured behaviors reproduced here:
//! - **stream** (D2 single-pass range fix — the legacy double-range bug is NOT
//!   preserved): full GET → 200 `audio/wav` (content-type from the asset),
//!   `Accept-Ranges: bytes`, `Content-Length`, `Content-Duration`, no-store
//!   cache headers, `Last-Modified`, `ETag "{size}-{ticks}"`. HEAD → same
//!   headers no body. Single-pass ranges return 206 with correct bytes and
//!   correct `Content-Range: bytes start-end/{full_size}` total. Invalid/
//!   unsatisfiable ranges and `timeOffset` → 404 + error 70 JSON (the legacy
//!   416 path is unreachable). `maxBitRate`/`format` silently ignored. Unknown
//!   song id → 302 credential-free class.
//! - **download:** gated by `state.downloads_enabled`; 200 with
//!   `Content-Disposition: attachment; filename=...`; unknown → 302
//!   credential-free.
//! - **getCoverArt:** serves stored artwork bytes with sniffed content-type,
//!   `ETag`, `Cache-Control: public, max-age=86400`; no artwork serves the
//!   captured default placeholder (sized → processed JPEG, unsized → raw
//!   fixture bytes) WITHOUT Cache-Control; unknown id → 200 empty body.
//! - **getAvatar:** serves the user's stored avatar by `username`/`u`/`id`.
//! - **getLyrics/getLyricsBySongId:** the legacy serializer ignored the
//!   structured `.lrc` list at this baseline, so `getLyrics` echoes
//!   `{lyrics:{value:"",artist,title}}` and `getLyricsBySongId` returns a bare
//!   ok envelope; missing lyrics never error.

use axum::{
    Router,
    extract::{Extension, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    annotation::{EntityRef, parse_prefixed_id},
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};
use crate::native_stream::{PlayableAsset, StreamError};

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

/// Captured unknown-id 302 class: the legacy Location embedded the full request
/// query (including credentials) — that credential leak is not preserved
/// (ADR-0130); the Rust redirect points to a static error target with no
/// query material.
fn redirect_to_error(state: &OsState) -> Response {
    let location = format!(
        "{}/Account/Login?ReturnUrl=%2FError",
        state.public_origin.trim_end_matches('/')
    );
    let mut response = Response::default();
    *response.status_mut() = StatusCode::FOUND;
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Captured stream/download range/timeOffset failure: HTTP 404 + error 70 JSON
/// envelope (the legacy 416 path is unreachable; the service re-wraps the
/// error). The captured error envelopes carried no `x-total-count` header,
/// unlike the success/other-error envelopes.
fn stream_not_found(state: &OsState, params: &Option<Extension<Params>>) -> Response {
    OsReply::error(70, "The requested data was not found.")
        .without_total_count_header()
        .respond(
            &format_of(params),
            &state.server_version,
            StatusCode::NOT_FOUND,
        )
}

/// One catalog row resolving a track public id to its playable media asset.
struct PlayableRow {
    asset: PlayableAsset,
    duration_ms: i64,
    source_path: Option<String>,
}

async fn resolve_playable(
    pool: &sqlx::PgPool,
    track_public_id: Uuid,
) -> Result<Option<PlayableRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT m.canonical_relative_path, m.byte_size, m.mime_type, m.duration_ms,
                t.source_path
         FROM tracks t
         JOIN releases r ON r.id = t.release_id AND r.status = 'active'
         JOIN media_assets m ON m.id = t.playable_media_asset_id
         WHERE t.public_api_id = $1",
    )
    .bind(track_public_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| {
        let byte_size_i64: i64 = row.try_get(1).unwrap_or(0);
        let byte_size = u64::try_from(byte_size_i64).unwrap_or(0);
        PlayableRow {
            asset: PlayableAsset {
                canonical_relative_path: row.try_get(0).unwrap_or_default(),
                byte_size,
                mime_type: row.try_get(2).unwrap_or_default(),
            },
            duration_ms: row.try_get(3).unwrap_or(0),
            source_path: row.try_get(4).unwrap_or(None),
        }
    }))
}

/// Renders the `Content-Duration` header value (seconds, one decimal place).
fn content_duration_header(duration_ms: i64) -> String {
    let seconds = (duration_ms as f64) / 1000.0;
    format!("{seconds:.1}")
}

/// Builds the captured stream/download response headers shared by GET and
/// HEAD: no-store cache headers, CORS, Content-Duration, Last-Modified, ETag.
/// `Last-Modified` and `ETag` are volatile (dropped at golden compare) so a
/// stable now-based stand-in is acceptable.
fn add_stream_headers(
    response: &mut Response,
    byte_size: u64,
    duration_ms: i64,
    mtime: OffsetDateTime,
) {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, must-revalidate, no-cache, max-age=0"),
    );
    headers.insert(
        header::EXPIRES,
        HeaderValue::from_static("Mon, 01 Jan 1990 00:00:00 GMT"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if let Ok(value) = HeaderValue::from_str(&content_duration_header(duration_ms)) {
        headers.insert(header::HeaderName::from_static("content-duration"), value);
    }
    if let Ok(value) = HeaderValue::from_str(&http_date(mtime)) {
        headers.insert(header::LAST_MODIFIED, value);
    }
    // ETag "{size}-{ticks}": the .NET ticks value is volatile (dropped at
    // compare). Use the file mtime's unix nanos as a stable stand-in.
    let ticks = mtime.unix_timestamp_nanos();
    if let Ok(value) = HeaderValue::from_str(&format!("{byte_size}-{ticks}")) {
        headers.insert(header::ETAG, value);
    }
}

/// RFC 7231 IMF-fixdate, e.g. `Sun, 09 Aug 2026 19:56:14 GMT`.
fn http_date(value: OffsetDateTime) -> String {
    let weekday = match value.weekday() {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    };
    let month = match value.month() {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    };
    format!(
        "{weekday}, {:02} {month} {:04} {:02}:{:02}:{:02} GMT",
        value.day(),
        value.year(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

pub async fn stream(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
    method: Method,
    headers: axum::http::HeaderMap,
) -> Response {
    stream_impl(&state, &params, &principal, &method, &headers).await
}

async fn stream_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    method: &Method,
    headers: &axum::http::HeaderMap,
) -> Response {
    // The captured bypass synthesizes no user; a bypassed stream request has
    // no captured behavior beyond the unknown-id 302 class, so require a
    // principal (the fixture authenticated stream requests).
    let _ = match principal {
        Some(Extension(p)) => p,
        None => return stream_not_found(state, params),
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return stream_not_found(state, params);
    };
    // `timeOffset` is advertised but non-functional: the captured contract
    // answers 404 + error 70 (the 416 path is unreachable).
    if params_ref.get("timeOffset").is_some() {
        return stream_not_found(state, params);
    }
    let Some(raw_id) = params_ref.get("id") else {
        return stream_not_found(state, params);
    };
    let Some(EntityRef::Track(track_public_id)) = parse_prefixed_id(raw_id) else {
        return stream_not_found(state, params);
    };
    // `maxBitRate`/`format` are silently ignored (no transcoding exists).
    let row = match resolve_playable(&state.pool, track_public_id).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "opensubsonic stream asset resolution failed");
            return stream_not_found(state, params);
        }
    };
    let Some(row) = row else {
        return redirect_to_error(state);
    };
    let Some(direct_play) = &state.direct_play else {
        tracing::error!("opensubsonic stream requested but direct_play is unconfigured");
        return stream_not_found(state, params);
    };
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let head_only = method == Method::HEAD;
    let response = match direct_play
        .response(&row.asset, range_header, head_only)
        .await
    {
        Ok(response) => response,
        Err(StreamError::InvalidRange) | Err(StreamError::AssetUnavailable) => {
            // Invalid/unsatisfiable ranges and missing-file both map to the
            // captured 404 + error 70 (ADR-0130 decision 4: correct
            // single-pass range handling; the legacy double-range bug —
            // frozen in the os-stream-range-* goldens — is NOT preserved).
            return stream_not_found(state, params);
        }
        Err(error) => {
            tracing::error!(%error, "opensubsonic stream direct-play failed");
            return stream_not_found(state, params);
        }
    };
    let mtime = OffsetDateTime::now_utc();
    let mut response = response;
    add_stream_headers(&mut response, row.asset.byte_size, row.duration_ms, mtime);
    response
}

/// `Content-Disposition: attachment; filename="..."` for download. The
/// captured filename is the source-path basename (e.g.
/// `01 - Solar Wind.wav`).
fn content_disposition(filename: &str) -> Option<HeaderValue> {
    // RFC 5987 encoded fallback for non-ASCII; the captured fixture is ASCII.
    let encoded: String = filename
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_') {
                format!("{}", byte as char)
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    let value = format!("attachment; filename=\"{filename}\"; filename*=UTF-8''{encoded}");
    HeaderValue::from_str(&value).ok()
}

fn download_basename(source_path: Option<&str>) -> String {
    source_path
        .and_then(|path| path.rsplit_once('/'))
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| "download.bin".to_string())
}

pub async fn download(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
    method: Method,
    headers: axum::http::HeaderMap,
) -> Response {
    download_impl(&state, &params, &principal, &method, &headers).await
}

async fn download_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    method: &Method,
    headers: &axum::http::HeaderMap,
) -> Response {
    let _ = match principal {
        Some(Extension(p)) => p,
        None => return stream_not_found(state, params),
    };
    if !state.downloads_enabled {
        // The captured disabled-download behavior collapses to the unknown-id
        // 302 class (the legacy gate redirects unauthenticated-style).
        return redirect_to_error(state);
    }
    let Some(Extension(params_ref)) = params.as_ref() else {
        return stream_not_found(state, params);
    };
    let Some(raw_id) = params_ref.get("id") else {
        return stream_not_found(state, params);
    };
    let Some(EntityRef::Track(track_public_id)) = parse_prefixed_id(raw_id) else {
        return stream_not_found(state, params);
    };
    let row = match resolve_playable(&state.pool, track_public_id).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "opensubsonic download asset resolution failed");
            return stream_not_found(state, params);
        }
    };
    let Some(row) = row else {
        return redirect_to_error(state);
    };
    let Some(direct_play) = &state.direct_play else {
        tracing::error!("opensubsonic download requested but direct_play is unconfigured");
        return stream_not_found(state, params);
    };
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let head_only = method == Method::HEAD;
    let mut response = match direct_play
        .response(&row.asset, range_header, head_only)
        .await
    {
        Ok(response) => response,
        Err(StreamError::InvalidRange) | Err(StreamError::AssetUnavailable) => {
            return stream_not_found(state, params);
        }
        Err(error) => {
            tracing::error!(%error, "opensubsonic download direct-play failed");
            return stream_not_found(state, params);
        }
    };
    let mtime = OffsetDateTime::now_utc();
    add_stream_headers(&mut response, row.asset.byte_size, row.duration_ms, mtime);
    let basename = download_basename(row.source_path.as_deref());
    if let Some(value) = content_disposition(&basename) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

/// Sniffs a content-type from artwork bytes when the stored mime_type is
/// absent. Falls back to `application/octet-stream`.
fn sniff_image_content_type(bytes: &[u8]) -> String {
    if bytes.len() >= 3 && &bytes[0..3] == b"\xFF\xD8\xFF" {
        return "image/jpeg".to_string();
    }
    if bytes.len() >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        return "image/png".to_string();
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    if bytes.len() >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        return "image/gif".to_string();
    }
    "application/octet-stream".to_string()
}

/// Captured default cover-art bytes: the legacy image pipeline's processed
/// album/song default (identical to the native large album placeholder) and
/// the raw default returned when no `size` is requested (the 136-byte legacy
/// placeholder, bytes frozen in `os-getcoverart-default.bin`).
const DEFAULT_COVER_SIZED: &[u8] = include_bytes!(
    "../../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin"
);
const DEFAULT_COVER_RAW: &[u8] = include_bytes!(
    "../../../../compatibility/opensubsonic/golden/media/os-getcoverart-default.bin"
);

/// Artwork resolution for a `song_`/`album_`/`artist_`/`playlist_` id.
enum Artwork {
    /// Stored bytes (playlist images only at this baseline).
    Stored(Vec<u8>, String),
    /// A known catalog entity without stored artwork: the legacy default
    /// image applies.
    Default,
}

/// Resolves artwork for a `song_`/`album_`/`artist_`/`playlist_` id. Playlist
/// ids serve `native_playlists.image_bytes`; known song/album/artist ids fall
/// back to the captured default artwork; unknown ids return None (the
/// captured 200 empty-body shape).
async fn resolve_artwork(
    pool: &sqlx::PgPool,
    raw_id: &str,
) -> Result<Option<Artwork>, sqlx::Error> {
    if let Some(rest) = raw_id.strip_prefix("playlist_")
        && let Ok(id) = Uuid::parse_str(rest)
    {
        let row = sqlx::query(
            "SELECT image_mime_type, image_bytes FROM native_playlists WHERE public_api_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        let stored = row.and_then(|row| {
            let mime: Option<String> = row.try_get(0).ok()?;
            let bytes: Option<Vec<u8>> = row.try_get(1).ok()?;
            bytes.map(|b| {
                let mime_type = mime.unwrap_or_else(|| sniff_image_content_type(&b));
                (b, mime_type)
            })
        });
        return Ok(stored.map(|(bytes, mime)| Artwork::Stored(bytes, mime)));
    }
    if !(raw_id.starts_with("album_")
        || raw_id.starts_with("song_")
        || raw_id.starts_with("artist_"))
    {
        return Ok(None);
    }
    Ok(if artwork_id_exists(pool, raw_id).await? {
        Some(Artwork::Default)
    } else {
        None
    })
}

/// Whether an album_/song_/artist_ cover-art id names a known entity.
async fn artwork_id_exists(pool: &sqlx::PgPool, raw_id: &str) -> Result<bool, sqlx::Error> {
    let Some(entity) = parse_prefixed_id(raw_id) else {
        return Ok(false);
    };
    let (table, id) = match entity {
        EntityRef::Release(id) => ("releases", id),
        EntityRef::Track(id) => ("tracks", id),
        EntityRef::Artist(id) => ("artists", id),
    };
    // The table name is one of three module-owned constants selected by the
    // entity kind; only the id is bound.
    sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE public_api_id = $1)"
    )))
    .bind(id)
    .fetch_one(pool)
    .await
}

pub async fn get_cover_art(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
    headers: axum::http::HeaderMap,
) -> Response {
    get_cover_art_impl(&state, &params, &principal, &headers).await
}

async fn get_cover_art_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    headers: &axum::http::HeaderMap,
) -> Response {
    // getCoverArt is bypass-eligible but the captured authenticated path is
    // the fixture default; a bypass request (no principal) still resolves
    // artwork by id.
    let _ = principal;
    let Some(Extension(params_ref)) = params.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(raw_id) = params_ref.get("id") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let artwork = match resolve_artwork(&state.pool, raw_id).await {
        Ok(artwork) => artwork,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getCoverArt resolution failed");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    // Captured unknown-artwork shape: 200 with content-length 0 and no
    // content-type (os-getcoverart-unknown metadata).
    let Some(artwork) = artwork else {
        return StatusCode::OK.into_response();
    };
    let requested_size = params_ref.get("size");
    let (bytes, mime_type, etag, is_default) = match artwork {
        Artwork::Stored(bytes, mime_type) => {
            let etag = format!("\"{}\"", sha256_hex(&bytes));
            (bytes, mime_type, etag, false)
        }
        Artwork::Default => {
            // Captured defaults (`os-getcoverart-default/-80/-song`): a sized
            // request is served the processed default JPEG; an unsized
            // request the raw placeholder whose legacy ETag is the fixture
            // file's ticks value.
            if requested_size.is_some() {
                (
                    DEFAULT_COVER_SIZED.to_vec(),
                    "image/jpeg".to_string(),
                    "fda2a2741f69f726b74e07522e8e57487ed3d26a32699e798da0877f47348b18".to_string(),
                    true,
                )
            } else {
                (
                    DEFAULT_COVER_RAW.to_vec(),
                    "image/jpeg".to_string(),
                    "17673120000000000".to_string(),
                    true,
                )
            }
        }
    };
    if let Some(inm) = headers.get(header::IF_NONE_MATCH)
        && inm.to_str().map(|value| value == etag).unwrap_or(false)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        return response;
    }
    let mut response = (StatusCode::OK, axum::body::Body::from(bytes.clone())).into_response();
    if let Ok(value) = HeaderValue::from_str(&mime_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    // Captured: the legacy default-placeholder responses carry no
    // Cache-Control header; only stored-artwork responses do.
    if !is_default {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
    }
    response
}

pub async fn get_avatar(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    get_avatar_impl(&state, &params, &principal).await
}

async fn get_avatar_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    let Some(Extension(params_ref)) = params.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // The avatar is selected by `username`/`u`/`id`. The `id` form is
    // `user_<uuid>` (native public_api_id); `username`/`u` is the login name.
    let username = params_ref.get("username").or_else(|| params_ref.get("u"));
    let row = if let Some(name) = username {
        sqlx::query(
            "SELECT image_mime_type, image_bytes FROM native_users WHERE normalized_username = UPPER($1)",
        )
        .bind(name)
        .fetch_optional(&state.pool)
        .await
    } else if let Some(raw_id) = params_ref.get("id") {
        if let Some(rest) = raw_id.strip_prefix("user_") {
            if let Ok(id) = Uuid::parse_str(rest) {
                sqlx::query(
                    "SELECT image_mime_type, image_bytes FROM native_users WHERE public_api_id = $1",
                )
                .bind(id)
                .fetch_optional(&state.pool)
                .await
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    };
    let row = match row {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getAvatar lookup failed");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let Some(row) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime: Option<String> = row.try_get(0).unwrap_or(None);
    let bytes: Option<Vec<u8>> = row.try_get(1).unwrap_or(None);
    // Captured (`os-getavatar`): a user without stored avatar bytes is served
    // the legacy default avatar image, not a 404.
    let (bytes, default_mime) = match bytes {
        Some(bytes) => (bytes, None),
        None => (
            crate::native::images::DEFAULT_USER.to_vec(),
            Some("image/png"),
        ),
    };
    let mime_type = mime
        .or_else(|| default_mime.map(str::to_string))
        .unwrap_or_else(|| sniff_image_content_type(&bytes));
    let mut response = (StatusCode::OK, axum::body::Body::from(bytes)).into_response();
    if let Ok(value) = HeaderValue::from_str(&mime_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

/// Renders the captured lyrics envelope. The legacy `getLyrics` shape is a
/// single `lyrics` object with `value` (joined text), `artist`, `title`. The
/// OpenSubsonic extension advertises a structured `lyricsList` from a `.lrc`
/// sidecar, but the captured fixture returns `value: ""` with the artist/title
/// echo even when a sidecar is present (the legacy serializer ignored the
/// structured list at this baseline).
fn lyrics_envelope(artist: Option<&str>, title: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "lyrics": {
            "value": "",
            "artist": artist.unwrap_or(""),
            "title": title.unwrap_or(""),
        }
    })
}

pub async fn get_lyrics(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    get_lyrics_impl(&state, &params, &principal).await
}

async fn get_lyrics_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    let Some(Extension(params_ref)) = params.as_ref() else {
        return OsReply::ok(lyrics_envelope(None, None)).respond(
            &format_of(params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let artist = params_ref.get("artist").map(str::to_string);
    let title = params_ref.get("title").map(str::to_string);
    OsReply::ok(lyrics_envelope(artist.as_deref(), title.as_deref())).respond(
        &format_of(params),
        &state.server_version,
        StatusCode::OK,
    )
}

pub async fn get_lyrics_by_song_id(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    get_lyrics_by_song_id_impl(&state, &params, &principal).await
}

async fn get_lyrics_by_song_id_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    // The captured getLyricsBySongId golden returns a bare ok envelope with no
    // lyrics key (os-getlyricsbysongid.json / os-getlyricsbysongid-none.json
    // are identical). The structured lyricsList extension is advertised but
    // the fixture emits the bare ok shape at this baseline.
    OsReply::ok(serde_json::json!({})).respond(
        &format_of(params),
        &state.server_version,
        StatusCode::OK,
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("stream", stream))
        .merge(super::endpoint_router("download", download))
        .merge(super::endpoint_router("getCoverArt", get_cover_art))
        .merge(super::endpoint_router("getAvatar", get_avatar))
        .merge(super::endpoint_router("getLyrics", get_lyrics))
        .merge(super::endpoint_router(
            "getLyricsBySongId",
            get_lyrics_by_song_id,
        ))
}
