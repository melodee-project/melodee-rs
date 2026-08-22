//! Jellyfin session endpoints (PRD-API-JF-001/003).
//!
//! Reproduced from the frozen baseline (`SessionsController.cs`):
//!
//! - `GET /Sessions` returns the frozen `[]`: the legacy capture posted
//!   Playing/Progress and still listed zero sessions (HTTP playback reports
//!   never registered a listable session at this baseline), so the adapter
//!   must not synthesize entries from now-playing state.
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
use time::OffsetDateTime;

use super::JfState;
use super::auth::resolve_token;
use super::browse::json_response;
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
    // Frozen baseline: `jf-userstate/sessions-list` is `[]` even immediately
    // after Playing/Progress reports — HTTP playback posts never registered a
    // listable session. No session manager exists at this baseline.
    json_response(Value::Array(Vec::new()))
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
        Ok(_) => {
            // fire-and-record Last.fm dispatch; never blocks or fails
            // the write path (legacy ADR-0004 silent no-op when unlinked).
            crate::lastfm::LastfmDispatch::new(
                state.pool.clone(),
                state.lastfm_api_base_url.as_deref(),
            )
            .spawn_now_playing(principal.user_id, track_public_id);
        }
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
    let now_playing = NativeUserStateStore::record_now_playing(
        &state.pool,
        principal.user_id,
        track_public_id,
        &client_event,
        now,
        now,
    )
    .await;
    if now_playing.is_ok() {
        crate::lastfm::LastfmDispatch::new(
            state.pool.clone(),
            state.lastfm_api_base_url.as_deref(),
        )
        .spawn_now_playing(principal.user_id, track_public_id);
    }
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
    let scrobble = NativeUserStateStore::record_scrobble(
        &state.pool,
        principal.user_id,
        track_public_id,
        &client_event,
        now,
        now,
        None,
    )
    .await;
    // dispatch to Last.fm only for a freshly applied play — an
    // adopted idempotent replay must not double-scrobble.
    if matches!(
        scrobble,
        Ok(melodee_persistence::NativeScrobbleResult::Applied)
    ) {
        crate::lastfm::LastfmDispatch::new(
            state.pool.clone(),
            state.lastfm_api_base_url.as_deref(),
        )
        .spawn_scrobble(principal.user_id, track_public_id);
    }
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
