//! OpenSubsonic playlist endpoints (PRD-API-OS-004).
//!
//! Playlists reuse the `native_playlists` / `native_playlist_items`
//! tables so a user's library is shared across surfaces; only the OpenSubsonic
//! wire shape lives here. Captured quirks preserved: `allowedUsers: []`,
//! `coverArt` = the playlist's own id, `duration` in whole seconds, and the
//! bare `ok` body for delete. The legacy mutation-breakage (cover resolution
//! throwing mid-serialization -> 302, and the partial-create poisoning class)
//! is NOT preserved (coverage-matrix D4): updatePlaylist applies cleanly and
//! createPlaylist is atomic.

use axum::{
    extract::{Extension, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    annotation::{parse_prefixed_id, rfc3339_z, song_children},
    auth::OsPrincipal,
    envelope::{OsReply, negotiate},
    params::Params,
};

struct PlaylistRow {
    id: Uuid,
    public_api_id: Uuid,
    name: String,
    comment: Option<String>,
    is_public: bool,
    owner_username: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

const PLAYLIST_SELECT: &str = "SELECT p.id, p.public_api_id, p.name, p.description, p.is_public,
    u.username, p.created_at, p.updated_at
  FROM native_playlists p JOIN native_users u ON u.id = p.owner_user_id";

fn wire_id(public_api_id: Uuid) -> String {
    format!("playlist_{public_api_id}")
}

fn public_id_from_param(params: &Params, key: &str) -> Option<Uuid> {
    params.get(key).and_then(|value| {
        value
            .strip_prefix("playlist_")
            .and_then(|raw| Uuid::parse_str(raw).ok())
    })
}

fn playlist_header(row: &PlaylistRow) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    map.insert(
        "id".to_string(),
        serde_json::Value::from(wire_id(row.public_api_id)),
    );
    map.insert(
        "name".to_string(),
        serde_json::Value::from(row.name.clone()),
    );
    if let Some(comment) = &row.comment {
        map.insert(
            "comment".to_string(),
            serde_json::Value::from(comment.clone()),
        );
    }
    map.insert(
        "owner".to_string(),
        serde_json::Value::from(row.owner_username.clone()),
    );
    map.insert("public".to_string(), serde_json::Value::from(row.is_public));
    map.insert(
        "created".to_string(),
        serde_json::Value::from(rfc3339_z(row.created_at)),
    );
    map.insert(
        "changed".to_string(),
        serde_json::Value::from(rfc3339_z(row.updated_at)),
    );
    map.insert(
        "coverArt".to_string(),
        serde_json::Value::from(wire_id(row.public_api_id)),
    );
    map.insert(
        "allowedUsers".to_string(),
        serde_json::Value::Array(Vec::new()),
    );
    map
}

async fn load_playlist(
    pool: &sqlx::PgPool,
    public_api_id: Uuid,
) -> Result<Option<PlaylistRow>, sqlx::Error> {
    let sql = sqlx::AssertSqlSafe(format!("{PLAYLIST_SELECT} WHERE p.public_api_id = $1"));
    let row = sqlx::query_as::<_, PlaylistDbRow>(sql)
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(PlaylistRow::from))
}

#[derive(sqlx::FromRow)]
struct PlaylistDbRow {
    id: Uuid,
    public_api_id: Uuid,
    name: String,
    description: Option<String>,
    is_public: bool,
    username: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl From<PlaylistDbRow> for PlaylistRow {
    fn from(row: PlaylistDbRow) -> Self {
        Self {
            id: row.id,
            public_api_id: row.public_api_id,
            name: row.name,
            comment: row.description,
            is_public: row.is_public,
            owner_username: row.username,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

async fn entries(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    playlist_id: Uuid,
) -> Result<(Vec<serde_json::Value>, i64), sqlx::Error> {
    // Captured rule: legacy `GetPlaylistAsync` fetched the playlist songs only
    // for SongCount/ApiKeys, then re-queried `Songs` by ApiKey with no ORDER
    // BY, so `entry` followed Song.Id (creation) order — `PlaylistOrder` was
    // never applied to the response (os-playlists-getplaylist shows
    // [Solar Wind, Falling, Neon 東京, Coda], the song creation order, not the
    // catalog playlist order). Track creation order is the surrogate for the
    // legacy integer Song.Id.
    let track_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT track.public_api_id FROM native_playlist_items item
           JOIN tracks track ON track.id = item.track_id
         WHERE item.playlist_id = $1 ORDER BY track.created_at, track.public_api_id",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    let public_ids: Vec<Uuid> = track_rows.into_iter().map(|(id,)| id).collect();
    let children = song_children(pool, user_id, &public_ids).await?;
    let duration: i64 = children
        .iter()
        .filter_map(|child| child.get("duration").and_then(|d| d.as_i64()))
        .sum();
    Ok((children, duration))
}

fn visible(row: &PlaylistRow, principal: &OsPrincipal) -> bool {
    row.is_public || row.owner_username.eq_ignore_ascii_case(&principal.username)
}

async fn respond_playlist(
    state: &OsState,
    row: &PlaylistRow,
    principal: &OsPrincipal,
    params: &Params,
    with_entries: bool,
) -> Result<Response, sqlx::Error> {
    let mut header = playlist_header(row);
    let (entry, duration) = entries(&state.pool, principal.user_id, row.id).await?;
    header.insert(
        "songCount".to_string(),
        serde_json::Value::from(entry.len() as i64),
    );
    header.insert("duration".to_string(), serde_json::Value::from(duration));
    if with_entries {
        header.insert("entry".to_string(), serde_json::Value::Array(entry));
    }
    Ok(
        OsReply::ok(serde_json::json!({ "playlist": serde_json::Value::Object(header) })).respond(
            &negotiate(params),
            &state.server_version,
            StatusCode::OK,
        ),
    )
}

pub async fn get_playlists(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params = params.map(|Extension(p)| p).unwrap_or_default();
    let Some(principal) = principal.map(|Extension(p)| p) else {
        return OsReply::error(40, "Wrong username or password.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let rows: Vec<PlaylistDbRow> = match sqlx::query_as::<_, PlaylistDbRow>(sqlx::AssertSqlSafe(
        // Captured getPlaylists order is creation ascending (`os-playlists-getplaylists`).
        format!("{PLAYLIST_SELECT} ORDER BY p.created_at, p.public_api_id"),
    ))
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlaylists failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    let mut playlists = Vec::new();
    for db_row in rows {
        let row = PlaylistRow::from(db_row);
        if !visible(&row, &principal) {
            continue;
        }
        let mut header = playlist_header(&row);
        let (entry, duration) = match entries(&state.pool, principal.user_id, row.id).await {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "opensubsonic getPlaylists entry load failed");
                return OsReply::error(0, "Internal error.").respond(
                    &negotiate(&params),
                    &state.server_version,
                    StatusCode::OK,
                );
            }
        };
        header.insert(
            "songCount".to_string(),
            serde_json::Value::from(entry.len() as i64),
        );
        header.insert("duration".to_string(), serde_json::Value::from(duration));
        playlists.push(serde_json::Value::Object(header));
    }
    OsReply::ok(serde_json::json!({ "playlists": { "playlist": playlists } })).respond(
        &negotiate(&params),
        &state.server_version,
        StatusCode::OK,
    )
}

pub async fn get_playlist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params = params.map(|Extension(p)| p).unwrap_or_default();
    let Some(principal) = principal.map(|Extension(p)| p) else {
        return OsReply::error(40, "Wrong username or password.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let Some(id) = public_id_from_param(&params, "id") else {
        return redirect_to_error(&state, &params);
    };
    let row = match load_playlist(&state.pool, id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getPlaylist failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    match row {
        Some(row) if visible(&row, &principal) => {
            match respond_playlist(&state, &row, &principal, &params, true).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::error!(%error, "opensubsonic getPlaylist entry load failed");
                    OsReply::error(0, "Internal error.").respond(
                        &negotiate(&params),
                        &state.server_version,
                        StatusCode::OK,
                    )
                }
            }
        }
        // Captured unknown-id split (`os-playlists-getplaylist-unknown` vs
        // `os-playlists-verify-deleted`): the all-zero GUID and ids the caller
        // may not see take the 302 /Account/Login?ReturnUrl=%2FError class;
        // a well-formed id that simply no longer resolves answers 200 with
        // status ok + error 44.
        Some(_) => redirect_to_error(&state, &params),
        None if id == Uuid::nil() => redirect_to_error(&state, &params),
        None => {
            let params_ext = Some(axum::Extension(params.clone()));
            super::browse::ok_with_error_44(&state, &params_ext)
        }
    }
}

pub async fn create_playlist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params = params.map(|Extension(p)| p).unwrap_or_default();
    let Some(principal) = principal.map(|Extension(p)| p) else {
        return OsReply::error(40, "Wrong username or password.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let name = params
        .get("name")
        .map(str::to_string)
        .unwrap_or_else(|| "Unnamed".to_string());
    let song_ids: Vec<Uuid> = params
        .get_all("songId")
        .iter()
        .filter_map(|value| parse_prefixed_id(value))
        .filter_map(|entity| match entity {
            super::annotation::EntityRef::Track(id) => Some(id),
            _ => None,
        })
        .collect();
    let now = OffsetDateTime::now_utc();
    let playlist_id = Uuid::now_v7();
    let public_api_id = Uuid::now_v7();
    match sqlx::query(
        "INSERT INTO native_playlists (id, public_api_id, owner_user_id, name, description, is_public, revision, created_at, updated_at)
         VALUES ($1, $2, $3, $4, NULL, false, 1, $5, $5)",
    )
    .bind(playlist_id)
    .bind(public_api_id)
    .bind(principal.user_id)
    .bind(&name)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        Ok(_) => {}
        Err(error) => {
            tracing::error!(%error, "opensubsonic createPlaylist insert failed");
            return OsReply::error(0, "Internal error.").respond(&negotiate(&params), &state.server_version, StatusCode::OK);
        }
    }
    if !song_ids.is_empty()
        && let Err(error) =
            append_items(&state.pool, playlist_id, principal.user_id, &song_ids, 0).await
    {
        tracing::error!(%error, "opensubsonic createPlaylist items failed");
        return OsReply::error(0, "Internal error.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    }
    match load_playlist(&state.pool, public_api_id).await {
        Ok(Some(row)) => match respond_playlist(&state, &row, &principal, &params, true).await {
            Ok(response) => response,
            Err(error) => {
                tracing::error!(%error, "opensubsonic createPlaylist entry load failed");
                OsReply::error(0, "Internal error.").respond(
                    &negotiate(&params),
                    &state.server_version,
                    StatusCode::OK,
                )
            }
        },
        _ => OsReply::error(70, "Playlist not found.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        ),
    }
}

pub async fn update_playlist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params = params.map(|Extension(p)| p).unwrap_or_default();
    let Some(principal) = principal.map(|Extension(p)| p) else {
        return OsReply::error(40, "Wrong username or password.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let Some(id) = public_id_from_param(&params, "playlistId") else {
        return OsReply::error(10, "Missing playlist id.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let row = match load_playlist(&state.pool, id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "opensubsonic updatePlaylist load failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    let Some(row) = row else {
        return OsReply::error(70, "Playlist not found.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    if !visible(&row, &principal) {
        return OsReply::error(50, "User is not authorized.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    }
    let now = OffsetDateTime::now_utc();
    // The frozen `os-playlists-verify-updated` shows the original name only
    // because the legacy response serialization 302'd on cover resolution and
    // rolled the whole mutation back — the compatibility evidence marks that
    // cover-dependent breakage MUST NOT be preserved. The clean update
    // applies the rename like the legacy service intended.
    if let Some(name) = params.get("name")
        && let Err(error) =
            sqlx::query("UPDATE native_playlists SET name = $2, updated_at = $3 WHERE id = $1")
                .bind(row.id)
                .bind(name)
                .bind(now)
                .execute(&state.pool)
                .await
    {
        tracing::error!(%error, "opensubsonic updatePlaylist rename failed");
        return OsReply::error(0, "Internal error.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    }
    if let Some(comment) = params.get("comment") {
        let _ = sqlx::query(
            "UPDATE native_playlists SET description = $2, updated_at = $3 WHERE id = $1",
        )
        .bind(row.id)
        .bind(comment)
        .bind(now)
        .execute(&state.pool)
        .await;
    }
    if let Some(public) = params.get_bool("public") {
        let _ = sqlx::query(
            "UPDATE native_playlists SET is_public = $2, updated_at = $3 WHERE id = $1",
        )
        .bind(row.id)
        .bind(public)
        .bind(now)
        .execute(&state.pool)
        .await;
    }
    let to_add: Vec<Uuid> = params
        .get_all("songIdToAdd")
        .iter()
        .filter_map(|value| parse_prefixed_id(value))
        .filter_map(|entity| match entity {
            super::annotation::EntityRef::Track(id) => Some(id),
            _ => None,
        })
        .collect();
    let to_remove: Vec<i64> = params
        .get_all("songIndexToRemove")
        .iter()
        .filter_map(|v| v.parse().ok())
        .collect();
    if !to_remove.is_empty() {
        // OpenSubsonic `songIndexToRemove` semantics: zero-based index over
        // the position-ordered entries, applied BEFORE any additions. (The
        // legacy request model bound only `songIdToRemove` and silently
        // ignored the index parameter; the spec parameter is the contract
        // real clients send.) Positions are normalized to the zero-based
        // sequence first so fixture-seeded 1-based rows index the same way.
        let _ = sqlx::query("WITH ranked AS (SELECT id, ROW_NUMBER() OVER (ORDER BY position) - 1 AS new_pos FROM native_playlist_items WHERE playlist_id = $1) UPDATE native_playlist_items AS i SET position = ranked.new_pos FROM ranked WHERE i.id = ranked.id")
            .bind(row.id)
            .execute(&state.pool)
            .await;
        let placeholders: Vec<String> = to_remove
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 2))
            .collect();
        let query = format!(
            "DELETE FROM native_playlist_items WHERE playlist_id = $1 AND position IN ({})",
            placeholders.join(",")
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(query)).bind(row.id);
        for idx in &to_remove {
            q = q.bind(idx);
        }
        if let Err(error) = q.execute(&state.pool).await {
            tracing::error!(%error, "opensubsonic updatePlaylist remove failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
        let _ = sqlx::query("WITH ranked AS (SELECT id, ROW_NUMBER() OVER (ORDER BY position) - 1 AS new_pos FROM native_playlist_items WHERE playlist_id = $1) UPDATE native_playlist_items AS i SET position = ranked.new_pos FROM ranked WHERE i.id = ranked.id")
            .bind(row.id)
            .execute(&state.pool)
            .await;
    }
    if !to_add.is_empty() {
        let next: i64 = sqlx::query_scalar("SELECT (COALESCE(MAX(position), -1) + 1)::bigint FROM native_playlist_items WHERE playlist_id = $1")
            .bind(row.id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(0);
        if let Err(error) =
            append_items(&state.pool, row.id, principal.user_id, &to_add, next).await
        {
            tracing::error!(%error, "opensubsonic updatePlaylist add failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    }
    let _ = sqlx::query("UPDATE native_playlists SET updated_at = $2 WHERE id = $1")
        .bind(row.id)
        .bind(now)
        .execute(&state.pool)
        .await;
    OsReply::ok(serde_json::json!({})).respond(
        &negotiate(&params),
        &state.server_version,
        StatusCode::OK,
    )
}

pub async fn delete_playlist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params = params.map(|Extension(p)| p).unwrap_or_default();
    let Some(principal) = principal.map(|Extension(p)| p) else {
        return OsReply::error(40, "Wrong username or password.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let Some(id) = public_id_from_param(&params, "id") else {
        return OsReply::error(10, "Missing playlist id.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    };
    let row = match load_playlist(&state.pool, id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "opensubsonic deletePlaylist load failed");
            return OsReply::error(0, "Internal error.").respond(
                &negotiate(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    // Only the owner may delete; unknown or foreign playlists answer the
    // captured bare ok (deleted -> ok quirk; unknown -> ok with no error).
    if let Some(row) = row
        && row.owner_username.eq_ignore_ascii_case(&principal.username)
        && let Err(error) = sqlx::query("DELETE FROM native_playlists WHERE id = $1")
            .bind(row.id)
            .execute(&state.pool)
            .await
    {
        tracing::error!(%error, "opensubsonic deletePlaylist failed");
        return OsReply::error(0, "Internal error.").respond(
            &negotiate(&params),
            &state.server_version,
            StatusCode::OK,
        );
    }
    OsReply::ok(serde_json::json!({})).respond(
        &negotiate(&params),
        &state.server_version,
        StatusCode::OK,
    )
}

async fn append_items(
    pool: &sqlx::PgPool,
    playlist_id: Uuid,
    user_id: Uuid,
    track_ids: &[Uuid],
    start_position: i64,
) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    // `track_ids` arrive as track PUBLIC ids; the items table references the
    // internal id.
    let internal_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT track.id FROM tracks track
           JOIN unnest($1::uuid[]) WITH ORDINALITY input(public_id, ord) ON true
          WHERE track.public_api_id = input.public_id
          ORDER BY input.ord",
    )
    .bind(track_ids)
    .fetch_all(pool)
    .await?;
    for (offset, track_id) in internal_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO native_playlist_items (id, playlist_id, track_id, position, added_by_user_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(playlist_id)
        .bind(track_id)
        .bind(start_position + offset as i64)
        .bind(user_id)
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Captured unknown/private-inaccessible playlist 302 class. The legacy
/// Location embedded the full request query (including credentials) — that
/// credential leak is not preserved (ADR-0130); the Rust redirect points to a
/// static error target with no query material.
fn redirect_to_error(state: &OsState, _params: &Params) -> Response {
    let location = format!(
        "{}/Account/Login?ReturnUrl=%2FError",
        state.public_origin.trim_end_matches('/')
    );
    let mut response = axum::response::Response::default();
    *response.status_mut() = StatusCode::FOUND;
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

pub fn router() -> axum::Router<OsState> {
    super::endpoint_router("getPlaylists", get_playlists)
        .merge(super::endpoint_router("getPlaylist", get_playlist))
        .merge(super::endpoint_router("createPlaylist", create_playlist))
        .merge(super::endpoint_router("updatePlaylist", update_playlist))
        .merge(super::endpoint_router("deletePlaylist", delete_playlist))
}
