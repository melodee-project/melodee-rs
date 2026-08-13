//! Jellyfin session endpoints (PRD-API-JF-001/003).
//!
//! Reproduced from the frozen baseline (`SessionsController.cs`):
//!
//! - `GET /Sessions` synthesizes entries from now-playing state. The frozen
//!   fixture golden is `[]` (no now-playing entries existed); with live
//!   now-playing rows the captured handler shape is emitted.
//! - Playing/Progress/Stopped/Ping and Capabilities/Capabilities/Full → 204.
//! - Playing/Progress feed now-playing; Stopped records a scrobble unless
//!   `Failed=true`; unknown/malformed ItemId is silently 204.
//! - Logout lives in `auth.rs` (token lifecycle) and revokes the current
//!   token idempotently.

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::auth::resolve_token;
use super::browse::{db_failure, json_response};
use super::error::unauthorized;
use super::model;
use melodee_persistence::NativeUserStateStore;

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/Sessions", get(list_sessions))
        .route("/Sessions/Playing", post(playback_start))
        .route("/Sessions/Playing/Progress", post(playback_progress))
        .route("/Sessions/Playing/Stopped", post(playback_stopped))
        .route("/Sessions/Playing/Ping", post(playback_ping))
        .route("/Sessions/Capabilities", post(capabilities))
        .route("/Sessions/Capabilities/Full", post(capabilities_full))
}

async fn list_sessions(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let rows = match sqlx::query(
        "SELECT now_playing.client_event_id, now_playing.started_at, now_playing.updated_at, \
                track.public_api_id, track.title, track.duration_ms, \
                account.public_api_id, account.username \
           FROM native_now_playing now_playing \
           JOIN tracks track ON track.id = now_playing.track_id \
           JOIN native_users account ON account.id = now_playing.user_id \
          ORDER BY now_playing.updated_at, track.public_api_id \
          LIMIT 100",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => return db_failure("sessions/list", error),
    };
    let sessions: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let client: String = row.get(0);
            let last_activity: OffsetDateTime = row.get(2);
            let track_public_id: Uuid = row.get(3);
            let track_title: String = row.get(4);
            let duration_ms: Option<i64> = row.get(5);
            let user_public_id: Uuid = row.get(6);
            let username: String = row.get(7);
            // The legacy session id is the now-playing cache entry id; the
            // adapter derives a stable one from the user+track pair.
            model::session_entry_json(
                &model::md5_hex(format!(
                    "jellyfin-session:{}:{}",
                    model::nf(user_public_id),
                    model::nf(track_public_id)
                )),
                &model::nf(user_public_id),
                &username,
                &client,
                None,
                None,
                track_public_id,
                &track_title,
                duration_ms,
                last_activity,
            )
        })
        .collect();
    json_response(Value::Array(sessions))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackStartBody {
    item_id: Option<String>,
    play_session_id: Option<String>,
    position_ticks: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackStopBody {
    item_id: Option<String>,
    play_session_id: Option<String>,
    failed: Option<bool>,
}

/// Parses the captured playback-report body leniently; null/invalid bodies
/// and unparseable ItemIds are silent 204s.
fn parse_body<T: for<'de> Deserialize<'de>>(body: &axum::body::Bytes) -> Option<T> {
    serde_json::from_slice(body).ok()
}

async fn playback_start(
    State(state): State<JfState>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(request) = parse_body::<PlaybackStartBody>(&body) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(item_id) = request.item_id.filter(|id| !id.trim().is_empty()) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(track_public_id) = model::parse_id(&item_id) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let now = OffsetDateTime::now_utc();
    let _ = request.position_ticks; // now-playing position is not persisted
    let client_event = request
        .play_session_id
        .unwrap_or_else(|| "Jellyfin".to_string());
    match NativeUserStateStore::record_now_playing(
        &state.pool,
        principal.user_id,
        track_public_id,
        &client_event,
        now,
        now,
    )
    .await
    {
        Ok(_) => {}
        Err(melodee_persistence::NativeUserStateStoreError::NotFound) => {}
        Err(error) => {
            tracing::warn!(%error, "jellyfin playback start failed");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn playback_progress(
    State(state): State<JfState>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(request) = parse_body::<PlaybackStartBody>(&body) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(item_id) = request.item_id.filter(|id| !id.trim().is_empty()) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(track_public_id) = model::parse_id(&item_id) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    // Captured: Progress also feeds the now-playing cache.
    let now = OffsetDateTime::now_utc();
    let client_event = request
        .play_session_id
        .unwrap_or_else(|| "Jellyfin".to_string());
    let _ = NativeUserStateStore::record_now_playing(
        &state.pool,
        principal.user_id,
        track_public_id,
        &client_event,
        now,
        now,
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn playback_stopped(
    State(state): State<JfState>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(request) = parse_body::<PlaybackStopBody>(&body) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(item_id) = request.item_id.filter(|id| !id.trim().is_empty()) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some(track_public_id) = model::parse_id(&item_id) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    // Captured: Failed=true skips the scrobble.
    if request.failed == Some(true) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let now = OffsetDateTime::now_utc();
    let client_event = request
        .play_session_id
        .unwrap_or_else(|| "Jellyfin".to_string());
    let _ = NativeUserStateStore::record_scrobble(
        &state.pool,
        principal.user_id,
        track_public_id,
        &client_event,
        now,
        now,
        None,
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn playback_ping(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn capabilities(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn capabilities_full(
    State(state): State<JfState>,
    headers: HeaderMap,
    uri: Uri,
    _body: axum::body::Bytes,
) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    StatusCode::NO_CONTENT.into_response()
}
