//! Jellyfin audio endpoints: stream, stream.{container}, universal, lyrics,
//! download/file, and PlaybackInfo (PRD-API-JF-002/003).
//!
//! Reproduced from the frozen baseline (`AudioController.cs` and
//! `ItemsController.cs`): //!
//! - Streaming requires `HasStreamRole` (403 "Streaming permission required.");
//! download requires `HasDownloadRole` (403 "Download permission required.").
//! - Transcoding requests are honored (ADR-0147, superseding the captured
//! 400 rejection): `static!= true` with a container or bitrate parameter
//! streams configured FFmpeg output; `/universal` forces `static=true`
//! (unchanged captured direct play).
//! - Range handling is the captured manual parser: 206 with
//! `Content-Range: bytes start-end/len`; 416 with the ProblemDetails body;
//! `Content-Range: bytes */len` is only attached for out-of-range starts
//! and inverted ranges, never for unparseable headers (captured nuance).
//! - HEAD returns headers only (200, Content-Type/Content-Length).
//! - The Last-Modified/ETag the legacy `PhysicalFile` result attached are
//! filesystem-volatile (normalized away in the frozen captures) and are not
//! reproduced.
//! - Lyrics come from the sibling `.lrc` file with the captured timestamp
//! regex semantics; missing sidecar → `{"lyrics":[]}` (lowercase keys).

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::JfState;
use super::auth::resolve_token;
use super::browse::{db_failure, has_role, json_response, load_song, raw_query};
use super::error::{
    bad_request, not_found, range_not_satisfiable, service_unavailable, unauthorized,
};
use super::model;
use crate::native_stream::PlayableAsset;
use crate::transcode::{OutputFormat, TranscodeError};

const MAX_LYRIC_BYTES: u64 = 2 * 1024 * 1024;

pub fn router() -> Router<JfState> {
    Router::new()
 // axum 0.8 cannot express the legacy `{itemId}/stream.{extension}`
 // mixed segment, so the resource segment is dispatched manually.
        .route("/Audio/{id}/{resource}", get(audio_resource).head(audio_resource))
 // Captured: PlaybackInfo answers GET and POST identically.
        .route("/Items/{id}/PlaybackInfo", get(playback_info).post(playback_info))
 // Legacy registers GET only for Download/File (no HEAD).
        .route(
            "/Items/{id}/Download",
            axum::routing::on(axum::routing::MethodFilter::GET, download_file),
        )
        .route(
            "/Items/{id}/File",
            axum::routing::on(axum::routing::MethodFilter::GET, download_file),
        )
}

async fn resolve_playable_asset(
    state: &JfState,
    track_public_id: Uuid,
) -> Result<Option<PlayableAsset>, sqlx::Error> {
    // The canonical playable-asset pattern (native/stream.rs): the track's
    // playable_media_asset_id must also be linked with purpose='playable'.
    sqlx::query_as::<_, (String, i64, String)>(
        "SELECT media.canonical_relative_path, media.byte_size, media.mime_type
           FROM tracks track
           JOIN releases release ON release.id = track.release_id
             AND release.status = 'active'
           JOIN media_assets media ON media.id = track.playable_media_asset_id
           JOIN track_media_assets link
             ON link.track_id = track.id
            AND link.media_asset_id = media.id
            AND link.purpose = 'playable'
          WHERE track.public_api_id = $1",
    )
    .bind(track_public_id)
    .fetch_optional(&state.pool)
    .await
    .map(|row| {
        row.and_then(|(canonical_relative_path, byte_size, mime_type)| {
            match u64::try_from(byte_size) {
                Ok(byte_size) if byte_size > 0 => Some(PlayableAsset {
                    canonical_relative_path,
                    byte_size,
                    mime_type,
                }),
                _ => None,
            }
        })
    })
}

/// The captured extension-based content type mapping (`GetContentTypeForFile`).
fn content_type_for_path(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wav") => "audio/wav",
        Some("opus") => "audio/opus",
        _ => "application/octet-stream",
    }
}

enum RangeResolution {
    Resolved {
        start: u64,
        end: u64,
    },
    /// Parse failure: 416 without Content-Range (captured nuance).
    RejectNoHeader,
    /// Out-of-range or inverted: 416 with `Content-Range: bytes */{len}`.
    RejectWithSize,
}

/// The captured manual range parser (`AudioController.HandleRangeRequestAsync`).
fn resolve_range(range_header: &str, file_len: u64) -> RangeResolution {
    if !range_header
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bytes="))
    {
        return RangeResolution::RejectNoHeader;
    }
    let spec = &range_header[6..];
    let Some(dash) = spec.find('-') else {
        return RangeResolution::RejectNoHeader;
    };
    let start_part = &spec[..dash];
    let end_part = &spec[dash + 1..];
    let (start, end) = if start_part.is_empty() {
        match end_part.parse::<u64>() {
            Ok(suffix) if suffix > 0 => (file_len.saturating_sub(suffix), file_len - 1),
            _ => return RangeResolution::RejectNoHeader,
        }
    } else {
        let start = match start_part.parse::<u64>() {
            Ok(start) if start < file_len => start,
            _ => return RangeResolution::RejectWithSize,
        };
        let end = if end_part.is_empty() {
            file_len - 1
        } else {
            match end_part.parse::<u64>() {
                Ok(parsed) => parsed.min(file_len - 1),
                // Captured: an unparseable end falls back to the full length.
                Err(_) => file_len - 1,
            }
        };
        (start, end)
    };
    if start > end {
        return RangeResolution::RejectWithSize;
    }
    RangeResolution::Resolved { start, end }
}

fn ranged_response_416(file_len: u64, with_size: bool) -> Response {
    let mut response = range_not_satisfiable("Invalid range specified.");
    if with_size && let Ok(value) = HeaderValue::from_str(&format!("bytes */{file_len}")) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    // Captured: the 416 still carries Accept-Ranges.
    append_header(&mut response, header::ACCEPT_RANGES, "bytes");
    response
}

fn append_header(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

struct StreamRequest {
    container: Option<String>,
    static_flag: bool,
    audio_bit_rate: Option<String>,
}

fn stream_request(uri: &Uri, extension_container: Option<String>) -> StreamRequest {
    let mut container = extension_container;
    let mut static_flag = false;
    let mut audio_bit_rate = None;
    for (key, value) in raw_query(uri) {
        match key.as_str() {
            "container" if container.is_none() && !value.is_empty() => container = Some(value),
            "static" => static_flag = value.eq_ignore_ascii_case("true"),
            "audioBitRate" if !value.is_empty() => audio_bit_rate = Some(value),
            _ => {}
        }
    }
    StreamRequest {
        container,
        static_flag,
        audio_bit_rate,
    }
}

async fn audio_resource(
    State(state): State<JfState>,
    Path((id, resource)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
    method: axum::http::Method,
) -> Response {
    if resource.eq_ignore_ascii_case("lyrics") {
        return lyrics(state, id, headers, uri).await;
    }
    if resource == "universal" {
        // Captured: /universal forces static=true.
        let mut request = stream_request(&uri, None);
        request.static_flag = true;
        return stream(state, id, request, headers, uri, method).await;
    }
    if resource == "stream" {
        return stream(state, id, stream_request(&uri, None), headers, uri, method).await;
    }
    if let Some(extension) = resource.strip_prefix("stream.") {
        let request = stream_request(&uri, Some(extension.to_string()));
        return stream(state, id, request, headers, uri, method).await;
    }
    not_found("Item not found.")
}

async fn stream(
    state: JfState,
    id: String,
    request: StreamRequest,
    headers: HeaderMap,
    uri: Uri,
    method: axum::http::Method,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    if !has_role(&principal, "HasStreamRole") {
        return super::error::forbidden("Streaming permission required.");
    }
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let asset = match resolve_playable_asset(&state, public_id).await {
        Ok(Some(asset)) => asset,
        Ok(None) => return not_found("Audio item not found."),
        Err(error) => return db_failure("audio/stream", error),
    };
    let Some(direct_play) = state.direct_play.as_ref() else {
        return not_found("Audio file not found.");
    };
    // Captured order: the on-disk existence check precedes the transcode gate.
    if !direct_play.asset_exists(&asset) {
        return not_found("Audio file not found.");
    }

    // ADR-0147 decision 4: container/bitrate without static=true transcodes
    // (the captured 400 rejection is retired). static=true and /universal
    // keep the captured direct-play path byte-identical.
    if !request.static_flag {
        let container = request
            .container
            .as_deref()
            .map(str::trim)
            .filter(|container| !container.is_empty());
        if request.audio_bit_rate.is_some() || container.is_some() {
            return transcode_stream(
                &state,
                &asset,
                container,
                request.audio_bit_rate.as_deref(),
                &method,
            )
            .await;
        }
    }

    let content_type = content_type_for_path(&asset.canonical_relative_path);

    // Captured HEAD: 200 with Content-Type/Content-Length/Accept-Ranges only;
    // the Range header is ignored for HEAD.
    if method == axum::http::Method::HEAD {
        let mut response = StatusCode::OK.into_response();
        append_header(&mut response, header::CONTENT_TYPE, content_type);
        append_header(
            &mut response,
            header::CONTENT_LENGTH,
            &asset.byte_size.to_string(),
        );
        append_header(&mut response, header::ACCEPT_RANGES, "bytes");
        return response;
    }

    if let Some(range_header) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        match resolve_range(range_header, asset.byte_size) {
            RangeResolution::RejectNoHeader => {
                return ranged_response_416(asset.byte_size, false);
            }
            RangeResolution::RejectWithSize => {
                return ranged_response_416(asset.byte_size, true);
            }
            RangeResolution::Resolved { start, end } => {
                let body = match direct_play.stream_range(&asset, start, end).await {
                    Ok(body) => body,
                    Err(_) => return not_found("Audio file not found."),
                };
                let length = end - start + 1;
                let mut response = (StatusCode::PARTIAL_CONTENT, body).into_response();
                append_header(&mut response, header::CONTENT_TYPE, content_type);
                append_header(
                    &mut response,
                    header::CONTENT_RANGE,
                    &format!("bytes {start}-{end}/{}", asset.byte_size),
                );
                append_header(&mut response, header::CONTENT_LENGTH, &length.to_string());
                append_header(&mut response, header::ACCEPT_RANGES, "bytes");
                return response;
            }
        }
    }

    let body = match direct_play.stream_full(&asset).await {
        Ok(body) => body,
        Err(_) => return not_found("Audio file not found."),
    };
    let mut response = (StatusCode::OK, body).into_response();
    append_header(&mut response, header::CONTENT_TYPE, content_type);
    append_header(
        &mut response,
        header::CONTENT_LENGTH,
        &asset.byte_size.to_string(),
    );
    append_header(&mut response, header::ACCEPT_RANGES, "bytes");
    response
}

/// ADR-0147 decision 4 transcoding path. `container` (route extension or
/// query) maps mp3/aac/opus to the configured template and defaults to mp3
/// when absent; an unsupported container answers 400 "Unsupported
/// transcoding container." (the retired rejection message is not reused).
/// `audioBitRate` is bits per second (Jellyfin convention) and overrides the
/// template bitrate as `audioBitRate / 1000` kbps. HEAD answers headers only
/// and spawns no FFmpeg process. Transcoded responses carry the target
/// Content-Type with no Content-Length/Accept-Ranges (unseekable output).
async fn transcode_stream(
    state: &JfState,
    asset: &PlayableAsset,
    container: Option<&str>,
    audio_bit_rate: Option<&str>,
    method: &axum::http::Method,
) -> Response {
    let format = match container {
        None => OutputFormat::Mp3,
        Some(tag) => match OutputFormat::parse_tag(tag) {
            Some(format) => format,
            None => return bad_request("Unsupported transcoding container."),
        },
    };
    let Some(service) = &state.transcoding else {
        return service_unavailable("Transcoding is not available.");
    };
    let template = match service.template(format).await {
        Ok(template) => template,
        Err(error) => {
            tracing::warn!(%error, ?format, "jellyfin transcode template unavailable");
            return service_unavailable("Transcoding is not available.");
        }
    };
    let bitrate_kbps = audio_bit_rate
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(|bits_per_second| bits_per_second / 1000)
        .filter(|kbps| *kbps > 0)
        .and_then(|kbps| u32::try_from(kbps).ok())
        .unwrap_or(template.bitrate_kbps);
    if method == axum::http::Method::HEAD {
        let mut response = StatusCode::OK.into_response();
        append_header(&mut response, header::CONTENT_TYPE, format.content_type());
        return response;
    }
    match service.stream(asset, format, &template, bitrate_kbps).await {
        Ok(body) => {
            let mut response = (StatusCode::OK, body).into_response();
            append_header(&mut response, header::CONTENT_TYPE, format.content_type());
            response
        }
        // Missing/invalid asset keeps the captured missing-file 404 class.
        Err(TranscodeError::AssetUnavailable) => not_found("Audio file not found."),
        Err(TranscodeError::CapacityExhausted) => {
            service_unavailable("Transcoding capacity exhausted.")
        }
        Err(error) => {
            tracing::warn!(%error, ?format, "jellyfin transcode stream failed");
            service_unavailable("Transcoding failed.")
        }
    }
}

/// Parses one LRC line with the captured regex semantics
/// `\[(\d{2}):(\d{2})\.(\d{2,3})\](.*)`: two-digit milliseconds are
/// right-padded to three; non-timestamped text lines get `Start: null`.
fn parse_lrc_line(line: &str) -> Option<Value> {
    if let Some(rest) = line.strip_prefix('[') {
        let close = rest.find(']')?;
        let stamp = &rest[..close];
        let text = rest[close + 1..].trim();
        let mut parts = stamp.splitn(2, ':');
        let minutes: i64 = parts.next()?.parse().ok()?;
        let mut sec_parts = parts.next()?.splitn(2, '.');
        let seconds: i64 = sec_parts.next()?.parse().ok()?;
        let mut millis = sec_parts.next()?.to_string();
        if millis.len() == 2 {
            millis.push('0');
        }
        if millis.len() != 3 {
            return None;
        }
        let millis: i64 = millis.parse().ok()?;
        if text.is_empty() {
            return None;
        }
        let start = ((minutes * 60 + seconds) * 1000 + millis) * 10_000;
        return Some(json!({"text": text, "start": start}));
    }
    let text = line.trim();
    if text.is_empty() {
        return None;
    }
    Some(json!({"text": text, "start": null}))
}

async fn lyrics(state: JfState, id: String, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let asset = match resolve_playable_asset(&state, public_id).await {
        Ok(Some(asset)) => asset,
        Ok(None) => return not_found("Audio item not found."),
        Err(error) => return db_failure("audio/lyrics", error),
    };
    let Some(direct_play) = state.direct_play.as_ref() else {
        return json_response(json!({"lyrics": []}));
    };
    // Path.ChangeExtension semantics: replace the final extension with.lrc.
    let lrc_path = match asset.canonical_relative_path.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => format!("{stem}.lrc"),
        _ => format!("{}.lrc", asset.canonical_relative_path),
    };
    let bytes = match direct_play
        .read_relative_bounded(&lrc_path, MAX_LYRIC_BYTES)
        .await
    {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return json_response(json!({"lyrics": []})),
        Err(_) => return json_response(json!({"lyrics": []})),
    };
    let content = String::from_utf8_lossy(&bytes);
    let lyrics: Vec<Value> = content.lines().filter_map(parse_lrc_line).collect();
    json_response(json!({"lyrics": lyrics}))
}

async fn playback_info(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(_principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    match load_song(&state.pool, public_id).await {
        Ok(Some(song)) => json_response(json!({
            "MediaSources": [model::playback_info_media_source(&song)],
            "PlaySessionId": model::nf(Uuid::now_v7()),
        })),
        Ok(None) => not_found("Item not found."),
        Err(error) => db_failure("audio/playback-info", error),
    }
}

async fn download_file(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    if !has_role(&principal, "HasDownloadRole") {
        return super::error::forbidden("Download permission required.");
    }
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let asset = match resolve_playable_asset(&state, public_id).await {
        Ok(Some(asset)) => asset,
        Ok(None) => return not_found("Item not found."),
        Err(error) => return db_failure("audio/download", error),
    };
    let Some(direct_play) = state.direct_play.as_ref() else {
        return not_found("File not found.");
    };
    if !direct_play.asset_exists(&asset) {
        return not_found("File not found.");
    }
    let content_type = content_type_for_path(&asset.canonical_relative_path);
    // The captured Content-Disposition filename is the on-disk file name,
    // percent-escaped (Uri.EscapeDataString).
    let file_name = asset
        .canonical_relative_path
        .rsplit('/')
        .next()
        .unwrap_or(&asset.canonical_relative_path);
    let disposition = format!(
        "attachment; filename=\"{}\"",
        super::browse::escape_data_string(file_name)
    );

    if let Some(range_header) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        match resolve_range(range_header, asset.byte_size) {
            RangeResolution::RejectNoHeader => {
                return ranged_response_416(asset.byte_size, false);
            }
            RangeResolution::RejectWithSize => {
                return ranged_response_416(asset.byte_size, true);
            }
            RangeResolution::Resolved { start, end } => {
                let body = match direct_play.stream_range(&asset, start, end).await {
                    Ok(body) => body,
                    Err(_) => return not_found("File not found."),
                };
                let length = end - start + 1;
                let mut response = (StatusCode::PARTIAL_CONTENT, body).into_response();
                append_header(&mut response, header::CONTENT_TYPE, content_type);
                append_header(
                    &mut response,
                    header::CONTENT_RANGE,
                    &format!("bytes {start}-{end}/{}", asset.byte_size),
                );
                append_header(&mut response, header::CONTENT_LENGTH, &length.to_string());
                append_header(&mut response, header::ACCEPT_RANGES, "bytes");
                append_header(&mut response, header::CONTENT_DISPOSITION, &disposition);
                return response;
            }
        }
    }

    let body = match direct_play.stream_full(&asset).await {
        Ok(body) => body,
        Err(_) => return not_found("File not found."),
    };
    let mut response = (StatusCode::OK, body).into_response();
    append_header(&mut response, header::CONTENT_TYPE, content_type);
    append_header(
        &mut response,
        header::CONTENT_LENGTH,
        &asset.byte_size.to_string(),
    );
    append_header(&mut response, header::ACCEPT_RANGES, "bytes");
    append_header(&mut response, header::CONTENT_DISPOSITION, &disposition);
    response
}
