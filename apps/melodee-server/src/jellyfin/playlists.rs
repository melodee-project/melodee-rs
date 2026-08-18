//! Jellyfin playlist endpoints (PRD-API-JF-004/005).
//!
//! Reproduced from the frozen baseline (`PlaylistsController.cs` +
//! `ItemsController.DeleteItemAsync`):
//!
//! - Playlist ids are ApiKey GUIDs; items are the reduced Audio shape with
//!   `ParentIndexNumber` = 1-based page position and hydrated `UserData` only
//!   when `enableUserData=true`.
//! - Mutations are owner-scoped; reads allow owned or public playlists.
//! - `DELETE /Items/{id}` deletes owned playlists only; anything else →
//!   404 "Item not found or cannot be deleted."
//! - **D4 deviation (ADR-0129/0130 precedent):** the legacy
//!   add-to-nonempty-playlist latent corruption (500) is not preserved; add is
//!   the clean append the native-v1 store provides. The empty-add 204 no-op is
//!   preserved as captured.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::auth::resolve_token;
use super::browse::{has_role, json_response, raw_query};
use super::error::{bad_request, not_found, unauthorized};
use super::model::{self, PlaylistFacts, SongFacts, SongShape};
use melodee_persistence::{
    NativePlaylistCreate, NativePlaylistUpdate, NativeUserStateStore, NativeUserStateStoreError,
};

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/Playlists", get(list_playlists).post(create_playlist))
        .route(
            "/Playlists/{id}",
            get(get_playlist)
                .post(update_playlist)
                .delete(delete_playlist),
        )
        .route(
            "/Playlists/{id}/Items",
            get(playlist_items).post(add_items).delete(remove_items),
        )
        .route("/Playlists/{id}/Items/{item}/Move/{index}", post(move_item))
        .route("/Items/{id}", delete(delete_item))
}

/// Playlist counters (child count + runtime) — legacy maintains them on the
/// Playlist row; the adapter computes them from the durable items.
async fn playlist_counters(state: &JfState, playlist_public_id: Uuid) -> (i64, i64) {
    sqlx::query(
        "SELECT count(*), COALESCE(sum(track.duration_ms), 0)::bigint \
           FROM native_playlist_items item \
           JOIN native_playlists playlist ON playlist.id = item.playlist_id \
           JOIN tracks track ON track.id = item.track_id \
          WHERE playlist.public_api_id = $1",
    )
    .bind(playlist_public_id)
    .fetch_one(&state.pool)
    .await
    .map(|row| (row.get::<i64, _>(0), row.get::<i64, _>(1)))
    .unwrap_or((0, 0))
}

async fn playlist_facts(
    state: &JfState,
    playlist: &melodee_persistence::NativePlaylist,
) -> PlaylistFacts {
    let (song_count, duration_ms) = playlist_counters(state, playlist.public_api_id).await;
    PlaylistFacts {
        public_id: playlist.public_api_id,
        name: playlist.name.clone(),
        comment: playlist.description.clone(),
        song_count,
        duration_ms,
        created_at: playlist.created_at,
        updated_at: playlist.updated_at,
    }
}

async fn list_playlists(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let params = raw_query(&uri);
    let mut skip: i64 = 0;
    let mut take: i64 = 100;
    for (key, value) in &params {
        match key.as_str() {
            "startIndex" => skip = value.parse::<i64>().unwrap_or(0).max(0),
            "limit" => take = value.parse::<i64>().unwrap_or(100).clamp(1, 500),
            _ => {}
        }
    }
    // Captured handler pages in memory after fetching take+skip rows. The
    // Jellyfin's captured `/Playlists` contract orders ascending by creation
    // time (`jf-playlists/playlists-list`); native-v1 has a separate
    // name-ascending list contract.
    let page_size = (take + skip).clamp(1, 1000) as u16;
    let (total, playlists) = match NativeUserStateStore::playlists_oldest_first(
        &state.pool,
        principal.user_id,
        1,
        page_size,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(%error, "jellyfin playlist list failed");
            return not_found("Playlist not found.");
        }
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let mut items = Vec::new();
    for playlist in playlists.iter().skip(skip as usize).take(take as usize) {
        let facts = playlist_facts(&state, playlist).await;
        let mut item = model::playlist_item(&state.server_id, &facts);
        if let Value::Object(ref mut map) = item {
            map.insert("CanDownload".to_string(), serde_json::json!(can_download));
        }
        items.push(item);
    }
    json_response(model::items_result(items, total, skip))
}

async fn get_playlist(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let playlist =
        match NativeUserStateStore::playlist(&state.pool, principal.user_id, public_id).await {
            Ok(playlist) => playlist,
            Err(NativeUserStateStoreError::NotFound) => return not_found("Playlist not found."),
            Err(error) => {
                tracing::error!(%error, "jellyfin playlist fetch failed");
                return not_found("Playlist not found.");
            }
        };
    let facts = playlist_facts(&state, &playlist).await;
    let mut item = model::playlist_item(&state.server_id, &facts);
    if let Value::Object(ref mut map) = item {
        map.insert(
            "CanDownload".to_string(),
            serde_json::json!(has_role(&principal, "HasDownloadRole")),
        );
    }
    let etag = model::item_etag(public_id, playlist.updated_at);
    let not_modified = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_matches('"').eq_ignore_ascii_case(&etag));
    let quoted = format!("\"{etag}\"");
    if not_modified {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = axum::http::HeaderValue::from_str(&quoted) {
            response
                .headers_mut()
                .insert(axum::http::header::ETAG, value);
        }
        return response;
    }
    let mut response = json_response(item);
    if let Ok(value) = axum::http::HeaderValue::from_str(&quoted) {
        response
            .headers_mut()
            .insert(axum::http::header::ETAG, value);
    }
    response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreatePlaylistRequest {
    name: Option<String>,
    ids: Option<Vec<String>>,
    is_public: Option<bool>,
}

async fn create_playlist(
    State(state): State<JfState>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let request: CreatePlaylistRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => CreatePlaylistRequest {
            name: None,
            ids: None,
            is_public: None,
        },
    };
    if request
        .name
        .as_deref()
        .is_none_or(|name| name.trim().is_empty())
    {
        return bad_request("Playlist name is required.");
    }
    // Captured: invalid ids in the create list are silently skipped.
    let track_ids: Vec<Uuid> = request
        .ids
        .unwrap_or_default()
        .iter()
        .filter_map(|id| model::parse_id(id.trim()))
        .collect();
    let Some(name) = request.name else {
        return bad_request("Playlist name is required.");
    };
    match NativeUserStateStore::create_playlist(
        &state.pool,
        principal.user_id,
        &NativePlaylistCreate {
            public_api_id: None,
            name,
            description: None,
            is_public: request.is_public.unwrap_or(false),
            track_public_ids: track_ids,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(playlist) => json_response(serde_json::json!({
            "Id": model::nf(playlist.public_api_id)
        })),
        Err(error) => {
            tracing::error!(%error, "jellyfin playlist create failed");
            bad_request("Failed to create playlist.")
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdatePlaylistRequest {
    name: Option<String>,
    is_public: Option<bool>,
}

async fn update_playlist(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let request: UpdatePlaylistRequest =
        serde_json::from_slice(&body).unwrap_or(UpdatePlaylistRequest {
            name: None,
            is_public: None,
        });
    // Captured: blank names are ignored rather than rejected.
    let name = request.name.filter(|name| !name.trim().is_empty());
    match NativeUserStateStore::update_playlist(
        &state.pool,
        principal.user_id,
        public_id,
        &NativePlaylistUpdate {
            name,
            description: None,
            is_public: request.is_public,
        },
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(NativeUserStateStoreError::NotFound) => {
            not_found("Playlist not found or access denied.")
        }
        Err(error) => {
            tracing::error!(%error, "jellyfin playlist update failed");
            not_found("Playlist not found or access denied.")
        }
    }
}

async fn delete_playlist(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    match NativeUserStateStore::delete_playlist(&state.pool, principal.user_id, public_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => not_found("Playlist not found or access denied."),
    }
}

/// DELETE /Items/{id}: deletes owned playlists only (captured); anything
/// else → 404 "Item not found or cannot be deleted."
async fn delete_item(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let owner_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_playlists WHERE public_api_id = $1 AND owner_user_id = $2",
    )
    .bind(public_id)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);
    if owner_count == 0 {
        return not_found("Item not found or cannot be deleted.");
    }
    match NativeUserStateStore::delete_playlist(&state.pool, principal.user_id, public_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => not_found("Item not found or cannot be deleted."),
    }
}

async fn playlist_items(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let params = raw_query(&uri);
    let mut skip: i64 = 0;
    let mut take: i64 = 100;
    let mut enable_user_data = false;
    for (key, value) in &params {
        match key.as_str() {
            "startIndex" => skip = value.parse::<i64>().unwrap_or(0).max(0),
            "limit" => take = value.parse::<i64>().unwrap_or(100).clamp(1, 500),
            "enableUserData" => enable_user_data = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    // Visible playlists only (owned or public), matching the captured
    // fetch-private 404 class.
    if let Err(error) =
        NativeUserStateStore::playlist(&state.pool, principal.user_id, public_id).await
    {
        return match error {
            NativeUserStateStoreError::NotFound => not_found("Playlist not found."),
            error => {
                tracing::error!(%error, "jellyfin playlist items probe failed");
                not_found("Playlist not found.")
            }
        };
    }
    let items =
        match NativeUserStateStore::playlist_items(&state.pool, principal.user_id, public_id).await
        {
            Ok(items) => items,
            Err(error) => {
                tracing::error!(%error, "jellyfin playlist items failed");
                return not_found("Playlist not found.");
            }
        };
    let total = items.len() as i64;
    let can_download = has_role(&principal, "HasDownloadRole");
    let mut rendered = Vec::new();
    for (index, item) in items
        .into_iter()
        .enumerate()
        .skip(skip as usize)
        .take(take as usize)
    {
        let Some(song) = load_playlist_song(&state, item.track_public_id).await else {
            continue;
        };
        let mut facts: SongFacts = song;
        if enable_user_data {
            // The only captured surface that hydrates real user state.
            facts.user_data = Some(
                load_playlist_user_data(&state, principal.user_id, item.track_public_id).await,
            );
        }
        let mut value = model::song_item(
            &state.server_id,
            &facts,
            SongShape::PlaylistEntry {
                position: index as i32 + 1,
            },
        );
        if let Value::Object(ref mut map) = value {
            map.insert("CanDownload".to_string(), serde_json::json!(can_download));
            // Captured: playlist entry ETags hash the song's CreatedAt.
            map.insert(
                "Etag".to_string(),
                serde_json::json!(model::item_etag(facts.public_id, facts.created_at)),
            );
        }
        rendered.push(value);
    }
    json_response(model::items_result(rendered, total, skip))
}

async fn load_playlist_song(state: &JfState, track_public_id: Uuid) -> Option<SongFacts> {
    let mut facts = super::browse::load_song(&state.pool, track_public_id)
        .await
        .ok()
        .flatten()?;
    // Playlist entries never carry UserData unless enableUserData=true.
    facts.user_data = None;
    Some(facts)
}

async fn load_playlist_user_data(state: &JfState, user_id: Uuid, track_public_id: Uuid) -> Value {
    let row = sqlx::query(
        "SELECT state.starred, state.play_count, state.last_played_at \
           FROM native_track_user_states state \
           JOIN tracks track ON track.id = state.track_id \
          WHERE state.user_id = $1 AND track.public_api_id = $2",
    )
    .bind(user_id)
    .bind(track_public_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (starred, play_count, last_played) = match row {
        Some(row) => (row.get(0), row.get(1), row.get(2)),
        None => (false, 0, None),
    };
    model::user_data(
        play_count,
        starred,
        &model::nf(track_public_id),
        last_played,
    )
}

fn parse_id_list(raw: Option<&str>) -> Vec<Uuid> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter_map(model::parse_id)
        .collect()
}

async fn add_items(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let params = raw_query(&uri);
    let ids = parse_id_list(
        params
            .iter()
            .find(|(key, _)| key == "ids")
            .map(|(_, value)| value.as_str()),
    );
    // Captured: blank/all-invalid ids are a 204 no-op.
    if ids.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    // D4 deviation: the legacy latent add corruption (500 on non-empty
    // playlists) is not preserved; this is the clean append path.
    match NativeUserStateStore::append_playlist_tracks(
        &state.pool,
        principal.user_id,
        public_id,
        &ids,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(NativeUserStateStoreError::NotFound) => {
            not_found("Playlist not found or access denied.")
        }
        Err(error) => {
            tracing::error!(%error, "jellyfin playlist add failed");
            not_found("Playlist not found or access denied.")
        }
    }
}

async fn remove_items(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let params = raw_query(&uri);
    let ids = parse_id_list(
        params
            .iter()
            .find(|(key, _)| key == "entryIds")
            .map(|(_, value)| value.as_str()),
    );
    if ids.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    match NativeUserStateStore::remove_playlist_tracks(
        &state.pool,
        principal.user_id,
        public_id,
        &ids,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(NativeUserStateStoreError::NotFound) => {
            not_found("Playlist not found or access denied.")
        }
        Err(error) => {
            tracing::error!(%error, "jellyfin playlist remove failed");
            not_found("Playlist not found or access denied.")
        }
    }
}

async fn move_item(
    State(state): State<JfState>,
    Path((id, item, index)): Path<(String, String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    // The legacy route has an `:int` constraint; a non-integer segment never
    // reaches the controller (framework 404 without a body).
    let Ok(new_index) = index.parse::<usize>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(playlist_public_id) = model::parse_id(&id) else {
        return bad_request("Invalid playlist ID format.");
    };
    let Some(track_public_id) = model::parse_id(&item) else {
        return bad_request("Invalid item ID format.");
    };
    let items = match NativeUserStateStore::playlist_items(
        &state.pool,
        principal.user_id,
        playlist_public_id,
    )
    .await
    {
        Ok(items) => items,
        Err(_) => return not_found("Playlist not found."),
    };
    // Ownership scoping hardening: the legacy move handler did not scope to
    // the owner (captured oversight); the adapter requires ownership like the
    // other playlist mutations. Documented Phase 8 decision.
    let owned = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_playlists WHERE public_api_id = $1 AND owner_user_id = $2",
    )
    .bind(playlist_public_id)
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0)
        > 0;
    if !owned {
        return not_found("Playlist not found.");
    }
    let Some(current_index) = items
        .iter()
        .position(|entry| entry.track_public_id == track_public_id)
    else {
        return not_found("Item not found in playlist.");
    };
    if current_index == new_index || new_index >= items.len() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let mut ordered: Vec<Uuid> = items.iter().map(|entry| entry.track_public_id).collect();
    let moved = ordered.remove(current_index);
    ordered.insert(new_index, moved);
    match NativeUserStateStore::reorder_playlist_tracks(
        &state.pool,
        principal.user_id,
        playlist_public_id,
        &ordered,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => not_found("Playlist not found."),
    }
}
