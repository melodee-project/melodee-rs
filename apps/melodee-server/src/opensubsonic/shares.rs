//! Shares endpoint group: getShares, createShare, updateShare, deleteShare
//! (PRD-API-OS-005; golden evidence
//! `compatibility/opensubsonic/golden/community/os-shares-*`).
//!
//! Captured contract (conditional, CanShare role): `getShares` lists
//! `share_{guid}` entries with name/url/comment/owner/created/changed/visited/
//! allowedUsers plus the shared `entry` song children. `createShare` takes
//! `id` (song/album/artist) + optional `description`/`expires`; the share url
//! is `{public_origin}/share/{unique}`. `updateShare`/`deleteShare` by
//! `share_{guid}`. Unknown share ids on update/delete answer the captured
//! error 44 "Invalid API key." shape (the legacy fixture captured
//! `{shareId}` template ids that never resolved). Shares reuse the
//! `native_shares` table (migration 0012) so a user's library is shared
//! across surfaces; only the OpenSubsonic wire shape lives here.

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
    annotation::{EntityRef, parse_prefixed_id, rfc3339_z, song_children},
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    payload: serde_json::Value,
) -> Response {
    OsReply::ok(payload).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn error_reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    code: i64,
    message: &str,
) -> Response {
    OsReply::error(code, message).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn wire_id(public_api_id: Uuid) -> String {
    format!("share_{public_api_id}")
}

fn public_id_from_param(params: &Params, key: &str) -> Option<Uuid> {
    params.get(key).and_then(|value| {
        value
            .strip_prefix("share_")
            .and_then(|raw| Uuid::parse_str(raw).ok())
    })
}

/// `native_shares.share_type` encodes the shared entity kind: 1 = track,
/// 2 = release, 3 = artist (the 4th kind is reserved for future use).
fn share_type_for(entity: EntityRef) -> i16 {
    match entity {
        EntityRef::Track(_) => 1,
        EntityRef::Release(_) => 2,
        EntityRef::Artist(_) => 3,
    }
}

/// Collects the track public ids to render as the share `entry` children. For
/// a release share, all active tracks on the release are listed; for an
/// artist share, all active tracks across the artist's releases; for a track
/// share, just the one track.
async fn entry_track_ids(pool: &sqlx::PgPool, entity: EntityRef) -> Result<Vec<Uuid>, sqlx::Error> {
    match entity {
        EntityRef::Track(id) => {
            let row: Option<(Uuid,)> = sqlx::query_as(
                "SELECT t.public_api_id FROM tracks t
                 JOIN releases r ON r.id = t.release_id AND r.status = 'active'
                 WHERE t.public_api_id = $1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            Ok(row.into_iter().map(|(id,)| id).collect())
        }
        EntityRef::Release(id) => {
            let rows: Vec<(Uuid,)> = sqlx::query_as(
                "SELECT t.public_api_id FROM tracks t
                 JOIN releases r ON r.id = t.release_id AND r.status = 'active'
                 WHERE r.public_api_id = $1
                 ORDER BY COALESCE(t.disc_number, 1), COALESCE(t.track_number, t.ordinal), t.public_api_id",
            )
            .bind(id)
            .fetch_all(pool)
            .await?;
            Ok(rows.into_iter().map(|(id,)| id).collect())
        }
        EntityRef::Artist(id) => {
            let rows: Vec<(Uuid,)> = sqlx::query_as(
                "SELECT DISTINCT t.public_api_id FROM tracks t
                 JOIN releases r ON r.id = t.release_id AND r.status = 'active'
                 JOIN release_artists ra ON ra.release_id = r.id AND ra.role = 'primary'
                 JOIN artists a ON a.id = ra.artist_id
                 WHERE a.public_api_id = $1
                 ORDER BY t.public_api_id",
            )
            .bind(id)
            .fetch_all(pool)
            .await?;
            Ok(rows.into_iter().map(|(id,)| id).collect())
        }
    }
}

struct ShareRow {
    public_api_id: Uuid,
    share_unique_id: String,
    description: Option<String>,
    owner_username: String,
    visit_count: i64,
    created_at: OffsetDateTime,
    resource_public_api_id: Uuid,
    share_type: i16,
}

const SHARE_SELECT: &str = "SELECT s.public_api_id, s.share_unique_id, s.description,
    u.username, s.visit_count, s.created_at, s.resource_public_api_id, s.share_type
    FROM native_shares s JOIN native_users u ON u.id = s.owner_user_id";

async fn render_share(
    state: &OsState,
    user_id: Uuid,
    row: &ShareRow,
) -> Result<serde_json::Value, sqlx::Error> {
    let entity = match row.share_type {
        1 => EntityRef::Track(row.resource_public_api_id),
        2 => EntityRef::Release(row.resource_public_api_id),
        3 => EntityRef::Artist(row.resource_public_api_id),
        _ => EntityRef::Track(row.resource_public_api_id),
    };
    let track_ids = entry_track_ids(&state.pool, entity).await?;
    let entry = song_children(&state.pool, user_id, &track_ids).await?;
    let url = format!(
        "{}/share/{}",
        state.public_origin.trim_end_matches('/'),
        row.share_unique_id
    );
    Ok(serde_json::json!({
        "id": wire_id(row.public_api_id),
        "url": url,
        "description": row.description,
        "username": row.owner_username,
        "created": rfc3339_z(row.created_at),
        "visitCount": row.visit_count,
        "entry": entry,
    }))
}

pub async fn get_shares(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let Some(Extension(user)) = principal.as_ref() else {
        return error_reply(&state, &params, 40, "Wrong username or password.");
    };
    let rows = match sqlx::query(sqlx::AssertSqlSafe(format!(
        "{SHARE_SELECT} WHERE s.owner_user_id = $1 ORDER BY s.created_at DESC, s.public_api_id"
    )))
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getShares failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let mut shares = Vec::with_capacity(rows.len());
    for row in rows {
        let share_row = ShareRow {
            public_api_id: row.try_get(0).unwrap_or_else(|_| Uuid::nil()),
            share_unique_id: row.try_get(1).unwrap_or_default(),
            description: row.try_get(2).unwrap_or(None),
            owner_username: row.try_get(3).unwrap_or_default(),
            visit_count: row.try_get(4).unwrap_or(0),
            created_at: row.try_get(5).unwrap_or_else(|_| OffsetDateTime::now_utc()),
            resource_public_api_id: row.try_get(6).unwrap_or_else(|_| Uuid::nil()),
            share_type: row.try_get(7).unwrap_or(1),
        };
        match render_share(&state, user.user_id, &share_row).await {
            Ok(value) => shares.push(value),
            Err(error) => {
                tracing::error!(%error, "opensubsonic getShares render failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        }
    }
    reply(
        &state,
        &params,
        serde_json::json!({ "shares": { "share": shares } }),
    )
}

/// Captured unknown-id 302 class: a malformed `id` (not `song_`/`album_`/
/// `artist_<uuid>`) answers the credential-free 302 toward
/// `/Account/Login?ReturnUrl=%2FError` (the legacy Location embedded the
/// request query; that credential leak is not preserved).
fn redirect_to_error(state: &OsState) -> Response {
    let location = format!(
        "{}/Account/Login?ReturnUrl=%2FError",
        state.public_origin.trim_end_matches('/')
    );
    let mut response = axum::response::Response::default();
    *response.status_mut() = StatusCode::FOUND;
    if let Ok(value) = axum::http::HeaderValue::from_str(&location) {
        response
            .headers_mut()
            .insert(axum::http::header::LOCATION, value);
    }
    response
}

pub async fn create_share(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let Some(Extension(user)) = principal.as_ref() else {
        return error_reply(&state, &params, 40, "Wrong username or password.");
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(raw_id) = params_ref.get("id") else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(entity) = parse_prefixed_id(raw_id) else {
        return redirect_to_error(&state);
    };
    let description = params_ref.get("description").map(str::to_string);
    let now = OffsetDateTime::now_utc();
    let id = Uuid::now_v7();
    let public_api_id = Uuid::now_v7();
    let share_unique_id = share_unique_token();
    let share_type = share_type_for(entity);
    let resource_public_api_id = match entity {
        EntityRef::Track(id) | EntityRef::Release(id) | EntityRef::Artist(id) => id,
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO native_shares (
            id, public_api_id, share_unique_id, owner_user_id, share_type,
            resource_public_api_id, description, is_downloadable, visit_count,
            expires_at, last_visited_at, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, false, 0, NULL, NULL, $8, $8)",
    )
    .bind(id)
    .bind(public_api_id)
    .bind(&share_unique_id)
    .bind(user.user_id)
    .bind(share_type)
    .bind(resource_public_api_id)
    .bind(&description)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        tracing::error!(%error, "opensubsonic createShare insert failed");
        return error_reply(&state, &params, 0, "A generic error.");
    }
    // Render the new share in the captured `shares.share` object shape.
    let share_row = ShareRow {
        public_api_id,
        share_unique_id,
        description,
        owner_username: user.username.clone(),
        visit_count: 0,
        created_at: now,
        resource_public_api_id,
        share_type,
    };
    match render_share(&state, user.user_id, &share_row).await {
        Ok(share) => reply(
            &state,
            &params,
            serde_json::json!({ "shares": { "share": share } }),
        ),
        Err(error) => {
            tracing::error!(%error, "opensubsonic createShare render failed");
            error_reply(&state, &params, 0, "A generic error.")
        }
    }
}

pub async fn update_share(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let Some(Extension(_user)) = principal.as_ref() else {
        return error_reply(&state, &params, 40, "Wrong username or password.");
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    // The captured update golden used a `{shareId}` template id that never
    // resolved, producing the error 44 "Invalid API key." shape. A real
    // `share_<uuid>` that resolves applies the update; an unresolvable id
    // answers error 44.
    let Some(id) = public_id_from_param(params_ref, "id") else {
        return error_reply(&state, &params, 44, "Invalid API key.");
    };
    let now = OffsetDateTime::now_utc();
    let description = params_ref.get("description");
    let result = if let Some(desc) = description {
        sqlx::query(
            "UPDATE native_shares SET description = $2, updated_at = $3 WHERE public_api_id = $1",
        )
        .bind(id)
        .bind(desc)
        .bind(now)
        .execute(&state.pool)
        .await
    } else {
        sqlx::query("UPDATE native_shares SET updated_at = $2 WHERE public_api_id = $1")
            .bind(id)
            .bind(now)
            .execute(&state.pool)
            .await
    };
    match result {
        Ok(result) => {
            if result.rows_affected() == 0 {
                return error_reply(&state, &params, 44, "Invalid API key.");
            }
            reply(&state, &params, serde_json::json!({}))
        }
        Err(error) => {
            tracing::error!(%error, "opensubsonic updateShare failed");
            error_reply(&state, &params, 0, "A generic error.")
        }
    }
}

pub async fn delete_share(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let Some(Extension(_user)) = principal.as_ref() else {
        return error_reply(&state, &params, 40, "Wrong username or password.");
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(id) = public_id_from_param(params_ref, "id") else {
        return error_reply(&state, &params, 44, "Invalid API key.");
    };
    match sqlx::query("DELETE FROM native_shares WHERE public_api_id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
    {
        Ok(result) => {
            if result.rows_affected() == 0 {
                return error_reply(&state, &params, 44, "Invalid API key.");
            }
            reply(&state, &params, serde_json::json!({}))
        }
        Err(error) => {
            tracing::error!(%error, "opensubsonic deleteShare failed");
            error_reply(&state, &params, 0, "A generic error.")
        }
    }
}

/// Generates a 32-char hex share unique id (the `native_shares.share_unique_id`
/// CHECK regex `^[0-9A-Fa-f]{32}$`). Uses 16 random bytes from the platform
/// RNG, matching the legacy md5-guid shape without depending on md5.
fn share_unique_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Fall back to a v7 UUID's bytes if the platform RNG is unavailable;
        // both paths produce a 32-char hex string satisfying the CHECK.
        let id = Uuid::now_v7();
        bytes = id.to_bytes_le();
    }
    let mut out = String::with_capacity(32);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getShares", get_shares))
        .merge(super::endpoint_router("createShare", create_share))
        .merge(super::endpoint_router("updateShare", update_share))
        .merge(super::endpoint_router("deleteShare", delete_share))
}
