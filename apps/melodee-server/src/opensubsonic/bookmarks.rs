//! Bookmark endpoint group: getBookmarks, createBookmark, deleteBookmark
//! (PRD-API-OS-005; golden evidence `compatibility/opensubsonic/golden/playlists/os-bookmarks-*`).
//!
//! Bookmarks persist to `opensubsonic_bookmarks` (migration 0020). The raw
//! `position` value is preserved verbatim — the legacy unit is inconsistent
//! (controller doc says ms, model doc says seconds) and the captures keep
//! whatever the client sent.

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
    annotation::{
        EntityRef, authenticated_user, parse_prefixed_id, rfc3339_z, song_children,
        track_id_for_public_id,
    },
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

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

/// Resolves the captured `id` parameter to an internal Track id.
async fn resolve_track_param(
    state: &OsState,
    params: &Option<Extension<Params>>,
    raw_id: Option<&str>,
) -> Result<Uuid, Response> {
    let Some(raw_id) = raw_id else {
        return Err(error_reply(
            state,
            params,
            10,
            "Required parameter is missing.",
        ));
    };
    let Some(EntityRef::Track(public_id)) = parse_prefixed_id(raw_id) else {
        return Err(error_reply(state, params, 44, "Invalid API key."));
    };
    match track_id_for_public_id(&state.pool, public_id).await {
        Ok(Some(track_id)) => Ok(track_id),
        Ok(None) => Err(error_reply(
            state,
            params,
            70,
            "The requested data was not found.",
        )),
        Err(error) => {
            tracing::error!(%error, "opensubsonic bookmark track resolution failed");
            Err(error_reply(state, params, 0, "A generic error."))
        }
    }
}

pub async fn get_bookmarks(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let rows = match sqlx::query(
        "SELECT bookmark.track_id, track.public_api_id, bookmark.position, bookmark.comment,
                bookmark.created_at, bookmark.updated_at
         FROM opensubsonic_bookmarks bookmark
         JOIN tracks track ON track.id = bookmark.track_id
         WHERE bookmark.user_id = $1
         ORDER BY bookmark.created_at, track.public_api_id",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getBookmarks failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let mut bookmarks = Vec::with_capacity(rows.len());
    for row in rows {
        let public_id: Uuid = match row.try_get(1) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "opensubsonic bookmark row decode failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        };
        let position: i64 = row.try_get(2).unwrap_or(0);
        let comment: Option<String> = row.try_get(3).unwrap_or(None);
        let created_at: OffsetDateTime = match row.try_get(4) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "opensubsonic bookmark row decode failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        };
        let updated_at: OffsetDateTime = match row.try_get(5) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "opensubsonic bookmark row decode failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        };
        let entries = match song_children(&state.pool, user.user_id, &[public_id]).await {
            Ok(entries) => entries,
            Err(error) => {
                tracing::error!(%error, "opensubsonic bookmark child projection failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        };
        bookmarks.push(serde_json::json!({
            "position": position,
            "username": user.username,
            "comment": comment,
            "created": rfc3339_z(created_at),
            "changed": rfc3339_z(updated_at),
            "entry": entries,
        }));
    }
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"bookmarks": {"bookmark": bookmarks}})),
    )
}

pub async fn create_bookmark(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let raw_id = params.as_ref().and_then(|Extension(p)| p.get("id"));
    let position = params
        .as_ref()
        .and_then(|Extension(p)| p.get_i64("position"))
        .unwrap_or(0);
    let comment = params
        .as_ref()
        .and_then(|Extension(p)| p.get("comment"))
        .map(str::to_string);
    let track_id = match resolve_track_param(&state, &params, raw_id).await {
        Ok(track_id) => track_id,
        Err(response) => return response,
    };
    if position < 0 {
        return error_reply(&state, &params, 10, "Position must not be negative.");
    }
    let now = OffsetDateTime::now_utc();
    if let Err(error) = sqlx::query(
        "INSERT INTO opensubsonic_bookmarks (
            user_id, track_id, position, comment, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
            position = excluded.position,
            comment = excluded.comment,
            updated_at = excluded.updated_at",
    )
    .bind(user.user_id)
    .bind(track_id)
    .bind(position)
    .bind(comment)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        tracing::error!(%error, "opensubsonic createBookmark failed");
        return error_reply(&state, &params, 0, "A generic error.");
    }
    reply(&state, &params, OsReply::ok(serde_json::json!({})))
}

pub async fn delete_bookmark(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let raw_id = params.as_ref().and_then(|Extension(p)| p.get("id"));
    let track_id = match resolve_track_param(&state, &params, raw_id).await {
        Ok(track_id) => track_id,
        Err(response) => return response,
    };
    // Deleting an absent bookmark is a captured no-op ok.
    if let Err(error) =
        sqlx::query("DELETE FROM opensubsonic_bookmarks WHERE user_id = $1 AND track_id = $2")
            .bind(user.user_id)
            .bind(track_id)
            .execute(&state.pool)
            .await
    {
        tracing::error!(%error, "opensubsonic deleteBookmark failed");
        return error_reply(&state, &params, 0, "A generic error.");
    }
    reply(&state, &params, OsReply::ok(serde_json::json!({})))
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getBookmarks", get_bookmarks))
        .merge(super::endpoint_router("createBookmark", create_bookmark))
        .merge(super::endpoint_router("deleteBookmark", delete_bookmark))
}
