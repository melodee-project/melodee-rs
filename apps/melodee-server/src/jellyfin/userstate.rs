//! Jellyfin per-user item state: FavoriteItems and PlayedItems
//! (PRD-API-JF-001/004).
//!
//! Reproduced from the frozen baseline (`UsersController.cs`):
//!
//! - The `{userId}` path segment must equal the bearer's public id, else
//!   403 "Cannot modify favorites/played status for other users."; an
//!   unparseable userId takes the same 403 path (captured).
//! - Entity probing order is song → album → artist for favorites, song →
//!   album for played (artists are not playable → 404 "Item not found.").
//! - Responses are the partial `JellyfinUserItemData` (favorites echo
//!   `IsFavorite`/`Key`; played echo `Played`/`PlayCount`/`LastPlayedDate`).
//! - Played state maps to the native per-entity play counters; unplayed
//!   resets the counter and clears the last-played timestamp (captured).

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, Uri},
    response::Response,
    routing::post,
};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::auth::resolve_token;
use super::browse::json_response;
use super::error::{bad_request, forbidden, not_found, unauthorized};
use super::model;
use melodee_persistence::{NativeUserStateMutation, NativeUserStateStore};

pub fn router() -> Router<JfState> {
    Router::new()
        .route(
            "/Users/{user}/FavoriteItems/{item}",
            post(add_favorite).delete(remove_favorite),
        )
        .route(
            "/Users/{user}/PlayedItems/{item}",
            post(mark_played).delete(mark_unplayed),
        )
}

enum EntityKind {
    Track(Uuid),
    Release(Uuid),
    Artist(Uuid),
}

/// Probes track → release → artist by public id (captured order).
async fn resolve_entity(
    state: &JfState,
    public_id: Uuid,
) -> Result<Option<EntityKind>, sqlx::Error> {
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tracks track \
         JOIN releases release ON release.id = track.release_id \
         WHERE track.public_api_id = $1 AND release.status = 'active'",
    )
    .bind(public_id)
    .fetch_one(&state.pool)
    .await?
        > 0
    {
        return Ok(Some(EntityKind::Track(public_id)));
    }
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM releases WHERE public_api_id = $1 AND status = 'active'",
    )
    .bind(public_id)
    .fetch_one(&state.pool)
    .await?
        > 0
    {
        return Ok(Some(EntityKind::Release(public_id)));
    }
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM artists WHERE public_api_id = $1 AND status NOT IN ('merged', 'deprecated')",
    )
    .bind(public_id)
    .fetch_one(&state.pool)
    .await?
        > 0
    {
        return Ok(Some(EntityKind::Artist(public_id)));
    }
    Ok(None)
}

fn user_data_partial(fields: &[(&str, Value)], key: Uuid) -> Value {
    let mut body = serde_json::json!({
        "PlaybackPositionTicks": 0,
        "PlayCount": 0,
        "IsFavorite": false,
        "Played": false,
        "Key": model::nf(key),
        "LastPlayedDate": null,
    });
    if let Value::Object(ref mut map) = body {
        for (name, value) in fields {
            map.insert((*name).to_string(), value.clone());
        }
    }
    body
}

/// Captured gate: the path user id must parse and equal the bearer id.
async fn require_self(
    state: &JfState,
    headers: &HeaderMap,
    uri: &Uri,
    user_id: &str,
    other_user_detail: &str,
) -> Result<super::auth::JfPrincipal, Box<Response>> {
    let Some(principal) = resolve_token(state, headers, uri).await else {
        return Err(Box::new(unauthorized()));
    };
    // Captured: the path userId is the public id and must equal the bearer's
    // public id (not the internal row id).
    let parsed = model::parse_id(user_id);
    if parsed != Some(principal.public_api_id) {
        return Err(Box::new(forbidden(other_user_detail)));
    }
    Ok(principal)
}

async fn add_favorite(
    State(state): State<JfState>,
    Path((user_id, item_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let principal = match require_self(
        &state,
        &headers,
        &uri,
        &user_id,
        "Cannot modify favorites for other users.",
    )
    .await
    {
        Ok(principal) => principal,
        Err(response) => return *response,
    };
    let Some(item_id) = model::parse_id(&item_id) else {
        return bad_request("Invalid item ID format.");
    };
    let entity = match resolve_entity(&state, item_id).await {
        Ok(Some(entity)) => entity,
        Ok(None) => return not_found("Item not found."),
        Err(error) => {
            tracing::error!(%error, "jellyfin favorite probe failed");
            return not_found("Item not found.");
        }
    };
    let now = OffsetDateTime::now_utc();
    let result = match entity {
        EntityKind::Track(id) => {
            NativeUserStateStore::mutate_track(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(true),
                now,
            )
            .await
        }
        EntityKind::Release(id) => {
            NativeUserStateStore::mutate_release(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(true),
                now,
            )
            .await
        }
        EntityKind::Artist(id) => {
            NativeUserStateStore::mutate_artist(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(true),
                now,
            )
            .await
        }
    };
    match result {
        Ok(_) => json_response(user_data_partial(
            &[("IsFavorite", Value::Bool(true))],
            item_id,
        )),
        Err(error) => {
            tracing::error!(%error, "jellyfin favorite add failed");
            not_found("Item not found.")
        }
    }
}

async fn remove_favorite(
    State(state): State<JfState>,
    Path((user_id, item_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let principal = match require_self(
        &state,
        &headers,
        &uri,
        &user_id,
        "Cannot modify favorites for other users.",
    )
    .await
    {
        Ok(principal) => principal,
        Err(response) => return *response,
    };
    let Some(item_id) = model::parse_id(&item_id) else {
        return bad_request("Invalid item ID format.");
    };
    let entity = match resolve_entity(&state, item_id).await {
        Ok(Some(entity)) => entity,
        Ok(None) => return not_found("Item not found."),
        Err(error) => {
            tracing::error!(%error, "jellyfin favorite probe failed");
            return not_found("Item not found.");
        }
    };
    let now = OffsetDateTime::now_utc();
    let result = match entity {
        EntityKind::Track(id) => {
            NativeUserStateStore::mutate_track(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(false),
                now,
            )
            .await
        }
        EntityKind::Release(id) => {
            NativeUserStateStore::mutate_release(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(false),
                now,
            )
            .await
        }
        EntityKind::Artist(id) => {
            NativeUserStateStore::mutate_artist(
                &state.pool,
                principal.user_id,
                id,
                NativeUserStateMutation::Starred(false),
                now,
            )
            .await
        }
    };
    match result {
        Ok(_) => json_response(user_data_partial(
            &[("IsFavorite", Value::Bool(false))],
            item_id,
        )),
        Err(error) => {
            tracing::error!(%error, "jellyfin favorite remove failed");
            not_found("Item not found.")
        }
    }
}

/// Captured mark-played: increments the per-user per-entity play counter and
/// stamps LastPlayedAt; the response echoes the cumulative count.
async fn bump_play_count(
    state: &JfState,
    user_id: Uuid,
    table: &str,
    column: &str,
    entity_table: &str,
    public_id: Uuid,
    now: OffsetDateTime,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} (user_id, {column}, play_count, last_played_at, created_at, updated_at) \
         SELECT $1, entity.id, 1, $3, $3, $3 FROM {entity_table} entity WHERE entity.public_api_id = $2 \
         ON CONFLICT (user_id, {column}) DO UPDATE \
            SET play_count = {table}.play_count + 1, last_played_at = $3, updated_at = $3 \
         RETURNING {table}.play_count"
    )))
    .bind(user_id)
    .bind(public_id)
    .bind(now)
    .fetch_one(&state.pool)
    .await
}

async fn reset_play_count(
    state: &JfState,
    user_id: Uuid,
    table: &str,
    column: &str,
    entity_table: &str,
    public_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET play_count = 0, last_played_at = NULL, updated_at = $3 \
          FROM (SELECT id FROM {entity_table} WHERE public_api_id = $2) entity \
         WHERE {table}.user_id = $1 AND {table}.{column} = entity.id"
    )))
    .bind(user_id)
    .bind(public_id)
    .bind(now)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn mark_played(
    State(state): State<JfState>,
    Path((user_id, item_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let principal = match require_self(
        &state,
        &headers,
        &uri,
        &user_id,
        "Cannot modify played status for other users.",
    )
    .await
    {
        Ok(principal) => principal,
        Err(response) => return *response,
    };
    let Some(item_id) = model::parse_id(&item_id) else {
        return bad_request("Invalid item ID format.");
    };
    let entity = match resolve_entity(&state, item_id).await {
        Ok(Some(entity)) => entity,
        Ok(None) => return not_found("Item not found."),
        Err(error) => {
            tracing::error!(%error, "jellyfin played probe failed");
            return not_found("Item not found.");
        }
    };
    let now = OffsetDateTime::now_utc();
    let play_count = match entity {
        // Captured: artists are not playable and fall through to 404.
        EntityKind::Artist(_) => return not_found("Item not found."),
        EntityKind::Track(id) => {
            bump_play_count(
                &state,
                principal.user_id,
                "native_track_user_states",
                "track_id",
                "tracks",
                id,
                now,
            )
            .await
        }
        EntityKind::Release(id) => {
            bump_play_count(
                &state,
                principal.user_id,
                "native_release_user_states",
                "release_id",
                "releases",
                id,
                now,
            )
            .await
        }
    };
    match play_count {
        Ok(count) => json_response(user_data_partial(
            &[
                ("Played", Value::Bool(true)),
                ("PlayCount", serde_json::json!(count)),
                ("LastPlayedDate", Value::String(model::format_date(now))),
            ],
            item_id,
        )),
        Err(error) => {
            tracing::error!(%error, "jellyfin mark played failed");
            not_found("Item not found.")
        }
    }
}

async fn mark_unplayed(
    State(state): State<JfState>,
    Path((user_id, item_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let principal = match require_self(
        &state,
        &headers,
        &uri,
        &user_id,
        "Cannot modify played status for other users.",
    )
    .await
    {
        Ok(principal) => principal,
        Err(response) => return *response,
    };
    let Some(item_id) = model::parse_id(&item_id) else {
        return bad_request("Invalid item ID format.");
    };
    let entity = match resolve_entity(&state, item_id).await {
        Ok(Some(entity)) => entity,
        Ok(None) => return not_found("Item not found."),
        Err(error) => {
            tracing::error!(%error, "jellyfin unplayed probe failed");
            return not_found("Item not found.");
        }
    };
    let now = OffsetDateTime::now_utc();
    let result = match entity {
        EntityKind::Artist(_) => return not_found("Item not found."),
        EntityKind::Track(id) => {
            reset_play_count(
                &state,
                principal.user_id,
                "native_track_user_states",
                "track_id",
                "tracks",
                id,
                now,
            )
            .await
        }
        EntityKind::Release(id) => {
            reset_play_count(
                &state,
                principal.user_id,
                "native_release_user_states",
                "release_id",
                "releases",
                id,
                now,
            )
            .await
        }
    };
    match result {
        Ok(_) => json_response(user_data_partial(
            &[
                ("Played", Value::Bool(false)),
                ("PlayCount", serde_json::json!(0)),
            ],
            item_id,
        )),
        Err(error) => {
            tracing::error!(%error, "jellyfin mark unplayed failed");
            not_found("Item not found.")
        }
    }
}
