//! Jellyfin browse endpoints: Items, Artists, Genres/MusicGenres, UserViews,
//! Filters, Similar, InstantMix, and the `/Users/{id}/Items` redirect pair
//! (PRD-API-JF-001/004/005, ADR-0131).
//!
//! Behavior reproduced from the frozen baseline (commit `4028b58d`,
//! `src/Melodee.Blazor/Controllers/Jellyfin/{Items,Artists,Genres,
//! MusicGenres,UserViews,Songs}Controller.cs`) and the golden captures:
//!
//! - Paging: `skip = max(0, startIndex ?? 0)`, `take = clamp(limit ?? 100, 1, 500)`.
//! - `sortBy`/`sortOrder`/`recursive`/`fields`/`userId` are accepted and ignored.
//! - Non-music `includeItemTypes` are silently dropped (empty result, not an error).
//! - `searchTerm` is a normalized CONTAINS match with diacritic folding
//!   (captured: `Dune` matches `Düné`).
//! - Ordering: artists by sort name, albums by release year then sort title,
//!   songs by SongNumber — each with a stable public-id tiebreak (ADR-0131
//!   decision 7 removes the legacy nondeterministic DB-order ties).
//! - Similar/InstantMix legacy random ordering is replaced with a
//!   deterministic public-id order (documented D10-family deviation).
//! - Audio-item `UserData` is the captured always-default object on Items
//!   surfaces (legacy never hydrates it there); playlist items hydrate only
//!   with `enableUserData=true` (see `playlists.rs`).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::auth::{JfPrincipal, resolve_token};
use super::error::{bad_request, not_found, unauthorized};
use super::model::{self, AlbumFacts, ArtistFacts, SongFacts, SongShape};

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/Artists", get(get_artists))
        .route("/Artists/AlbumArtists", get(get_artists))
        .route("/Artists/{id}", get(get_artist))
        .route("/Artists/{id}/Similar", get(get_similar_artists))
        .route("/Genres", get(get_genres))
        .route("/MusicGenres", get(get_music_genres))
        .route("/Items", get(get_items))
        .route("/Items/Filters", get(get_filters))
        .route("/Items/{id}", get(get_item))
        .route("/Items/{id}/Similar", get(get_similar_items))
        .route("/Items/{id}/InstantMix", get(get_items_instant_mix))
        .route("/Items/{id}/Refresh", axum::routing::post(refresh_item))
        .route("/Songs/{id}/InstantMix", get(get_songs_instant_mix))
        .route("/UserViews", get(get_user_views))
        .route("/Users/{id}/Views", get(get_user_views))
        .route("/Users/{id}/Items", get(get_user_items_redirect))
        .route("/Users/{id}/Items/{item}", get(get_user_item_redirect))
}

// ---------------------------------------------------------------------------
// Query plumbing
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ListParams {
    search_term: Option<String>,
    start_index: i64,
    limit: i64,
    parent_id: Option<String>,
    include_item_types: Vec<String>,
    enable_user_data: bool,
}

impl ListParams {
    fn parse(query: &[(String, String)]) -> Self {
        let mut params = Self {
            limit: 100,
            ..Self::default()
        };
        for (key, value) in query {
            match key.as_str() {
                "searchTerm" if !value.is_empty() => params.search_term = Some(value.clone()),
                "startIndex" => params.start_index = value.parse::<i64>().unwrap_or(0).max(0),
                "limit" => params.limit = value.parse::<i64>().unwrap_or(100).clamp(1, 500),
                "parentId" if !value.is_empty() => params.parent_id = Some(value.clone()),
                "includeItemTypes" => {
                    params.include_item_types = value
                        .split(',')
                        .map(str::trim)
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "enableUserData" => params.enable_user_data = value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
        params
    }
}

/// Percent-decodes and splits the raw query string (form encoding; `+` is
/// space). Duplicate keys keep the first occurrence, matching legacy model
/// binding.
pub(crate) fn raw_query(uri: &Uri) -> Vec<(String, String)> {
    let Some(query) = uri.query() else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let mut parts = pair.splitn(2, '=');
        let key = percent_decode(parts.next().unwrap_or_default());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        let value = percent_decode(&parts.next().unwrap_or_default().replace('+', " "));
        out.push((key, value));
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `Uri.EscapeDataString` equivalent for redirect query reconstruction.
pub(crate) fn escape_data_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

fn clamped_limit(query: &[(String, String)], default: i64, max: i64) -> i64 {
    query
        .iter()
        .find(|(key, _)| key == "limit")
        .and_then(|(_, value)| value.parse::<i64>().ok())
        .unwrap_or(default)
        .clamp(1, max)
}

pub(crate) fn has_role(principal: &JfPrincipal, role: &str) -> bool {
    principal.roles.iter().any(|candidate| candidate == role)
}

/// Diacritic folding table mirroring the legacy normalized CONTAINS search
/// (captured: `searchTerm=Dune` matches both `Dune` and `Düné`). Same table
/// the OpenSubsonic adapter uses for its captured topSongs folding.
const FOLD_FROM: &str = "àáâãäåāăąèéêëēėęìíîïīòóôõöōőùúûüūůűçćčñńšýÿžæœøßþðłđħı";
const FOLD_TO: &str = "aaaaaaaaaeeeeeeeiiiiiooooooouuuuuuucccnnsyyz";

fn fold(value: &str) -> String {
    let from: Vec<char> = FOLD_FROM.chars().collect();
    let to: Vec<char> = FOLD_TO.chars().collect();
    let mut out = String::with_capacity(value.len());
    for ch in value.trim().chars() {
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
    out
}

/// SQL expression folding a text column the same way (translate + strip
/// non-alphanumerics), so CONTAINS matching runs in the database.
fn fold_sql(column: &str) -> String {
    format!(
        "upper(regexp_replace(translate(lower({column}), '{FOLD_FROM}', '{FOLD_TO}'), '[^a-z0-9]', '', 'g'))"
    )
}

pub(crate) fn json_response(body: Value) -> Response {
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

/// Applies the captured ETag negotiation: If-None-Match (quote-trimmed,
/// case-insensitive) → 304 with the ETag header; otherwise 200 + ETag.
fn etag_response(body: Value, etag: &str, headers: &HeaderMap) -> Response {
    let quoted = format!("\"{etag}\"");
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_matches('"').eq_ignore_ascii_case(etag));
    if not_modified {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&quoted) {
            response.headers_mut().insert(header::ETAG, value);
        }
        return response;
    }
    let mut response = json_response(body);
    if let Ok(value) = HeaderValue::from_str(&quoted) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

pub(crate) fn db_failure(route: &str, error: sqlx::Error) -> Response {
    tracing::error!(%error, route, "jellyfin browse query failed");
    not_found("Item not found.")
}

// ---------------------------------------------------------------------------
// Catalog loaders
// ---------------------------------------------------------------------------

async fn load_artists(
    pool: &PgPool,
    search_term: Option<&str>,
    skip: i64,
    take: i64,
) -> Result<(Vec<ArtistFacts>, i64), sqlx::Error> {
    let fold_expr = fold_sql("artist.canonical_name");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM artists artist \
         WHERE artist.status NOT IN ('merged', 'deprecated') \
           AND ($1::text IS NULL OR {fold_expr} LIKE '%' || $1 || '%')"
    )))
    .bind(search_term.map(fold))
    .fetch_one(pool)
    .await?;
    // Frozen ordering quirk: unfiltered listings sort by SortName, but the
    // searchTerm path sorts by Name (canonical_name) — the Dune/Düné
    // collision pair proves both orders (`jf-browse/items-artists` vs
    // `jf-browse/items-searchterm-dune`).
    let order_clause = if search_term.is_some() {
        "artist.canonical_name COLLATE \"C\", artist.public_api_id"
    } else {
        "artist.sort_name COLLATE \"C\", artist.public_api_id"
    };
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT artist.public_api_id, artist.canonical_name, artist.sort_name, \
                artist.created_at, artist.updated_at, \
                COALESCE(aggregate.release_count, 0) \
           FROM artists artist \
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id \
          WHERE artist.status NOT IN ('merged', 'deprecated') \
            AND ($1::text IS NULL OR {fold_expr} LIKE '%' || $1 || '%') \
          ORDER BY {order_clause} \
          LIMIT $2 OFFSET $3"
    )))
    .bind(search_term.map(fold))
    .bind(take)
    .bind(skip)
    .fetch_all(pool)
    .await?;
    let artists = rows
        .into_iter()
        .map(|row| ArtistFacts {
            public_id: row.get(0),
            name: row.get(1),
            sort_name: row.get(2),
            created_at: row.get(3),
            updated_at: row.get(4),
            album_count: row.get(5),
        })
        .collect();
    Ok((artists, total))
}

async fn load_artist(pool: &PgPool, public_id: Uuid) -> Result<Option<ArtistFacts>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT artist.public_api_id, artist.canonical_name, artist.sort_name, \
                artist.created_at, artist.updated_at, \
                COALESCE(aggregate.release_count, 0) \
           FROM artists artist \
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id \
          WHERE artist.public_api_id = $1 AND artist.status NOT IN ('merged', 'deprecated')",
    )
    .bind(public_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| ArtistFacts {
        public_id: row.get(0),
        name: row.get(1),
        sort_name: row.get(2),
        created_at: row.get(3),
        updated_at: row.get(4),
        album_count: row.get(5),
    }))
}

/// Batch genre lookup from ingestion manifests (the only genre source in the
/// Rust catalog at this baseline), mirroring the OpenSubsonic adapter.
async fn release_genres(
    pool: &PgPool,
    release_public_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let mut genres: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    if release_public_ids.is_empty() {
        return Ok(genres);
    }
    let rows = sqlx::query(
        "SELECT release.public_api_id, file.value->'metadata'->>'genre' AS genre \
          FROM releases release \
          JOIN catalog_imports catalog_import ON catalog_import.release_id = release.id \
          JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id \
          CROSS JOIN LATERAL jsonb_array_elements( \
            COALESCE(manifest.manifest_payload->'files', '[]')) file(value) \
         WHERE release.public_api_id = ANY($1) \
         ORDER BY release.public_api_id, catalog_import.imported_at DESC",
    )
    .bind(release_public_ids)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let release_id: Uuid = row.get(0);
        let genre: Option<String> = row.get(1);
        let entry = genres.entry(release_id).or_default();
        if let Some(genre) = genre {
            for part in genre.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                if !entry.iter().any(|known| known == part) {
                    entry.push(part.to_string());
                }
            }
        }
    }
    Ok(genres)
}

const PRIMARY_ARTIST_NAME: &str = "\
    (SELECT artist.canonical_name FROM release_artists billing \
      JOIN artists artist ON artist.id = billing.artist_id \
     WHERE billing.release_id = release.id AND billing.role = 'primary' \
     ORDER BY billing.presentation_order LIMIT 1)";

const PRIMARY_ARTIST_ID: &str = "\
    (SELECT artist.public_api_id FROM release_artists billing \
      JOIN artists artist ON artist.id = billing.artist_id \
     WHERE billing.release_id = release.id AND billing.role = 'primary' \
     ORDER BY billing.presentation_order LIMIT 1)";

const ALBUM_SELECT: &str = "\
    SELECT release.public_api_id, release.title, release.sort_title, \
           extract(year FROM release.release_date)::integer, \
           COALESCE(aggregate.track_count, 0), COALESCE(aggregate.duration_ms, 0), \
           release.created_at, release.updated_at";
// PRIMARY_ARTIST_NAME/PRIMARY_ARTIST_ID are appended as columns 8 and 9.

fn map_album_row(
    row: sqlx::postgres::PgRow,
    genres: Vec<String>,
    user_data: Option<Value>,
) -> AlbumFacts {
    let artist_name: Option<String> = row.get(8);
    let artist_id: Option<Uuid> = row.get(9);
    AlbumFacts {
        public_id: row.get(0),
        title: row.get(1),
        sort_title: row.get(2),
        year: row.get(3),
        track_count: row.get(4),
        duration_ms: row.get(5),
        genres,
        album_artists: match (artist_name, artist_id) {
            (Some(name), Some(id)) => vec![(name, id)],
            _ => Vec::new(),
        },
        created_at: row.get(6),
        updated_at: row.get(7),
        user_data,
    }
}

async fn load_albums(
    pool: &PgPool,
    search_term: Option<&str>,
    artist_public_id: Option<Uuid>,
    skip: i64,
    take: i64,
) -> Result<(Vec<AlbumFacts>, i64), sqlx::Error> {
    let filter = fold_sql("release.title");
    let where_clause = format!(
        "release.status = 'active' \
         AND ($1::text IS NULL OR {filter} LIKE '%' || $1 || '%') \
         AND ($2::uuid IS NULL OR EXISTS ( \
               SELECT 1 FROM release_artists billing \
               JOIN artists artist ON artist.id = billing.artist_id \
               WHERE billing.release_id = release.id AND billing.role = 'primary' \
                 AND artist.public_api_id = $2))"
    );
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM releases release WHERE {where_clause}"
    )))
    .bind(search_term.map(fold))
    .bind(artist_public_id)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{ALBUM_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} \
           FROM releases release \
           LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id \
         WHERE {where_clause} \
         ORDER BY extract(year FROM release.release_date) NULLS LAST, \
                  release.sort_title COLLATE \"C\", release.public_api_id \
         LIMIT $3 OFFSET $4"
    )))
    .bind(search_term.map(fold))
    .bind(artist_public_id)
    .bind(take)
    .bind(skip)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.get(0)).collect();
    let genres = release_genres(pool, &ids).await?;
    let albums = rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get(0);
            map_album_row(row, genres.get(&id).cloned().unwrap_or_default(), None)
        })
        .collect();
    Ok((albums, total))
}

async fn load_album(pool: &PgPool, public_id: Uuid) -> Result<Option<AlbumFacts>, sqlx::Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{ALBUM_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} \
           FROM releases release \
           LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id \
         WHERE release.status = 'active' AND release.public_api_id = $1"
    )))
    .bind(public_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let genres = release_genres(pool, &[public_id]).await?;
    Ok(Some(map_album_row(
        row,
        genres.get(&public_id).cloned().unwrap_or_default(),
        None,
    )))
}

// The frozen baseline passes the legacy SongNumber through as
// `IndexNumber` unchanged (multi-disc numbering such as 101/201 is emitted
// raw; `ParentIndexNumber` stays null), so no disc/track decomposition.
const SONG_SELECT: &str = "\
    SELECT track.public_api_id, track.title, track.sort_title, \
           track.track_number, \
           track.duration_ms, media.container, media.byte_size, media.bitrate_bps, \
           media.sample_rate_hz, \
           release.title, release.public_api_id, track.created_at, track.updated_at";
// PRIMARY_ARTIST_NAME/PRIMARY_ARTIST_ID are appended as columns 13 and 14.

fn map_song_row(row: sqlx::postgres::PgRow, user_data: Option<Value>) -> SongFacts {
    let artist_name: Option<String> = row.get(13);
    let artist_id: Option<Uuid> = row.get(14);
    let artists = match (artist_name, artist_id) {
        (Some(name), Some(id)) => vec![(name, id)],
        _ => Vec::new(),
    };
    SongFacts {
        public_id: row.get(0),
        title: row.get(1),
        sort_title: row.get(2),
        song_number: row.get(3),
        duration_ms: row.get(4),
        container: row.get(5),
        byte_size: row.get(6),
        bitrate_bps: row.get(7),
        sample_rate_hz: row.get(8),
        release_title: row.get(9),
        release_public_id: row.get(10),
        track_artists: artists.clone(),
        album_artists: artists,
        created_at: row.get(11),
        updated_at: row.get(12),
        user_data,
    }
}

const SONG_FROM: &str = "\
      FROM tracks track \
      JOIN releases release ON release.id = track.release_id AND release.status = 'active' \
      JOIN media_assets media ON media.id = track.playable_media_asset_id";

async fn load_songs(
    pool: &PgPool,
    search_term: Option<&str>,
    album_public_id: Option<Uuid>,
    skip: i64,
    take: i64,
) -> Result<(Vec<SongFacts>, i64), sqlx::Error> {
    let filter = fold_sql("track.title");
    let where_clause = format!(
        "($1::text IS NULL OR {filter} LIKE '%' || $1 || '%') \
         AND ($2::uuid IS NULL OR release.public_api_id = $2)"
    );
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM tracks track \
         JOIN releases release ON release.id = track.release_id AND release.status = 'active' \
         WHERE {where_clause}"
    )))
    .bind(search_term.map(fold))
    .bind(album_public_id)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{SONG_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} {SONG_FROM} \
         WHERE {where_clause} \
         ORDER BY 4, track.public_api_id LIMIT $3 OFFSET $4"
    )))
    .bind(search_term.map(fold))
    .bind(album_public_id)
    .bind(take)
    .bind(skip)
    .fetch_all(pool)
    .await?;
    let key_user_data = |row: &sqlx::postgres::PgRow| {
        let id: Uuid = row.get(0);
        model::user_data(0, false, &model::nf(id), None)
    };
    let songs = rows
        .into_iter()
        .map(|row| {
            let user_data = key_user_data(&row);
            map_song_row(row, Some(user_data))
        })
        .collect();
    Ok((songs, total))
}

pub(crate) async fn load_song(
    pool: &PgPool,
    public_id: Uuid,
) -> Result<Option<SongFacts>, sqlx::Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{SONG_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} {SONG_FROM} \
         WHERE track.public_api_id = $1"
    )))
    .bind(public_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| {
        let user_data = model::user_data(0, false, &model::nf(public_id), None);
        map_song_row(row, Some(user_data))
    }))
}

async fn distinct_genres(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let mut genres: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT btrim(part) \
           FROM releases release \
           JOIN catalog_imports catalog_import ON catalog_import.release_id = release.id \
           JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id \
           CROSS JOIN LATERAL jsonb_array_elements( \
             COALESCE(manifest.manifest_payload->'files', '[]')) file(value) \
           CROSS JOIN LATERAL unnest(string_to_array(file.value->'metadata'->>'genre', ';')) AS part \
          WHERE release.status = 'active' \
            AND file.value->'metadata'->>'genre' IS NOT NULL \
            AND btrim(part) <> ''",
    )
    .fetch_all(pool)
    .await?;
    genres.sort();
    Ok(genres)
}

/// Shared-release-genre subquery: release public ids sharing at least one
/// `;`-separated manifest genre with the `$genres` bind.
const GENRE_OVERLAP_RELEASES: &str = "\
    SELECT genre_release.id FROM catalog_imports catalog_import \
    JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id \
    JOIN releases genre_release ON genre_release.id = catalog_import.release_id \
    CROSS JOIN LATERAL jsonb_array_elements( \
      COALESCE(manifest.manifest_payload->'files', '[]')) file(value) \
    CROSS JOIN LATERAL unnest(string_to_array(file.value->'metadata'->>'genre', ';')) AS part \
    WHERE btrim(part) = ANY($GENRES::text[])";

fn genre_overlap_sql(genres_bind: &str) -> String {
    GENRE_OVERLAP_RELEASES.replace("$GENRES", genres_bind)
}

// ---------------------------------------------------------------------------
// Artists
// ---------------------------------------------------------------------------

async fn get_artists(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let params = ListParams::parse(&raw_query(&uri));
    let (artists, total) = match load_artists(
        &state.pool,
        params.search_term.as_deref(),
        params.start_index,
        params.limit,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return db_failure("get_artists", error),
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let items: Vec<Value> = artists
        .iter()
        .map(|artist| {
            let mut item = model::artist_item(&state.server_id, artist);
            if let Value::Object(ref mut map) = item {
                // The Artists handler derives CanDownload from the role and
                // populates UserData only with enableUserData=true (captured).
                map.insert("CanDownload".to_string(), json!(can_download));
                map.insert(
                    "UserData".to_string(),
                    if params.enable_user_data {
                        model::user_data(0, false, &model::nf(artist.public_id), None)
                    } else {
                        Value::Null
                    },
                );
            }
            item
        })
        .collect();
    let latest = artists
        .iter()
        .map(|artist| artist.updated_at)
        .max()
        .unwrap_or_else(OffsetDateTime::now_utc);
    let etag = model::artists_collection_etag(total, params.start_index, params.limit, latest);
    etag_response(
        model::items_result(items, total, params.start_index),
        &etag,
        &headers,
    )
}

async fn get_artist(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid artist ID format.");
    };
    let artist = match load_artist(&state.pool, public_id).await {
        Ok(Some(artist)) => artist,
        Ok(None) => return not_found("Artist not found."),
        Err(error) => return db_failure("get_artist", error),
    };
    let mut item = model::artist_item(&state.server_id, &artist);
    if let Value::Object(ref mut map) = item {
        map.insert(
            "CanDownload".to_string(),
            json!(has_role(&principal, "HasDownloadRole")),
        );
    }
    let etag = model::item_etag(artist.public_id, artist.updated_at);
    etag_response(item, &etag, &headers)
}

async fn get_similar_artists(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid artist ID format.");
    };
    let limit = clamped_limit(&raw_query(&uri), 10, 50);
    match load_artist(&state.pool, public_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("Artist not found."),
        Err(error) => return db_failure("get_similar_artists/seed", error),
    }
    // Genre-overlap scoring (captured handler), deterministically ordered:
    // overlap desc, then sort name, then public id (D10 tiebreak).
    let rows = match sqlx::query(
        "WITH artist_genres AS ( \
           SELECT billing.artist_id, btrim(part) AS genre \
             FROM release_artists billing \
             JOIN releases release ON release.id = billing.release_id AND release.status = 'active' \
             JOIN catalog_imports catalog_import ON catalog_import.release_id = release.id \
             JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id \
             CROSS JOIN LATERAL jsonb_array_elements( \
               COALESCE(manifest.manifest_payload->'files', '[]')) file(value) \
             CROSS JOIN LATERAL unnest(string_to_array(file.value->'metadata'->>'genre', ';')) AS part \
            WHERE billing.role = 'primary' \
              AND file.value->'metadata'->>'genre' IS NOT NULL \
              AND btrim(part) <> '' \
            GROUP BY billing.artist_id, btrim(part) \
         ), seed AS ( \
           SELECT id AS artist_id FROM artists WHERE public_api_id = $1 \
         ) \
         SELECT artist.public_api_id, artist.canonical_name, artist.sort_name, \
                artist.created_at, artist.updated_at, \
                COALESCE(aggregate.release_count, 0), count(*) AS overlap \
           FROM artist_genres candidate \
           JOIN artists artist ON artist.id = candidate.artist_id \
            AND artist.status NOT IN ('merged', 'deprecated') \
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id \
          WHERE candidate.genre IN ( \
                  SELECT genre FROM artist_genres JOIN seed ON artist_genres.artist_id = seed.artist_id) \
            AND candidate.artist_id <> (SELECT artist_id FROM seed) \
          GROUP BY artist.public_api_id, artist.canonical_name, artist.sort_name, \
                   artist.created_at, artist.updated_at, aggregate.release_count \
          ORDER BY overlap DESC, artist.sort_name COLLATE \"C\", artist.public_api_id \
          LIMIT $2",
    )
    .bind(public_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => return db_failure("get_similar_artists", error),
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let facts = ArtistFacts {
                public_id: row.get(0),
                name: row.get(1),
                sort_name: row.get(2),
                created_at: row.get(3),
                updated_at: row.get(4),
                album_count: row.get(5),
            };
            let mut item = model::artist_item(&state.server_id, &facts);
            if let Value::Object(ref mut map) = item {
                map.insert("CanDownload".to_string(), json!(can_download));
            }
            item
        })
        .collect();
    let total = items.len() as i64;
    json_response(model::items_result(items, total, 0))
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

async fn genres_response(
    state: &JfState,
    uri: &Uri,
    headers: &HeaderMap,
    handler: &str,
) -> Response {
    let params = ListParams::parse(&raw_query(uri));
    let mut genres = match distinct_genres(&state.pool).await {
        Ok(genres) => genres,
        Err(error) => return db_failure("get_genres", error),
    };
    if let Some(term) = params.search_term.as_deref() {
        let folded = fold(term);
        genres.retain(|genre| fold(genre).contains(&folded));
    }
    let total = genres.len() as i64;
    let now = OffsetDateTime::now_utc();
    let items: Vec<Value> = genres
        .iter()
        .skip(params.start_index as usize)
        .take(params.limit as usize)
        .map(|genre| model::genre_item(&state.server_id, genre, now))
        .collect();
    let etag = model::genres_collection_etag(handler, total, params.start_index, params.limit, now);
    etag_response(
        model::items_result(items, total, params.start_index),
        &etag,
        headers,
    )
}

async fn get_genres(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    genres_response(&state, &uri, &headers, "genres").await
}

async fn get_music_genres(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    genres_response(&state, &uri, &headers, "musicgenres").await
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

async fn get_items(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let params = ListParams::parse(&raw_query(&uri));
    let requested: Vec<String> = params
        .include_item_types
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect();

    // Captured dispatch order: MusicArtist, then MusicAlbum, then Audio, then
    // the parentId probe.
    if requested.iter().any(|value| value == "musicartist") {
        let (artists, total) = match load_artists(
            &state.pool,
            params.search_term.as_deref(),
            params.start_index,
            params.limit,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => return db_failure("get_items/artists", error),
        };
        let items = artists
            .iter()
            .map(|artist| {
                let mut item = model::artist_item(&state.server_id, artist);
                if let Value::Object(ref mut map) = item {
                    // The Items artist list passes literal CanDownload=true.
                    map.insert("CanDownload".to_string(), json!(true));
                    map.insert(
                        "UserData".to_string(),
                        if params.enable_user_data {
                            model::user_data(0, false, &model::nf(artist.public_id), None)
                        } else {
                            Value::Null
                        },
                    );
                }
                item
            })
            .collect();
        return json_response(model::items_result(items, total, params.start_index));
    }

    if requested.iter().any(|value| value == "musicalbum") {
        let artist_id = params.parent_id.as_deref().and_then(model::parse_id);
        return album_items_response(&state, &params, artist_id).await;
    }

    if requested.iter().any(|value| value == "audio") {
        return song_items_response(&state, &params).await;
    }

    // parentId probe (captured dispatch): artist -> albums; album -> songs;
    // library -> songs. The Rust catalog has a single canonical store, so a
    // library parentId is accepted but not a filter (documented Phase 8
    // decision; legacy partitioned songs by Library rows).
    if let Some(parent) = params.parent_id.as_deref().and_then(model::parse_id) {
        match load_artist(&state.pool, parent).await {
            Ok(Some(_)) => return album_items_response(&state, &params, Some(parent)).await,
            Ok(None) => {}
            Err(error) => return db_failure("get_items/probe", error),
        }
        let is_album = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM releases WHERE public_api_id = $1 AND status = 'active'",
        )
        .bind(parent)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0)
            > 0;
        let is_library = state.libraries.iter().any(|library| library.id == parent);
        if is_album || is_library {
            let mut scoped = ListParams {
                parent_id: None,
                ..ListParams::parse(&raw_query(&uri))
            };
            if is_album {
                scoped.parent_id = Some(model::nf(parent));
            }
            return song_items_response(&state, &scoped).await;
        }
    }

    json_response(model::items_result(Vec::new(), 0, params.start_index))
}

async fn album_items_response(
    state: &JfState,
    params: &ListParams,
    artist_public_id: Option<Uuid>,
) -> Response {
    let (albums, total) = match load_albums(
        &state.pool,
        params.search_term.as_deref(),
        artist_public_id,
        params.start_index,
        params.limit,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return db_failure("album_items", error),
    };
    let items = albums
        .into_iter()
        .map(|mut album| {
            if params.enable_user_data {
                album.user_data = Some(model::user_data(
                    0,
                    false,
                    &model::nf(album.public_id),
                    None,
                ));
            }
            model::album_item(&state.server_id, &album)
        })
        .collect();
    json_response(model::items_result(items, total, params.start_index))
}

async fn song_items_response(state: &JfState, params: &ListParams) -> Response {
    let album_id = params
        .parent_id
        .as_deref()
        .and_then(model::parse_id)
        .filter(|id| !state.libraries.iter().any(|library| library.id == *id));
    let (songs, total) = match load_songs(
        &state.pool,
        params.search_term.as_deref(),
        album_id,
        params.start_index,
        params.limit,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return db_failure("song_items", error),
    };
    let items = songs
        .into_iter()
        .map(|song| model::song_item(&state.server_id, &song, SongShape::Full))
        .collect();
    json_response(model::items_result(items, total, params.start_index))
}

async fn get_item(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };

    // Captured probe order: artist, then album, then song. This route sends
    // no ETag header (captured).
    match load_artist(&state.pool, public_id).await {
        Ok(Some(artist)) => {
            let mut item = model::artist_item(&state.server_id, &artist);
            if let Value::Object(ref mut map) = item {
                map.insert("CanDownload".to_string(), json!(can_download));
            }
            return json_response(item);
        }
        Ok(None) => {}
        Err(error) => return db_failure("get_item/artist", error),
    }
    match load_album(&state.pool, public_id).await {
        Ok(Some(mut album)) => {
            album.user_data = Some(model::user_data(0, false, &model::nf(public_id), None));
            let mut item = model::album_item(&state.server_id, &album);
            if let Value::Object(ref mut map) = item {
                map.insert("CanDownload".to_string(), json!(can_download));
            }
            return json_response(item);
        }
        Ok(None) => {}
        Err(error) => return db_failure("get_item/album", error),
    }
    match load_song(&state.pool, public_id).await {
        Ok(Some(song)) => {
            let mut item = model::song_item(&state.server_id, &song, SongShape::Full);
            if let Value::Object(ref mut map) = item {
                map.insert("CanDownload".to_string(), json!(can_download));
            }
            return json_response(item);
        }
        Ok(None) => {}
        Err(error) => return db_failure("get_item/song", error),
    }
    not_found("Item not found.")
}

async fn get_filters(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let genres = match distinct_genres(&state.pool).await {
        Ok(genres) => genres,
        Err(error) => return db_failure("get_filters", error),
    };
    let years: Vec<i32> = match sqlx::query_scalar(
        "SELECT DISTINCT extract(year FROM release_date)::integer FROM releases \
         WHERE status = 'active' AND release_date IS NOT NULL ORDER BY 1 DESC",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(years) => years,
        Err(error) => return db_failure("get_filters/years", error),
    };
    // Captured lowercase keys (anonymous type via the camelCase serializer).
    json_response(json!({
        "genres": genres,
        "tags": [],
        "years": years,
    }))
}

async fn get_similar_items(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let limit = clamped_limit(&raw_query(&uri), 20, 100);

    // Song seed -> similar songs (same release primary artist or shared
    // genre), deterministically ordered by public id (D10 deviation from the
    // captured random order).
    match load_song(&state.pool, public_id).await {
        Ok(Some(seed)) => {
            let seed_genres = release_genres(&state.pool, &[seed.release_public_id])
                .await
                .unwrap_or_default()
                .remove(&seed.release_public_id)
                .unwrap_or_default();
            let seed_artist = seed.album_artists.first().map(|(_, id)| *id);
            let rows = match sqlx::query(sqlx::AssertSqlSafe(format!(
                "{SONG_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} {SONG_FROM} \
                 WHERE track.public_api_id <> $1 \
                   AND (($2::uuid IS NOT NULL AND EXISTS ( \
                          SELECT 1 FROM release_artists billing \
                          JOIN artists artist ON artist.id = billing.artist_id \
                          WHERE billing.release_id = release.id AND billing.role = 'primary' \
                            AND artist.public_api_id = $2)) \
                        OR release.id IN ({})) \
                 ORDER BY track.public_api_id LIMIT $4",
                genre_overlap_sql("$3")
            )))
            .bind(public_id)
            .bind(seed_artist)
            .bind(&seed_genres)
            .bind(limit)
            .fetch_all(&state.pool)
            .await
            {
                Ok(rows) => rows,
                Err(error) => return db_failure("get_similar_items/songs", error),
            };
            let items = rows
                .into_iter()
                .map(|row| {
                    let song = map_song_row(row, None);
                    let mut item = model::song_item(&state.server_id, &song, SongShape::Full);
                    if let Value::Object(ref mut map) = item {
                        map.insert("CanDownload".to_string(), json!(can_download));
                    }
                    item
                })
                .collect::<Vec<_>>();
            let total = items.len() as i64;
            return json_response(model::items_result(items, total, 0));
        }
        Ok(None) => {}
        Err(error) => return db_failure("get_similar_items/seed-song", error),
    }

    // Album seed -> similar albums.
    match load_album(&state.pool, public_id).await {
        Ok(Some(seed)) => {
            let seed_artist = seed.album_artists.first().map(|(_, id)| *id);
            let rows = match sqlx::query(sqlx::AssertSqlSafe(format!(
                "{ALBUM_SELECT}, {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} \
                   FROM releases release \
                   LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id \
                 WHERE release.status = 'active' AND release.public_api_id <> $1 \
                   AND (($2::uuid IS NOT NULL AND EXISTS ( \
                          SELECT 1 FROM release_artists billing \
                          JOIN artists artist ON artist.id = billing.artist_id \
                          WHERE billing.release_id = release.id AND billing.role = 'primary' \
                            AND artist.public_api_id = $2)) \
                        OR release.id IN ({})) \
                 ORDER BY release.public_api_id LIMIT $4",
                genre_overlap_sql("$3")
            )))
            .bind(public_id)
            .bind(seed_artist)
            .bind(&seed.genres)
            .bind(limit)
            .fetch_all(&state.pool)
            .await
            {
                Ok(rows) => rows,
                Err(error) => return db_failure("get_similar_items/albums", error),
            };
            let ids: Vec<Uuid> = rows.iter().map(|row| row.get(0)).collect();
            let genres = release_genres(&state.pool, &ids).await.unwrap_or_default();
            let items = rows
                .into_iter()
                .map(|row| {
                    let id: Uuid = row.get(0);
                    let album = map_album_row(
                        row,
                        genres.get(&id).cloned().unwrap_or_default(),
                        Some(model::user_data(0, false, &model::nf(id), None)),
                    );
                    let mut item = model::album_item(&state.server_id, &album);
                    if let Value::Object(ref mut map) = item {
                        map.insert("CanDownload".to_string(), json!(can_download));
                    }
                    item
                })
                .collect::<Vec<_>>();
            let total = items.len() as i64;
            return json_response(model::items_result(items, total, 0));
        }
        Ok(None) => {}
        Err(error) => return db_failure("get_similar_items/seed-album", error),
    }

    // Captured quirk: artist ids (and unknown ids) return the empty envelope
    // here — only song/album seeds are probed.
    json_response(model::items_result(Vec::new(), 0, 0))
}

/// One InstantMix query for every captured seed kind. Binds:
/// `$1` seed track public id (None for album/artist seeds),
/// `$2` seed release public id (album seed only),
/// `$3` seed primary artist public id,
/// `$4` seed release genres,
/// `$5` limit.
async fn instant_mix_rows(
    state: &JfState,
    seed_track: Option<Uuid>,
    seed_release: Option<Uuid>,
    seed_artist: Option<Uuid>,
    seed_genres: &[String],
    limit: i64,
) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT track.public_api_id, track.title, \
                track.track_number, \
                track.duration_ms, track.created_at, \
                release.title, release.public_api_id, \
                extract(year FROM release.release_date)::integer, \
                {PRIMARY_ARTIST_NAME}, {PRIMARY_ARTIST_ID} \
           FROM tracks track \
           JOIN releases release ON release.id = track.release_id AND release.status = 'active' \
         WHERE ( \
                ($1::uuid IS NOT NULL AND track.public_api_id <> $1) \
             OR ($1::uuid IS NULL) \
           ) \
           AND ( \
                ($2::uuid IS NOT NULL AND release.public_api_id = $2) \
             OR ($3::uuid IS NOT NULL AND EXISTS ( \
                  SELECT 1 FROM release_artists billing \
                  JOIN artists artist ON artist.id = billing.artist_id \
                  WHERE billing.release_id = release.id AND billing.role = 'primary' \
                    AND artist.public_api_id = $3)) \
             OR release.id IN ({}) \
           ) \
         ORDER BY track.public_api_id LIMIT $5",
        genre_overlap_sql("$4")
    )))
    .bind(seed_track)
    .bind(seed_release)
    .bind(seed_artist)
    .bind(seed_genres)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
}

async fn instant_mix_response(
    state: &JfState,
    public_id: Uuid,
    limit: i64,
    can_download: bool,
    songs_only: bool,
) -> Response {
    // Captured seed probing: song -> album -> artist (Items route); the Songs
    // route probes songs only. Legacy picks random rows; the adapter returns
    // a deterministic public-id order (documented D10-family deviation).
    let seed_song = match load_song(&state.pool, public_id).await {
        Ok(song) => song,
        Err(error) => return db_failure("instant_mix/seed-song", error),
    };
    let mut seed_track = None;
    let mut seed_release = None;
    let mut seed_artist = None;
    let mut seed_genres: Vec<String> = Vec::new();

    if let Some(song) = &seed_song {
        seed_track = Some(public_id);
        seed_artist = song.album_artists.first().map(|(_, id)| *id);
        seed_genres = release_genres(&state.pool, &[song.release_public_id])
            .await
            .unwrap_or_default()
            .remove(&song.release_public_id)
            .unwrap_or_default();
    } else if !songs_only {
        match load_album(&state.pool, public_id).await {
            Ok(Some(album)) => {
                seed_release = Some(public_id);
                seed_artist = album.album_artists.first().map(|(_, id)| *id);
                seed_genres = album.genres.clone();
            }
            Ok(None) => match load_artist(&state.pool, public_id).await {
                Ok(Some(_)) => {
                    seed_artist = Some(public_id);
                }
                Ok(None) => {}
                Err(error) => return db_failure("instant_mix/seed-artist", error),
            },
            Err(error) => return db_failure("instant_mix/seed-album", error),
        }
    }

    if seed_track.is_none() && seed_release.is_none() && seed_artist.is_none() {
        return not_found(if songs_only {
            "Song not found."
        } else {
            "Item not found."
        });
    }

    let rows = match instant_mix_rows(
        state,
        seed_track,
        seed_release,
        seed_artist,
        &seed_genres,
        limit,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => return db_failure("instant_mix", error),
    };
    let release_ids: Vec<Uuid> = rows.iter().map(|row| row.get(6)).collect();
    let genres_by_release = release_genres(&state.pool, &release_ids)
        .await
        .unwrap_or_default();
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let release_id: Uuid = row.get(6);
        let artist_name: Option<String> = row.get(8);
        let artist_id: Option<Uuid> = row.get(9);
        items.push(model::mix_item(
            &state.server_id,
            row.get(0),
            &row.get::<String, _>(1),
            can_download,
            row.get(2),
            row.get(3),
            &row.get::<String, _>(5),
            release_id,
            row.get(7),
            artist_name.zip(artist_id),
            genres_by_release.get(&release_id).cloned(),
            row.get(4),
        ));
    }
    let total = items.len() as i64;
    json_response(model::items_result(items, total, 0))
}

async fn get_items_instant_mix(
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
    let limit = clamped_limit(&raw_query(&uri), 200, 300);
    let can_download = has_role(&principal, "HasDownloadRole");
    instant_mix_response(&state, public_id, limit, can_download, false).await
}

async fn get_songs_instant_mix(
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
    let limit = clamped_limit(&raw_query(&uri), 200, 300);
    let can_download = has_role(&principal, "HasDownloadRole");
    instant_mix_response(&state, public_id, limit, can_download, true).await
}

async fn refresh_item(
    State(state): State<JfState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    if model::parse_id(&id).is_none() {
        return bad_request("Invalid item ID format.");
    }
    // Captured no-op: refresh requests are acknowledged, never executed.
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// UserViews and the /Users/{id}/Items redirect pair
// ---------------------------------------------------------------------------

async fn get_user_views(State(state): State<JfState>, headers: HeaderMap, uri: Uri) -> Response {
    let Some(principal) = resolve_token(&state, &headers, &uri).await else {
        return unauthorized();
    };
    let can_download = has_role(&principal, "HasDownloadRole");
    let has_stream = has_role(&principal, "HasStreamRole");
    let mut items: Vec<Value> = state
        .libraries
        .iter()
        .map(|library| {
            let mut item = model::user_view_item(
                &state.server_id,
                library.id,
                &library.name,
                library.created_at,
            );
            if let Value::Object(ref mut map) = item {
                map.insert("CanDownload".to_string(), json!(can_download));
                map.insert(
                    "PlayAccess".to_string(),
                    json!(if has_stream { "Full" } else { "None" }),
                );
            }
            item
        })
        .collect();
    if items.is_empty() {
        // Captured quirk: no unlocked Storage libraries -> a synthetic "Music"
        // view with a per-request random GUID and record-default fields.
        let id = Uuid::now_v7();
        items.push(json!({
            "Name": "Music",
            "ServerId": state.server_id,
            "Id": model::nf(id),
            "Etag": null,
            "DateCreated": null,
            "CanDelete": false,
            "CanDownload": can_download,
            "SortName": null,
            "ExternalUrls": null,
            "Path": null,
            "EnableMediaSourceDisplay": true,
            "ChannelId": null,
            "Taglines": null,
            "Genres": null,
            "PlayAccess": if has_stream { "Full" } else { "None" },
            "RemoteTrailers": null,
            "ProviderIds": null,
            "IsFolder": true,
            "ParentId": null,
            "Type": "UserView",
            "Studios": null,
            "GenreItems": null,
            "LocalTrailerCount": 0,
            "UserData": null,
            "ChildCount": 0,
            "SpecialFeatureCount": 0,
            "DisplayPreferencesId": null,
            "Tags": null,
            "PrimaryImageAspectRatio": null,
            "CollectionType": "music",
            "ImageTags": null,
            "BackdropImageTags": null,
            "ScreenshotImageTags": null,
            "ImageBlurHashes": null,
            "LocationType": "Virtual",
            "LockedFields": null,
            "LockData": false
        }));
    }
    let total = items.len() as i64;
    json_response(model::items_result(items, total, 0))
}

/// GET /Users/{id}/Items → 307 RedirectPreserveMethod to /Items with the
/// captured query reconstruction (whitelisted params in fixed order, booleans
/// re-serialized as `True`/`False`, `userId` appended). Key matching is
/// case-insensitive — legacy `[FromQuery]` binding matched case-insensitively
/// and real clients (Finamp, Feishin) send PascalCase keys like
/// `IncludeItemTypes`.
async fn get_user_items_redirect(
    State(state): State<JfState>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let query = raw_query(&uri);
    let mut parts: Vec<String> = Vec::new();
    for key in [
        "includeItemTypes",
        "parentId",
        "albumArtistIds",
        "artistIds",
        "albumIds",
    ] {
        if let Some((_, value)) = query
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        {
            parts.push(format!("{key}={}", escape_data_string(value)));
        }
    }
    for key in ["recursive", "enableUserData"] {
        if let Some((_, value)) = query
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        {
            let rendered = if value.eq_ignore_ascii_case("true") {
                "True"
            } else {
                "False"
            };
            parts.push(format!("{key}={rendered}"));
        }
    }
    for key in [
        "sortBy",
        "sortOrder",
        "fields",
        "searchTerm",
        "genreIds",
        "filters",
        "startIndex",
        "limit",
    ] {
        if let Some((_, value)) = query
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        {
            parts.push(format!("{key}={}", escape_data_string(value)));
        }
    }
    parts.push(format!("userId={}", escape_data_string(&user_id)));
    let location = format!("/Items?{}", parts.join("&"));
    redirect_preserve_method(&location)
}

/// GET /Users/{id}/Items/{itemId} → 307 to /Items/{dashed-guid}. The GUID is
/// re-parsed and re-serialized (captured open-redirect hardening); invalid ids
/// get the captured non-ProblemDetails 400 shape.
async fn get_user_item_redirect(
    State(state): State<JfState>,
    Path((_user_id, item_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let Some(item_id) = model::parse_id(&item_id) else {
        return json_response_with_status(
            StatusCode::BAD_REQUEST,
            json!({"error": "Invalid item ID format"}),
        );
    };
    redirect_preserve_method(&format!("/Items/{item_id}"))
}

fn redirect_preserve_method(location: &str) -> Response {
    let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn json_response_with_status(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}
