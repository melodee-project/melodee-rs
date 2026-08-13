//! Lists/search endpoint group: getAlbumList, getAlbumList2, getRandomSongs,
//! getNowPlaying, getStarred, getStarred2, getSongsByGenre, search2, search3,
//! and the melodeeExtensions[1] search endpoints
//! (PRD-API-OS-005; golden evidence `compatibility/opensubsonic/golden/lists`,
//! capture semantics `design/evidence/phase-0/opensubsonic-capture.md` §8).
//!
//! Captured behaviors reproduced here:
//! - Album list types newest/alphabeticalByName/alphabeticalByArtist/byGenre/
//!   byYear/starred/frequent/recent/random/highest. `recent` sorts by release
//!   date, not last played (captured quirk). `x-total-count` carries the
//!   filtered, unpaged total.
//! - Legacy `albumList` entries are `albumList2` entries plus a leading
//!   `averageRating` decimal; `playedCount` there vs `playCount` on Child.
//! - search2/search3 share the search-result DTOs, including the hardcoded
//!   placeholder song fields (bitRate 0, suffix "mp3", path "unknown",
//!   contentType "audio/mpeg") and the offset-rounds-down-to-page quirk.
//! - The Dune/Düné collision: matching runs on the legacy normalized form
//!   (accent-stripped, uppercased, non-alphanumerics removed), so one query
//!   matches both identities. A query that folds to nothing (e.g. CJK-only)
//!   matches nothing, mirroring the legacy base64 fallback that never hits.
//! - The extension endpoints 302 to `/Account/Login?ReturnUrl=%2FError` in
//!   the captured fixture environment (external provider dependency). The
//!   captured Location embedded the request query including credentials;
//!   that leak is deliberately not preserved.
//!
//! Deterministic tiebreaks (ADR-0130 decision 5): every sorted list appends
//! the entity public id as the final ORDER BY key.

use std::collections::HashMap;

use axum::{
    Router,
    extract::{Extension, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value};
use sqlx::{AssertSqlSafe, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    annotation::{rfc3339_z, song_children},
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

const DEFAULT_LIST_SIZE: i64 = 10;
/// Legacy bounded list outputs by OpenSubsonicIndexesArtistLimit (a short);
/// the exact configured value is environment state, so a fixed bound keeps
/// the queries set-based and bounded without inventing a captured number.
const MAX_LIST_SIZE: i64 = 500;
const DEFAULT_SEARCH_PAGE: i64 = 20;
const MAX_SEARCH_PAGE: i64 = 500;

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(state: &OsState, params: &Option<Extension<Params>>, reply: OsReply) -> Response {
    reply.respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn error_reply(state: &OsState, params: &Option<Extension<Params>>, message: &str) -> Response {
    reply(state, params, OsReply::error(0, message))
}

/// Resolves the user context for list endpoints. Authenticated requests use
/// the principal. Under the captured bypass the legacy service still resolved
/// the `u` username (the `!RequiresAuthentication` branch of
/// AuthenticateSubsonicApiAsync), which is how the bypass-mode goldens carry
/// user-specific ratings and stars; without a resolvable username the legacy
/// BlankUserInfo applies and user-specific lists are empty.
async fn resolve_user_id(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Option<Uuid> {
    if let Some(Extension(principal)) = principal {
        return Some(principal.user_id);
    }
    let Extension(params) = params.as_ref()?;
    let username = params.get("u")?;
    let normalized = username.trim().to_uppercase();
    if normalized.is_empty() {
        return None;
    }
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM native_users WHERE normalized_username = $1")
        .bind(normalized)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
}

/// Wire id of the library rendered as album `parent`. The legacy value is the
/// owning artist's Library api key; the Rust catalog does not yet key releases
/// to libraries, so the first configured music folder (the ingestion source,
/// which the fixture library is) stands in, matching the captured fixture.
fn library_parent(state: &OsState) -> String {
    state
        .music_folders
        .first()
        .map_or_else(String::new, |folder| folder.wire_id())
}

// ---------------------------------------------------------------------------
// Album lists
// ---------------------------------------------------------------------------

/// Captured `type` parameter values (legacy `ListType` enum; ASP.NET binding
/// is case-insensitive and accepts numeric forms, unknown values bind to the
/// `NotSet` default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListType {
    NotSet,
    Random,
    Newest,
    Highest,
    Frequent,
    Recent,
    AlphabeticalByName,
    AlphabeticalByArtist,
    Starred,
    ByYear,
    ByGenre,
}

fn parse_list_type(value: Option<&str>) -> ListType {
    let Some(value) = value else {
        return ListType::NotSet;
    };
    match value.to_ascii_lowercase().as_str() {
        "random" | "1" => ListType::Random,
        "newest" | "2" => ListType::Newest,
        "highest" | "3" => ListType::Highest,
        "frequent" | "4" => ListType::Frequent,
        "recent" | "5" => ListType::Recent,
        "alphabeticalbyname" | "6" => ListType::AlphabeticalByName,
        "alphabeticalbyartist" | "7" => ListType::AlphabeticalByArtist,
        "starred" | "8" => ListType::Starred,
        "byyear" | "9" => ListType::ByYear,
        "bygenre" | "10" => ListType::ByGenre,
        _ => ListType::NotSet,
    }
}

/// Shared FROM/WHERE for both album list endpoints. Binds: $1 user id
/// (nullable), $2 genre (nullable), $3 fromYear, $4 toYear, $5 starred-only.
const ALBUM_LIST_FROM: &str = "FROM releases release
  JOIN LATERAL (
    SELECT artist.public_api_id, artist.canonical_name
    FROM release_artists billing
    JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.release_id = release.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) artist ON true
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
  LEFT JOIN native_release_user_states ustate
    ON ustate.release_id = release.id AND ustate.user_id = $1
  WHERE release.status = 'active'
    AND ($2::text IS NULL OR EXISTS (
      SELECT 1 FROM catalog_imports catalog_import
      JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
      CROSS JOIN LATERAL jsonb_array_elements(
        COALESCE(manifest.manifest_payload->'files', '[]')) file(value)
      CROSS JOIN LATERAL unnest(string_to_array(
        file.value->'metadata'->>'genre', ';')) genre_part(value)
      WHERE catalog_import.release_id = release.id
        AND btrim(genre_part.value) = $2))
    AND ($3::integer IS NULL OR extract(year FROM release.release_date) >= $3)
    AND ($4::integer IS NULL OR extract(year FROM release.release_date) <= $4)
    AND (NOT $5 OR COALESCE(ustate.starred, false))";

const ALBUM_LIST_SELECT: &str = "SELECT release.public_api_id AS release_id,
    release.title, release.created_at, release.release_date,
    COALESCE(aggregate.track_count, 0) AS song_count,
    COALESCE(aggregate.duration_ms, 0) AS duration_ms,
    artist.public_api_id AS artist_id, artist.canonical_name AS artist_name,
    extract(year FROM release.release_date)::integer AS year,
    COALESCE(ustate.rating, 0) AS user_rating,
    (SELECT count(*) FROM native_track_plays play
      JOIN tracks play_track ON play_track.id = play.track_id
      WHERE play_track.release_id = release.id) AS played_count,
    (SELECT COALESCE(AVG(ratings.rating), 0)::float8 FROM native_release_user_states ratings
      WHERE ratings.release_id = release.id AND ratings.rating > 0) AS average_rating
";

struct AlbumListRow {
    release_id: Uuid,
    title: String,
    created_at: OffsetDateTime,
    song_count: i64,
    duration_ms: i64,
    artist_id: Uuid,
    artist_name: String,
    year: Option<i32>,
    user_rating: i32,
    played_count: i64,
    average_rating: f64,
}

impl AlbumListRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            release_id: row.try_get("release_id")?,
            title: row.try_get("title")?,
            created_at: row.try_get("created_at")?,
            song_count: row.try_get("song_count")?,
            duration_ms: row.try_get("duration_ms")?,
            artist_id: row.try_get("artist_id")?,
            artist_name: row.try_get("artist_name")?,
            year: row.try_get("year")?,
            user_rating: row.try_get("user_rating")?,
            played_count: row.try_get("played_count")?,
            average_rating: row.try_get("average_rating")?,
        })
    }
}

fn album_list_entry(
    parent: &str,
    row: &AlbumListRow,
    genres: &[String],
    legacy_shape: bool,
) -> Value {
    let album_id = format!("album_{}", row.release_id);
    let mut album = Map::new();
    if legacy_shape {
        // The legacy AlbumList DTO prepends the decimal averageRating.
        album.insert("averageRating".to_string(), Value::from(row.average_rating));
    }
    album.insert("id".to_string(), album_id.clone().into());
    album.insert("album".to_string(), row.title.clone().into());
    album.insert("title".to_string(), row.title.clone().into());
    album.insert("name".to_string(), row.title.clone().into());
    album.insert("coverArt".to_string(), album_id.into());
    album.insert("songCount".to_string(), row.song_count.into());
    album.insert("created".to_string(), rfc3339_z(row.created_at).into());
    album.insert("duration".to_string(), (row.duration_ms / 1000).into());
    album.insert("playedCount".to_string(), row.played_count.into());
    album.insert(
        "artistId".to_string(),
        format!("artist_{}", row.artist_id).into(),
    );
    album.insert("artist".to_string(), row.artist_name.clone().into());
    // AlbumList2.Year is a non-nullable int in the legacy DTO.
    album.insert("year".to_string(), i64::from(row.year.unwrap_or(0)).into());
    album.insert("userRating".to_string(), i64::from(row.user_rating).into());
    if !genres.is_empty() {
        album.insert(
            "genres".to_string(),
            Value::Array(genres.iter().map(|genre| genre.clone().into()).collect()),
        );
        album.insert("genre".to_string(), genres[0].clone().into());
    }
    album.insert("parent".to_string(), parent.into());
    Value::Object(album)
}

/// Release-level genre display values resolve through the catalog import
/// manifest files (the same join the native adapter uses); multi-value genre
/// strings split on `;`, distinct in first-seen order.
async fn release_genres(
    pool: &PgPool,
    release_public_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let mut genres: HashMap<Uuid, Vec<String>> = HashMap::new();
    if release_public_ids.is_empty() {
        return Ok(genres);
    }
    let rows = sqlx::query(
        "SELECT release.public_api_id, file.value->'metadata'->>'genre' AS genre
         FROM releases release
         JOIN catalog_imports catalog_import ON catalog_import.release_id = release.id
         JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
         CROSS JOIN LATERAL jsonb_array_elements(
           COALESCE(manifest.manifest_payload->'files', '[]')) file(value)
         WHERE release.public_api_id = ANY($1)
         ORDER BY release.public_api_id, catalog_import.imported_at DESC",
    )
    .bind(release_public_ids)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let release_id: Uuid = row.try_get(0)?;
        let genre: Option<String> = row.try_get(1)?;
        let entry = genres.entry(release_id).or_default();
        if let Some(genre) = genre {
            for part in genre
                .split(';')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                if !entry.iter().any(|known| known == part) {
                    entry.push(part.to_string());
                }
            }
        }
    }
    Ok(genres)
}

async fn album_list(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    id3_shape: bool,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let list_type = parse_list_type(params_ref.and_then(|p| p.get("type")));
    let size = params_ref
        .and_then(|p| p.get_i64("size"))
        .unwrap_or(DEFAULT_LIST_SIZE)
        .clamp(0, MAX_LIST_SIZE);
    let offset = params_ref
        .and_then(|p| p.get_i64("offset"))
        .unwrap_or(0)
        .max(0);
    let genre = params_ref.and_then(|p| p.get("genre")).map(str::to_string);
    let from_year = params_ref
        .and_then(|p| p.get_i64("fromyear"))
        .and_then(|year| i32::try_from(year).ok());
    let to_year = params_ref
        .and_then(|p| p.get_i64("toyear"))
        .and_then(|year| i32::try_from(year).ok());
    let starred_only = list_type == ListType::Starred;
    let user_id = resolve_user_id(state, params, principal).await;

    let order = match list_type {
        ListType::Random => "random()",
        ListType::Newest => "created_at DESC, release_id",
        ListType::Highest => "average_rating DESC, release_id",
        ListType::Frequent => "played_count DESC, release_id",
        // Captured quirk: `recent` sorts by release date, not last played.
        ListType::Recent => "release_date DESC NULLS LAST, release_id",
        ListType::AlphabeticalByName => "title COLLATE \"C\", release_id",
        ListType::AlphabeticalByArtist => "artist_name COLLATE \"C\", release_id",
        // Starred/ByYear/ByGenre/NotSet carried no legacy ORDER BY; the public
        // id keeps them deterministic (ADR-0130 decision 5).
        ListType::Starred | ListType::ByYear | ListType::ByGenre | ListType::NotSet => "release_id",
    };
    // The ORDER BY is a module-owned constant selected above; every request
    // value stays a bind parameter.
    let query = format!(
        "WITH list AS ({ALBUM_LIST_SELECT} {ALBUM_LIST_FROM})
         SELECT * FROM list ORDER BY {order} LIMIT $6 OFFSET $7"
    );
    let count_query = format!("SELECT count(*) {ALBUM_LIST_FROM}");
    let rows = sqlx::query(AssertSqlSafe(query))
        .bind(user_id)
        .bind(genre.as_deref())
        .bind(from_year)
        .bind(to_year)
        .bind(starred_only)
        .bind(size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await;
    let total = sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_query))
        .bind(user_id)
        .bind(genre.as_deref())
        .bind(from_year)
        .bind(to_year)
        .bind(starred_only)
        .fetch_one(&state.pool)
        .await;
    let (rows, total) = match (rows, total) {
        (Ok(rows), Ok(total)) => (rows, total),
        (Err(error), _) | (_, Err(error)) => {
            tracing::error!(%error, "opensubsonic album list query failed");
            return error_reply(
                state,
                params,
                if id3_shape {
                    "Failed to get AlbumList2"
                } else {
                    "Failed to get AlbumList"
                },
            );
        }
    };
    let albums: Vec<AlbumListRow> = match rows
        .iter()
        .map(AlbumListRow::from_row)
        .collect::<Result<_, _>>()
    {
        Ok(albums) => albums,
        Err(error) => {
            tracing::error!(%error, "opensubsonic album list row mapping failed");
            return error_reply(
                state,
                params,
                if id3_shape {
                    "Failed to get AlbumList2"
                } else {
                    "Failed to get AlbumList"
                },
            );
        }
    };
    let release_ids: Vec<Uuid> = albums.iter().map(|album| album.release_id).collect();
    let genres_by_release = match release_genres(&state.pool, &release_ids).await {
        Ok(genres) => genres,
        Err(error) => {
            tracing::error!(%error, "opensubsonic album list genre query failed");
            HashMap::new()
        }
    };
    let parent = library_parent(state);
    let entries: Vec<Value> = albums
        .iter()
        .map(|album| {
            album_list_entry(
                &parent,
                album,
                genres_by_release
                    .get(&album.release_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                !id3_shape,
            )
        })
        .collect();
    let key = if id3_shape { "albumList2" } else { "albumList" };
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({key: {"album": entries}})).with_total_count(total),
    )
}

pub async fn get_album_list(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    album_list(&state, &params, &principal, false).await
}

pub async fn get_album_list2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    album_list(&state, &params, &principal, true).await
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Song lists
// ---------------------------------------------------------------------------

/// Genre membership for one Track: the captured predicate is an exact,
/// case-sensitive `Contains` over the song's genre tags or its album's (the
/// album set subsumes the track set, so one release-level EXISTS suffices).
/// Bound as $1 with `release` and `track` in scope.
const TRACK_GENRE_MATCH: &str = "EXISTS (
  SELECT 1 FROM catalog_imports catalog_import
  JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
  CROSS JOIN LATERAL jsonb_array_elements(
    COALESCE(manifest.manifest_payload->'files', '[]')) file(value)
  CROSS JOIN LATERAL unnest(string_to_array(
    file.value->'metadata'->>'genre', ';')) genre_part(value)
  WHERE catalog_import.release_id = release.id
    AND btrim(genre_part.value) = $1)";

pub async fn get_songs_by_genre(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(genre) = params_ref.and_then(|p| p.get("genre")) else {
        // Legacy binds a required `genre`; a missing one fails model binding
        // before the service runs. There is no capture; fail closed empty.
        return reply(
            &state,
            &params,
            OsReply::ok(serde_json::json!({"songsByGenre": {"song": []}})),
        );
    };
    let count = params_ref
        .and_then(|p| p.get_i64("count"))
        .unwrap_or(MAX_LIST_SIZE)
        .clamp(0, MAX_LIST_SIZE);
    let offset = params_ref
        .and_then(|p| p.get_i64("offset"))
        .unwrap_or(0)
        .max(0);
    let user_id = resolve_user_id(&state, &params, &principal).await;

    let from_where = format!(
        "FROM tracks track
         JOIN releases release ON release.id = track.release_id AND release.status = 'active'
         WHERE {TRACK_GENRE_MATCH}"
    );
    let ids = sqlx::query_scalar::<_, Uuid>(AssertSqlSafe(format!(
        "SELECT track.public_api_id {from_where}
         ORDER BY track.release_id, track.ordinal, track.public_api_id LIMIT $2 OFFSET $3"
    )))
    .bind(genre)
    .bind(count)
    .bind(offset)
    .fetch_all(&state.pool)
    .await;
    let total =
        sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!("SELECT count(*) {from_where}")))
            .bind(genre)
            .fetch_one(&state.pool)
            .await;
    let (ids, total) = match (ids, total) {
        (Ok(ids), Ok(total)) => (ids, total),
        (Err(error), _) | (_, Err(error)) => {
            tracing::error!(%error, "opensubsonic songs-by-genre query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let songs = match song_children(&state.pool, user_id.unwrap_or(Uuid::nil()), &ids).await {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic songs-by-genre projection failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"songsByGenre": {"song": songs}})).with_total_count(total),
    )
}

pub async fn get_random_songs(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    // Legacy signature is `int size`: a missing/invalid value binds to 0 and
    // yields an empty list.
    let size = params_ref
        .and_then(|p| p.get_i64("size"))
        .unwrap_or(0)
        .clamp(0, MAX_LIST_SIZE);
    let genre = params_ref.and_then(|p| p.get("genre")).map(str::to_string);
    let from_year = params_ref
        .and_then(|p| p.get_i64("fromyear"))
        .and_then(|year| i32::try_from(year).ok())
        .unwrap_or(0);
    let to_year = params_ref
        .and_then(|p| p.get_i64("toyear"))
        .and_then(|year| i32::try_from(year).ok())
        .unwrap_or(9999);
    let user_id = resolve_user_id(&state, &params, &principal).await;

    let query = format!(
        "SELECT track.public_api_id
         FROM tracks track
         JOIN releases release ON release.id = track.release_id AND release.status = 'active'
         WHERE ($1::text IS NULL OR {TRACK_GENRE_MATCH})
           AND extract(year FROM release.release_date) >= $2
           AND extract(year FROM release.release_date) <= $3
         ORDER BY random() LIMIT $4"
    );
    let ids = match sqlx::query_scalar::<_, Uuid>(AssertSqlSafe(query))
        .bind(genre.as_deref())
        .bind(from_year)
        .bind(to_year)
        .bind(size)
        .fetch_all(&state.pool)
        .await
    {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "opensubsonic random-songs query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let songs = match song_children(&state.pool, user_id.unwrap_or(Uuid::nil()), &ids).await {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic random-songs projection failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"randomSongs": {"song": songs}})),
    )
}

pub async fn get_now_playing(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user_id = resolve_user_id(&state, &params, &principal).await;
    let rows = sqlx::query(
        "SELECT track.public_api_id, now_playing.started_at, account.username
         FROM native_now_playing now_playing
         JOIN tracks track ON track.id = now_playing.track_id
         JOIN native_users account ON account.id = now_playing.user_id
         ORDER BY now_playing.updated_at DESC, track.public_api_id
         LIMIT $1",
    )
    .bind(MAX_LIST_SIZE)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic now-playing query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let ids: Vec<Uuid> = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>(0))
        .collect::<Result<_, _>>()
        .unwrap_or_default();
    let mut entries = match song_children(&state.pool, user_id.unwrap_or(Uuid::nil()), &ids).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(%error, "opensubsonic now-playing projection failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    // Rows and children are in the same order; now-playing adds the playing
    // user and age fields the legacy Child DTO carries.
    let now = OffsetDateTime::now_utc();
    for (row, entry) in rows.iter().zip(entries.iter_mut()) {
        let Ok(started_at) = row.try_get::<OffsetDateTime, _>(1) else {
            continue;
        };
        let Ok(username) = row.try_get::<String, _>(2) else {
            continue;
        };
        if let Value::Object(fields) = entry {
            fields.insert("username".to_string(), username.into());
            fields.insert(
                "minutesAgo".to_string(),
                (now - started_at).whole_minutes().max(0).into(),
            );
        }
    }
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"nowPlaying": {"entry": entries}})),
    )
}

// ---------------------------------------------------------------------------
// Starred
// ---------------------------------------------------------------------------

struct StarredAlbumRow {
    release_id: Uuid,
    title: String,
    sort_title: String,
    created_at: OffsetDateTime,
    year: Option<i32>,
    song_count: i64,
    duration_ms: i64,
    artist_id: Uuid,
    artist_name: String,
    artist_sort_name: String,
    artist_album_count: i64,
    user_rating: i16,
    starred_at: OffsetDateTime,
    played_count: i64,
    last_played_at: Option<OffsetDateTime>,
}

/// Starred Release rows for one user, oldest star first (the legacy ordered
/// by the user-row surrogate key) with the public-id tiebreak appended.
const STARRED_ALBUM_SELECT: &str =
    "SELECT release.public_api_id, release.title, release.sort_title,
    release.created_at, extract(year FROM release.release_date)::integer,
    COALESCE(aggregate.track_count, 0), COALESCE(aggregate.duration_ms, 0),
    artist.public_api_id, artist.canonical_name, artist.sort_name,
    COALESCE(artist_aggregate.release_count, 0),
    ustate.rating, ustate.updated_at,
    (SELECT count(*) FROM native_track_plays play
      JOIN tracks play_track ON play_track.id = play.track_id
      WHERE play_track.release_id = release.id),
    (SELECT max(play.played_at) FROM native_track_plays play
      JOIN tracks play_track ON play_track.id = play.track_id
      WHERE play_track.release_id = release.id)
  FROM native_release_user_states ustate
  JOIN releases release ON release.id = ustate.release_id AND release.status = 'active'
  JOIN LATERAL (
    SELECT artist.* FROM release_artists billing
    JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.release_id = release.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) artist ON true
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
  LEFT JOIN artist_catalog_aggregates artist_aggregate
    ON artist_aggregate.artist_id = artist.id
  WHERE ustate.user_id = $1 AND ustate.starred
  ORDER BY ustate.created_at, release.public_api_id
  LIMIT $2";

fn starred_album_rows(rows: &[sqlx::postgres::PgRow]) -> Vec<StarredAlbumRow> {
    rows.iter()
        .map(|row| {
            Ok(StarredAlbumRow {
                release_id: row.try_get(0)?,
                title: row.try_get(1)?,
                sort_title: row.try_get(2)?,
                created_at: row.try_get(3)?,
                year: row.try_get(4)?,
                song_count: row.try_get(5)?,
                duration_ms: row.try_get(6)?,
                artist_id: row.try_get(7)?,
                artist_name: row.try_get(8)?,
                artist_sort_name: row.try_get(9)?,
                artist_album_count: row.try_get(10)?,
                user_rating: row.try_get(11)?,
                starred_at: row.try_get(12)?,
                played_count: row.try_get(13)?,
                last_played_at: row.try_get(14)?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .unwrap_or_default()
}

/// The captured legacy getStarred album is the full Child directory object.
fn starred_album_child(row: &StarredAlbumRow, genres: &[String]) -> Value {
    let album_id = format!("album_{}", row.release_id);
    let artist_id = format!("artist_{}", row.artist_id);
    let mut album = Map::new();
    album.insert("id".to_string(), album_id.clone().into());
    album.insert("parent".to_string(), artist_id.clone().into());
    album.insert("isDir".to_string(), true.into());
    album.insert("title".to_string(), row.title.clone().into());
    album.insert("album".to_string(), row.title.clone().into());
    album.insert("artist".to_string(), row.artist_name.clone().into());
    if let Some(year) = row.year {
        album.insert("year".to_string(), year.into());
    }
    album.insert("coverArt".to_string(), album_id.clone().into());
    // The Child `starred` field is the user row's last-updated timestamp (the
    // captured "" is the legacy fixture seeding the audit field as null).
    album.insert("starred".to_string(), rfc3339_z(row.starred_at).into());
    album.insert("duration".to_string(), (row.duration_ms / 1000).into());
    album.insert("playCount".to_string(), row.played_count.into());
    if let Some(played) = row.last_played_at {
        album.insert("played".to_string(), rfc3339_z(played).into());
    }
    album.insert("created".to_string(), rfc3339_z(row.created_at).into());
    album.insert("albumId".to_string(), album_id.into());
    album.insert("artistId".to_string(), artist_id.into());
    album.insert("type".to_string(), "music".into());
    album.insert("mediaType".to_string(), "album".into());
    album.insert("isVideo".to_string(), false.into());
    album.insert("sortName".to_string(), row.sort_title.clone().into());
    if !genres.is_empty() {
        album.insert(
            "genres".to_string(),
            Value::Array(
                genres
                    .iter()
                    .map(|genre| {
                        serde_json::json!({"value": genre, "songCount": 0, "albumCount": 0})
                    })
                    .collect(),
            ),
        );
    }
    album.insert("artists".to_string(), serde_json::json!([]));
    album.insert("displayArtist".to_string(), row.artist_name.clone().into());
    album.insert(
        "albumArtists".to_string(),
        serde_json::json!([{
            "id": format!("artist_{}", row.artist_id),
            "name": row.artist_name,
            "coverArt": format!("artist_{}", row.artist_id),
            "albumCount": row.artist_album_count,
            // The legacy nested ArtistID3 is built without the user row.
            "userRating": 0,
            "artistImageUrl": "URL",
            "sortName": row.artist_sort_name,
        }]),
    );
    album.insert(
        "displayAlbumArtist".to_string(),
        row.artist_name.clone().into(),
    );
    album.insert("contributors".to_string(), serde_json::json!([]));
    album.insert("replayGain".to_string(), serde_json::json!({}));
    album.insert("averageRating".to_string(), 0.into());
    album.insert("userRating".to_string(), i64::from(row.user_rating).into());
    album.insert("playerId".to_string(), 0.into());
    if let Some(genre) = genres.first() {
        album.insert("genre".to_string(), genre.clone().into());
    }
    Value::Object(album)
}

/// The captured getStarred2 album is the AlbumID3 object.
fn starred_album_id3(row: &StarredAlbumRow, genres: &[String]) -> Value {
    let album_id = format!("album_{}", row.release_id);
    let mut album = Map::new();
    album.insert("id".to_string(), album_id.clone().into());
    album.insert("name".to_string(), row.title.clone().into());
    album.insert("artist".to_string(), row.artist_name.clone().into());
    album.insert(
        "artistId".to_string(),
        format!("artist_{}", row.artist_id).into(),
    );
    album.insert("coverArt".to_string(), album_id.into());
    album.insert("songCount".to_string(), row.song_count.into());
    album.insert("duration".to_string(), (row.duration_ms / 1000).into());
    album.insert("playCount".to_string(), row.played_count.into());
    album.insert("created".to_string(), rfc3339_z(row.created_at).into());
    album.insert("starred".to_string(), rfc3339_z(row.starred_at).into());
    if let Some(year) = row.year {
        album.insert("year".to_string(), year.into());
    }
    if !genres.is_empty() {
        album.insert(
            "genres".to_string(),
            Value::Array(genres.iter().map(|genre| genre.clone().into()).collect()),
        );
        album.insert("genre".to_string(), genres[0].clone().into());
    }
    Value::Object(album)
}

struct StarredArtistRow {
    artist_id: Uuid,
    name: String,
    sort_name: String,
    album_count: i64,
    user_rating: i16,
    starred_at: OffsetDateTime,
}

const STARRED_ARTIST_SELECT: &str = "SELECT artist.public_api_id, artist.canonical_name,
    artist.sort_name, COALESCE(aggregate.release_count, 0), ustate.rating, ustate.updated_at
  FROM native_artist_user_states ustate
  JOIN artists artist ON artist.id = ustate.artist_id
  LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
  WHERE ustate.user_id = $1 AND ustate.starred
  ORDER BY ustate.created_at, artist.public_api_id
  LIMIT $2";

fn starred_artist_rows(rows: &[sqlx::postgres::PgRow]) -> Vec<StarredArtistRow> {
    rows.iter()
        .map(|row| {
            Ok(StarredArtistRow {
                artist_id: row.try_get(0)?,
                name: row.try_get(1)?,
                sort_name: row.try_get(2)?,
                album_count: row.try_get(3)?,
                user_rating: row.try_get(4)?,
                starred_at: row.try_get(5)?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .unwrap_or_default()
}

/// Legacy getStarred artist: the non-ID3 Artist DTO, whose artistImageUrl is
/// the renderable `/images/{apiKey}/medium` path (not the "URL" placeholder).
fn starred_artist_legacy(row: &StarredArtistRow, average_rating: f64) -> Value {
    let artist_id = format!("artist_{}", row.artist_id);
    let starred = rfc3339_z(row.starred_at);
    serde_json::json!({
        "id": artist_id,
        "name": row.name,
        "albumCount": row.album_count,
        "userRating": row.user_rating,
        "averageRating": average_rating,
        "coverArt": artist_id,
        "artistImageUrl": format!("/images/{}/medium", row.artist_id),
        "userStarred": starred,
        "starred": starred,
    })
}

/// ArtistID3: keeps the captured literal `artistImageUrl: "URL"` placeholder.
fn starred_artist_id3(row: &StarredArtistRow) -> Value {
    let artist_id = format!("artist_{}", row.artist_id);
    serde_json::json!({
        "id": artist_id,
        "name": row.name,
        "coverArt": artist_id,
        "albumCount": row.album_count,
        "userRating": row.user_rating,
        "artistImageUrl": "URL",
        "starred": rfc3339_z(row.starred_at),
        "sortName": row.sort_name,
    })
}

async fn starred(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    id3_shape: bool,
) -> Response {
    let key = if id3_shape { "starred2" } else { "starred" };
    let user_id = resolve_user_id(state, params, principal).await;
    let Some(user_id) = user_id else {
        return reply(
            state,
            params,
            OsReply::ok(serde_json::json!({key: {"artist": [], "album": [], "song": []}})),
        );
    };
    let artist_rows = sqlx::query(STARRED_ARTIST_SELECT)
        .bind(user_id)
        .bind(MAX_LIST_SIZE)
        .fetch_all(&state.pool)
        .await;
    let album_rows = sqlx::query(STARRED_ALBUM_SELECT)
        .bind(user_id)
        .bind(MAX_LIST_SIZE)
        .fetch_all(&state.pool)
        .await;
    let song_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT track.public_api_id
         FROM native_track_user_states ustate
         JOIN tracks track ON track.id = ustate.track_id
         WHERE ustate.user_id = $1 AND ustate.starred
         ORDER BY ustate.created_at, track.public_api_id
         LIMIT $2",
    )
    .bind(user_id)
    .bind(MAX_LIST_SIZE)
    .fetch_all(&state.pool)
    .await;
    let (artist_rows, album_rows, song_ids) = match (artist_rows, album_rows, song_ids) {
        (Ok(artists), Ok(albums), Ok(songs)) => (artists, albums, songs),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            tracing::error!(%error, "opensubsonic starred query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let artists = starred_artist_rows(&artist_rows);
    let albums = starred_album_rows(&album_rows);
    let release_ids: Vec<Uuid> = albums.iter().map(|album| album.release_id).collect();
    let genres_by_release = release_genres(&state.pool, &release_ids)
        .await
        .unwrap_or_default();
    let songs = match song_children(&state.pool, user_id, &song_ids).await {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic starred songs projection failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let artist_ids: Vec<Uuid> = artists.iter().map(|artist| artist.artist_id).collect();
    let average_ratings = artist_average_ratings(&state.pool, &artist_ids)
        .await
        .unwrap_or_default();
    let artist_entries: Vec<Value> = artists
        .iter()
        .map(|artist| {
            if id3_shape {
                starred_artist_id3(artist)
            } else {
                starred_artist_legacy(
                    artist,
                    average_ratings
                        .get(&artist.artist_id)
                        .copied()
                        .unwrap_or(0.0),
                )
            }
        })
        .collect();
    let album_entries: Vec<Value> = albums
        .iter()
        .map(|album| {
            let genres = genres_by_release
                .get(&album.release_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if id3_shape {
                starred_album_id3(album, genres)
            } else {
                starred_album_child(album, genres)
            }
        })
        .collect();
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({key: {
            "artist": artist_entries,
            "album": album_entries,
            "song": songs,
        }})),
    )
}

/// Artist `averageRating` (legacy CalculatedRating decimal): the mean of
/// nonzero user ratings, 0.0 when unrated.
async fn artist_average_ratings(
    pool: &PgPool,
    artist_public_ids: &[Uuid],
) -> Result<HashMap<Uuid, f64>, sqlx::Error> {
    let mut averages = HashMap::new();
    if artist_public_ids.is_empty() {
        return Ok(averages);
    }
    let rows = sqlx::query(
        "SELECT artist.public_api_id, AVG(ratings.rating)::float8
         FROM artists artist
         JOIN native_artist_user_states ratings ON ratings.artist_id = artist.id
         WHERE artist.public_api_id = ANY($1) AND ratings.rating > 0
         GROUP BY artist.public_api_id",
    )
    .bind(artist_public_ids)
    .fetch_all(pool)
    .await?;
    for row in rows {
        averages.insert(row.try_get(0)?, row.try_get(1)?);
    }
    Ok(averages)
}

pub async fn get_starred(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    starred(&state, &params, &principal, false).await
}

pub async fn get_starred2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    starred(&state, &params, &principal, true).await
}

// ---------------------------------------------------------------------------
// search2 / search3
// ---------------------------------------------------------------------------

/// Latin letters with canonical decompositions fold to their base letter,
/// mirroring the legacy `RemoveAccents` (NFD + combining-mark strip); letters
/// without a decomposition (æ, œ, ø, ß, …) are deleted by the trailing
/// `translate`, matching the legacy `[^A-Z0-9]` strip.
const FOLD_FROM: &str = "àáâãäåāăąèéêëēėęìíîïīòóôõöōőùúûüūůűçćčñńšýÿžæœøßþðłđħı";
const FOLD_TO: &str = "aaaaaaaaaeeeeeeeiiiiiooooooouuuuuuucccnnsyyz";

/// SQL form of the legacy normalized-name comparison for one column.
fn fold_sql(column: &str) -> String {
    format!(
        "regexp_replace(upper(translate(lower({column}), '{FOLD_FROM}', '{FOLD_TO}')), '[^A-Z0-9]', '', 'g')"
    )
}

/// Rust form of the same fold for the query term. Returns `None` when the
/// query is absent/blank (the legacy then applies no filter at all) and a
/// never-matching sentinel when it folds to empty — the legacy base64-falls
/// back there, which likewise matches no normalized name.
fn fold_query(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let from: Vec<char> = FOLD_FROM.chars().collect();
    let to: Vec<char> = FOLD_TO.chars().collect();
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        for lower in ch.to_lowercase() {
            if lower.is_ascii_alphanumeric() {
                out.push(lower.to_ascii_uppercase());
            } else if let Some(position) = from.iter().position(|&f| f == lower)
                && let Some(base) = to.get(position)
            {
                out.push(base.to_ascii_uppercase());
            }
        }
    }
    if out.is_empty() {
        // SOH can never appear in a folded name, so the LIKE never matches.
        Some('\u{1}'.to_string())
    } else {
        Some(out)
    }
}

/// Captured paging quirk: the legacy computes `page = offset / count + 1`,
/// so the effective offset rounds down to a page boundary.
fn search_window(offset: i64, count: i64) -> (i64, i64) {
    let count = count.clamp(0, MAX_SEARCH_PAGE);
    if count == 0 {
        return (0, 0);
    }
    ((offset / count) * count, count)
}

struct SearchAlbumRow {
    release_id: Uuid,
    title: String,
    created_at: OffsetDateTime,
    song_count: i64,
    duration_ms: i64,
    artist_id: Uuid,
    artist_name: String,
}

async fn search(state: &OsState, params: &Option<Extension<Params>>, is_search3: bool) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let raw_query = params_ref.and_then(|p| p.get("query"));
    let term = fold_query(raw_query).map(|term| format!("%{term}%"));
    let artist_count = params_ref
        .and_then(|p| p.get_i64("artistcount"))
        .unwrap_or(DEFAULT_SEARCH_PAGE);
    let album_count = params_ref
        .and_then(|p| p.get_i64("albumcount"))
        .unwrap_or(DEFAULT_SEARCH_PAGE);
    let song_count = params_ref
        .and_then(|p| p.get_i64("songcount"))
        .unwrap_or(DEFAULT_SEARCH_PAGE);
    // Captured quirk: a negative offset is replaced by the default page size.
    let artist_offset = params_ref
        .and_then(|p| p.get_i64("artistoffset"))
        .map_or(0, |raw| if raw < 0 { DEFAULT_SEARCH_PAGE } else { raw });
    let album_offset = params_ref
        .and_then(|p| p.get_i64("albumoffset"))
        .map_or(0, |raw| if raw < 0 { DEFAULT_SEARCH_PAGE } else { raw });
    let song_offset = params_ref
        .and_then(|p| p.get_i64("songoffset"))
        .map_or(0, |raw| if raw < 0 { DEFAULT_SEARCH_PAGE } else { raw });

    // Captured total-count behavior: with a blank query and exactly one raw
    // count param equal to 1, x-total-count reports that entity's total.
    let mut total_count = 0_i64;
    if raw_query.is_none_or(|query| query.trim().is_empty()) {
        let which = if params_ref.and_then(|p| p.get_i64("albumcount")) == Some(1) {
            Some("releases")
        } else if params_ref.and_then(|p| p.get_i64("artistcount")) == Some(1) {
            Some("artists")
        } else if params_ref.and_then(|p| p.get_i64("songcount")) == Some(1) {
            Some("tracks")
        } else {
            None
        };
        if let Some(table) = which {
            let count_sql = match table {
                "releases" => "SELECT count(*) FROM releases WHERE status = 'active'",
                "artists" => {
                    "SELECT count(*) FROM artists WHERE status NOT IN ('merged', 'deprecated')"
                }
                _ => "SELECT count(*) FROM tracks",
            };
            total_count = sqlx::query_scalar::<_, i64>(count_sql)
                .fetch_one(&state.pool)
                .await
                .unwrap_or(0);
        }
    }

    let (artist_skip, artist_take) = search_window(artist_offset, artist_count);
    let (album_skip, album_take) = search_window(album_offset, album_count);
    let (song_skip, song_take) = search_window(song_offset, song_count);

    let artist_fold = fold_sql("artist.canonical_name");
    let alias_fold = fold_sql("alias.alias");
    let album_fold = fold_sql("release.title");
    let song_fold = fold_sql("track.title");

    let artists: Vec<Value> = if artist_take > 0 {
        let query = format!(
            "SELECT artist.public_api_id, artist.canonical_name,
                    COALESCE(aggregate.release_count, 0) AS album_count
             FROM artists artist
             LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
             WHERE artist.status NOT IN ('merged', 'deprecated')
               AND ($1::text IS NULL OR {artist_fold} LIKE $1
                 OR EXISTS (SELECT 1 FROM artist_aliases alias
                            WHERE alias.artist_id = artist.id AND {alias_fold} LIKE $1))
             ORDER BY artist.canonical_name COLLATE \"C\", artist.public_api_id
             LIMIT $2 OFFSET $3"
        );
        match sqlx::query(AssertSqlSafe(query))
            .bind(term.as_deref())
            .bind(artist_take)
            .bind(artist_skip)
            .fetch_all(&state.pool)
            .await
        {
            Ok(rows) => rows
                .iter()
                .filter_map(|row| {
                    let id = row.try_get::<Uuid, _>(0).ok()?;
                    let name = row.try_get::<String, _>(1).ok()?;
                    let album_count = row.try_get::<i64, _>(2).ok()?;
                    Some(serde_json::json!({
                        "id": format!("artist_{id}"),
                        "name": name,
                        "coverArt": format!("artist_{id}"),
                        "albumCount": album_count,
                    }))
                })
                .collect(),
            Err(error) => {
                tracing::error!(%error, "opensubsonic search artist query failed");
                return error_reply(state, params, "A generic error.");
            }
        }
    } else {
        Vec::new()
    };

    let album_rows: Vec<SearchAlbumRow> = if album_take > 0 {
        let query = format!(
            "SELECT release.public_api_id, release.title, release.created_at,
                    COALESCE(aggregate.track_count, 0), COALESCE(aggregate.duration_ms, 0),
                    artist.public_api_id, artist.canonical_name
             FROM releases release
             JOIN LATERAL (
               SELECT billing_artist.public_api_id, billing_artist.canonical_name
               FROM release_artists billing
               JOIN artists billing_artist ON billing_artist.id = billing.artist_id
               WHERE billing.release_id = release.id AND billing.role = 'primary'
               ORDER BY billing.presentation_order, billing.id LIMIT 1
             ) artist ON true
             LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
             WHERE release.status = 'active'
               AND ($1::text IS NULL OR {album_fold} LIKE $1)
             ORDER BY release.title COLLATE \"C\", release.public_api_id
             LIMIT $2 OFFSET $3"
        );
        match sqlx::query(AssertSqlSafe(query))
            .bind(term.as_deref())
            .bind(album_take)
            .bind(album_skip)
            .fetch_all(&state.pool)
            .await
        {
            Ok(rows) => rows
                .iter()
                .map(|row| {
                    Ok(SearchAlbumRow {
                        release_id: row.try_get(0)?,
                        title: row.try_get(1)?,
                        created_at: row.try_get(2)?,
                        song_count: row.try_get(3)?,
                        duration_ms: row.try_get(4)?,
                        artist_id: row.try_get(5)?,
                        artist_name: row.try_get(6)?,
                    })
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()
                .unwrap_or_default(),
            Err(error) => {
                tracing::error!(%error, "opensubsonic search album query failed");
                return error_reply(state, params, "A generic error.");
            }
        }
    } else {
        Vec::new()
    };
    let release_ids: Vec<Uuid> = album_rows.iter().map(|album| album.release_id).collect();
    let genres_by_release = release_genres(&state.pool, &release_ids)
        .await
        .unwrap_or_default();
    let albums: Vec<Value> = album_rows
        .iter()
        .map(|row| {
            let album_id = format!("album_{}", row.release_id);
            let mut album = Map::new();
            album.insert("id".to_string(), album_id.clone().into());
            album.insert("name".to_string(), row.title.clone().into());
            album.insert("coverArt".to_string(), album_id.into());
            album.insert("songCount".to_string(), row.song_count.into());
            album.insert("artist".to_string(), row.artist_name.clone().into());
            album.insert(
                "artistId".to_string(),
                format!("artist_{}", row.artist_id).into(),
            );
            album.insert("duration".to_string(), (row.duration_ms / 1000).into());
            album.insert("created".to_string(), rfc3339_z(row.created_at).into());
            let genres = genres_by_release.get(&row.release_id);
            if let Some(genres) = genres.filter(|genres| !genres.is_empty()) {
                album.insert(
                    "genres".to_string(),
                    Value::Array(genres.iter().map(|genre| genre.clone().into()).collect()),
                );
            }
            Value::Object(album)
        })
        .collect();

    let songs: Vec<Value> = if song_take > 0 {
        let query = format!(
            "SELECT track.public_api_id, track.title, COALESCE(track.track_number, track.ordinal),
                    COALESCE(track.duration_ms, media.duration_ms, 0), media.byte_size,
                    track.created_at,
                    release.public_api_id, release.title,
                    extract(year FROM release.release_date)::integer,
                    artist.public_api_id, artist.canonical_name
             FROM tracks track
             JOIN releases release ON release.id = track.release_id AND release.status = 'active'
             JOIN media_assets media ON media.id = track.playable_media_asset_id
             JOIN LATERAL (
               SELECT billing_artist.public_api_id, billing_artist.canonical_name
               FROM release_artists billing
               JOIN artists billing_artist ON billing_artist.id = billing.artist_id
               WHERE billing.release_id = release.id AND billing.role = 'primary'
               ORDER BY billing.presentation_order, billing.id LIMIT 1
             ) artist ON true
             WHERE ($1::text IS NULL OR {song_fold} LIKE $1)
             ORDER BY artist.canonical_name COLLATE \"C\", release.title COLLATE \"C\", track.ordinal, track.public_api_id
             LIMIT $2 OFFSET $3"
        );
        let rows = match sqlx::query(AssertSqlSafe(query))
            .bind(term.as_deref())
            .bind(song_take)
            .bind(song_skip)
            .fetch_all(&state.pool)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "opensubsonic search song query failed");
                return error_reply(state, params, "A generic error.");
            }
        };
        let track_ids: Vec<Uuid> = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>(0))
            .collect::<Result<_, _>>()
            .unwrap_or_default();
        let genres_by_track = track_genres(&state.pool, &track_ids)
            .await
            .unwrap_or_default();
        rows.iter()
            .filter_map(|row| {
                let track_id = row.try_get::<Uuid, _>(0).ok()?;
                let title = row.try_get::<String, _>(1).ok()?;
                let number = row.try_get::<i32, _>(2).ok()?;
                let duration_ms = row.try_get::<i64, _>(3).ok()?;
                let byte_size = row.try_get::<i64, _>(4).ok()?;
                let created_at = row.try_get::<OffsetDateTime, _>(5).ok()?;
                let release_id = row.try_get::<Uuid, _>(6).ok()?;
                let release_title = row.try_get::<String, _>(7).ok()?;
                let year = row.try_get::<Option<i32>, _>(8).ok()?;
                let artist_id = row.try_get::<Uuid, _>(9).ok()?;
                let artist_name = row.try_get::<String, _>(10).ok()?;
                let genres = genres_by_track.get(&track_id).cloned().unwrap_or_default();
                let song_id = format!("song_{track_id}");
                let album_id = format!("album_{release_id}");
                let mut song = Map::new();
                song.insert("type".to_string(), "music".into());
                song.insert("isDir".to_string(), false.into());
                song.insert("id".to_string(), song_id.clone().into());
                song.insert("parent".to_string(), album_id.clone().into());
                song.insert("title".to_string(), title.into());
                song.insert("album".to_string(), release_title.into());
                song.insert("artist".to_string(), artist_name.into());
                song.insert("coverArt".to_string(), song_id.into());
                song.insert("created".to_string(), rfc3339_z(created_at).into());
                song.insert("duration".to_string(), (duration_ms / 1000).into());
                song.insert("size".to_string(), byte_size.into());
                // Captured placeholder fields: the legacy search pipeline
                // (SongDataInfo) does not carry these, so the DTO hardcodes
                // them.
                song.insert("bitrate".to_string(), 0.into());
                song.insert("track".to_string(), number.into());
                song.insert("year".to_string(), i64::from(year.unwrap_or(0)).into());
                if !genres.is_empty() {
                    song.insert(
                        "genres".to_string(),
                        Value::Array(genres.iter().map(|genre| genre.clone().into()).collect()),
                    );
                }
                song.insert("suffix".to_string(), "mp3".into());
                song.insert("contentType".to_string(), "audio/mpeg".into());
                song.insert("path".to_string(), "unknown".into());
                song.insert("albumId".to_string(), album_id.into());
                song.insert("artistId".to_string(), format!("artist_{artist_id}").into());
                song.insert("genre".to_string(), genres.join(",").into());
                Some(Value::Object(song))
            })
            .collect()
    } else {
        Vec::new()
    };

    let key = if is_search3 {
        "searchResult3"
    } else {
        "searchResult2"
    };
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({key: {
            "album": albums,
            "song": songs,
            "artist": artists,
        }}))
        .with_total_count(total_count),
    )
}

/// Track-level genre display values for search results, keyed by the Track
/// source path like the sibling Child projection.
async fn track_genres(
    pool: &PgPool,
    track_public_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let mut genres: HashMap<Uuid, Vec<String>> = HashMap::new();
    if track_public_ids.is_empty() {
        return Ok(genres);
    }
    let rows = sqlx::query(
        "SELECT DISTINCT ON (track.public_api_id)
           track.public_api_id, file.value->'metadata'->>'genre'
         FROM tracks track
         JOIN catalog_imports catalog_import ON catalog_import.release_id = track.release_id
         JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
         CROSS JOIN LATERAL jsonb_array_elements(
           COALESCE(manifest.manifest_payload->'files', '[]')) file(value)
         WHERE track.public_api_id = ANY($1)
           AND file.value->>'relative_path' = track.source_path
         ORDER BY track.public_api_id, catalog_import.imported_at DESC",
    )
    .bind(track_public_ids)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let track_id: Uuid = row.try_get(0)?;
        let genre: Option<String> = row.try_get(1)?;
        if let Some(genre) = genre {
            let parts: Vec<String> = genre
                .split(';')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
            genres.insert(track_id, parts);
        }
    }
    Ok(genres)
}

pub async fn search2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    search(&state, &params, false).await
}

pub async fn search3(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    search(&state, &params, true).await
}

// ---------------------------------------------------------------------------
// melodeeExtensions[1]: external-provider search endpoints
// ---------------------------------------------------------------------------

/// Captured fixture-environment behavior for the extension search endpoints:
/// the external provider dependency fails and the legacy Blazor error pipeline
/// answers HTTP 302 toward the login error page. The captured Location
/// embedded the full request query — including the auth token and salt — in
/// `ReturnUrl`; that credential leak is deliberately not preserved.
fn extension_redirect() -> Response {
    let mut response = (StatusCode::FOUND, "").into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_static("/Account/Login?ReturnUrl=%2FError"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache,no-store"),
    );
    headers.insert(header::EXPIRES, HeaderValue::from_static("-1"));
    response
}

pub async fn search_for_artist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = (state, params, principal);
    extension_redirect()
}

pub async fn search_for_artist_image(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = (state, params, principal);
    extension_redirect()
}

pub async fn search_for_album_image(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = (state, params, principal);
    extension_redirect()
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getAlbumList", get_album_list))
        .merge(super::endpoint_router("getAlbumList2", get_album_list2))
        .merge(super::endpoint_router("getRandomSongs", get_random_songs))
        .merge(super::endpoint_router("getNowPlaying", get_now_playing))
        .merge(super::endpoint_router("getStarred", get_starred))
        .merge(super::endpoint_router("getStarred2", get_starred2))
        .merge(super::endpoint_router(
            "getSongsByGenre",
            get_songs_by_genre,
        ))
        .merge(super::endpoint_router("search2", search2))
        .merge(super::endpoint_router("search3", search3))
        .merge(super::endpoint_router("searchForArtist", search_for_artist))
        .merge(super::endpoint_router(
            "searchForArtistImage",
            search_for_artist_image,
        ))
        .merge(super::endpoint_router(
            "searchForAlbumImage",
            search_for_album_image,
        ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn list_type_binding_is_case_insensitive_with_numeric_forms() {
        assert_eq!(parse_list_type(None), ListType::NotSet);
        assert_eq!(parse_list_type(Some("newest")), ListType::Newest);
        assert_eq!(
            parse_list_type(Some("AlphabeticalByName")),
            ListType::AlphabeticalByName
        );
        assert_eq!(parse_list_type(Some("BYGENRE")), ListType::ByGenre);
        assert_eq!(parse_list_type(Some("5")), ListType::Recent);
        assert_eq!(parse_list_type(Some("bogus")), ListType::NotSet);
    }

    #[test]
    fn query_fold_matches_the_legacy_normalized_form() {
        // The Dune/Düné collision: both fold to DUNE.
        assert_eq!(fold_query(Some("Dune")), Some("DUNE".to_string()));
        assert_eq!(fold_query(Some("düné")), Some("DUNE".to_string()));
        assert_eq!(fold_query(Some("  Rock ")), Some("ROCK".to_string()));
        // CJK-only queries fold to nothing and therefore match nothing.
        assert_eq!(fold_query(Some("東京")), Some("\u{1}".to_string()));
        // Blank or absent queries apply no filter (legacy Nullify).
        assert_eq!(fold_query(Some("   ")), None);
        assert_eq!(fold_query(None), None);
    }

    #[test]
    fn album_list_entries_omit_genre_keys_only_when_genres_are_empty() {
        let row = AlbumListRow {
            release_id: Uuid::nil(),
            title: "Title".to_string(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            song_count: 1,
            duration_ms: 180_000,
            artist_id: Uuid::nil(),
            artist_name: "Artist".to_string(),
            year: Some(2020),
            user_rating: 0,
            played_count: 0,
            average_rating: 0.0,
        };
        let plain = album_list_entry("library_x", &row, &[], false);
        assert!(plain.get("genres").is_none());
        assert!(plain.get("genre").is_none());
        assert_eq!(plain["parent"], "library_x");
        let with_genres = album_list_entry("library_x", &row, &["Rock".to_string()], false);
        assert_eq!(with_genres["genres"], serde_json::json!(["Rock"]));
        assert_eq!(with_genres["genre"], "Rock");
        // The legacy albumList shape prepends the decimal averageRating.
        let legacy = album_list_entry("library_x", &row, &[], true);
        assert_eq!(legacy["averageRating"], serde_json::json!(0.0));
        assert!(legacy.get("songCount").is_some());
    }

    #[test]
    fn search_window_rounds_offsets_down_to_page_boundaries() {
        assert_eq!(search_window(0, 10), (0, 10));
        assert_eq!(search_window(5, 10), (0, 10));
        assert_eq!(search_window(15, 10), (10, 10));
        assert_eq!(search_window(0, 0), (0, 0));
    }
}
