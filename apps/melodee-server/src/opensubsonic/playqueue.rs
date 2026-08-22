//! Play queue endpoint group: savePlayQueue, getPlayQueue
//! (PRD-API-OS-005; golden evidence `compatibility/opensubsonic/golden/playlists/os-playqueue-*`).
//!
//! State lives in `opensubsonic_play_queue_items` + `opensubsonic_play_queues`
//! (migration 0020). Captured quirks preserved: //! - The response `current` field is the internal bigint entry id (the legacy
//! PlayQueId), NOT the song id the client submitted.
//! - Clearing (savePlayQueue without ids) empties the queue; getPlayQueue on
//! an empty queue answers the synthesized default whose `changedBy` is the
//! username (not the last saving client).
//! - `c` (client name) is stored as `changed_by`.

use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    annotation::{EntityRef, authenticated_user, parse_prefixed_id, rfc3339_z, song_children},
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

/// Defensive bound on one savePlayQueue submission.
const MAX_PLAY_QUEUE_ITEMS: usize = 10_000;

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(state: &OsState, params: &Option<Extension<Params>>, reply: OsReply) -> Response {
    reply.respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn error_reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    code: i64,
    message: &str,
) -> Response {
    reply(state, params, OsReply::error(code, message))
}

/// The captured default for a user with no saved queue: zeroed position and
/// `changedBy` equal to the username (os-playqueue-getplayqueue-empty).
fn empty_queue_payload(username: &str) -> serde_json::Value {
    serde_json::json!({
        "playQueue": {
            "current": 0,
            "position": 0,
            "changedBy": username,
            "changed": rfc3339_z(OffsetDateTime::now_utc()),
            "username": username,
            "entry": [],
        }
    })
}

pub async fn get_play_queue(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let queue = match sqlx::query(
        "SELECT current_entry_id, position, changed_by, changed_at
         FROM opensubsonic_play_queues WHERE user_id = $1",
    )
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(queue) => queue,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlayQueue failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let Some(queue) = queue else {
        return reply(
            &state,
            &params,
            OsReply::ok(empty_queue_payload(&user.username)),
        );
    };
    let item_rows = match sqlx::query(
        "SELECT track.public_api_id
         FROM opensubsonic_play_queue_items item
         JOIN tracks track ON track.id = item.track_id
         WHERE item.user_id = $1
         ORDER BY item.queue_position, track.public_api_id",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlayQueue items failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let mut public_ids = Vec::with_capacity(item_rows.len());
    for row in item_rows {
        match row.try_get::<Uuid, _>(0) {
            Ok(public_id) => public_ids.push(public_id),
            Err(error) => {
                tracing::error!(%error, "opensubsonic getPlayQueue row decode failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        }
    }
    let entries = match song_children(&state.pool, user.user_id, &public_ids).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlayQueue child projection failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let current: Option<i64> = queue.try_get(0).unwrap_or(None);
    let position: Option<i64> = queue.try_get(1).unwrap_or(None);
    let changed_by: String = queue.try_get(2).unwrap_or_else(|_| user.username.clone());
    let changed_at: OffsetDateTime = match queue.try_get(3) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlayQueue row decode failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({
            "playQueue": {
                "current": current.unwrap_or(0),
                "position": position.unwrap_or(0),
                "changedBy": changed_by,
                "changed": rfc3339_z(changed_at),
                "username": user.username,
                "entry": entries,
            }
        })),
    )
}

pub async fn save_play_queue(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let raw_ids: Vec<String> = params
        .as_ref()
        .map_or_else(Vec::new, |Extension(p)| p.get_all("id").to_vec());
    if raw_ids.len() > MAX_PLAY_QUEUE_ITEMS {
        return error_reply(&state, &params, 10, "Too many songs in play queue.");
    }
    let mut track_ids: Vec<Uuid> = Vec::with_capacity(raw_ids.len());
    for raw in &raw_ids {
        let Some(EntityRef::Track(public_id)) = parse_prefixed_id(raw) else {
            return error_reply(&state, &params, 44, "Invalid API key.");
        };
        track_ids.push(public_id);
    }
    let current_public_id: Option<Uuid> =
        match params.as_ref().and_then(|Extension(p)| p.get("current")) {
            Some(raw) => match parse_prefixed_id(raw) {
                Some(EntityRef::Track(public_id)) => Some(public_id),
                _ => return error_reply(&state, &params, 44, "Invalid API key."),
            },
            None => None,
        };
    let position = params
        .as_ref()
        .and_then(|Extension(p)| p.get_i64("position"))
        .unwrap_or(0);
    if position < 0 {
        return error_reply(&state, &params, 10, "Position must not be negative.");
    }
    let changed_by = params
        .as_ref()
        .and_then(|Extension(p)| p.get("c"))
        .filter(|value| !value.trim().is_empty())
        .map_or_else(|| user.username.clone(), str::to_string);

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, "opensubsonic savePlayQueue transaction failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let result = save_queue_in_transaction(
        &mut tx,
        &user,
        &track_ids,
        current_public_id,
        position,
        &changed_by,
    )
    .await;
    match result {
        Ok(()) => {
            if let Err(error) = tx.commit().await {
                tracing::error!(%error, "opensubsonic savePlayQueue commit failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
            reply(&state, &params, OsReply::ok(serde_json::json!({})))
        }
        Err(SaveQueueError::UnknownTrack) => {
            error_reply(&state, &params, 70, "The requested data was not found.")
        }
        Err(SaveQueueError::Database(error)) => {
            tracing::error!(%error, "opensubsonic savePlayQueue failed");
            error_reply(&state, &params, 0, "A generic error.")
        }
    }
}

enum SaveQueueError {
    UnknownTrack,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SaveQueueError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

/// Replaces the user's queue atomically: items first (to learn the identity
/// entry ids), then the queue row whose `current_entry_id` points at the
/// entry whose track matches the submitted `current` song id.
async fn save_queue_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: &OsPrincipal,
    track_public_ids: &[Uuid],
    current_public_id: Option<Uuid>,
    position: i64,
    changed_by: &str,
) -> Result<(), SaveQueueError> {
    sqlx::query("DELETE FROM opensubsonic_play_queue_items WHERE user_id = $1")
        .bind(user.user_id)
        .execute(&mut **tx)
        .await?;
    if track_public_ids.is_empty() {
        // Clearing removes the queue row entirely; getPlayQueue then answers
        // the captured synthesized default.
        sqlx::query("DELETE FROM opensubsonic_play_queues WHERE user_id = $1")
            .bind(user.user_id)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }
    let mut current_entry_id: Option<i64> = None;
    for (index, public_id) in track_public_ids.iter().enumerate() {
        // Bounded by MAX_PLAY_QUEUE_ITEMS at the handler boundary.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let queue_position = index as i32;
        let entry_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO opensubsonic_play_queue_items (
                user_id, track_id, queue_position, created_at
             ) SELECT $1, tracks.id, $3, now() FROM tracks WHERE tracks.public_api_id = $2
             RETURNING id",
        )
        .bind(user.user_id)
        .bind(public_id)
        .bind(queue_position)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(SaveQueueError::UnknownTrack)?;
        if Some(*public_id) == current_public_id {
            current_entry_id = Some(entry_id);
        }
    }
    sqlx::query(
        "INSERT INTO opensubsonic_play_queues (
            user_id, current_entry_id, position, changed_by, changed_at, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, now(), now(), now())
         ON CONFLICT (user_id) DO UPDATE SET
            current_entry_id = excluded.current_entry_id,
            position = excluded.position,
            changed_by = excluded.changed_by,
            changed_at = excluded.changed_at,
            updated_at = excluded.updated_at",
    )
    .bind(user.user_id)
    .bind(current_entry_id)
    .bind(position)
    .bind(changed_by)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getPlayQueue", get_play_queue))
        .merge(super::endpoint_router("savePlayQueue", save_play_queue))
}
