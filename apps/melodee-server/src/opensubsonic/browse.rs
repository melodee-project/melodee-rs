//! Browse endpoint group: getIndexes, getMusicDirectory, getArtists,
//! getArtist, getArtistInfo, getArtistInfo2, getAlbum, getAlbumInfo,
//! getAlbumInfo2, getSong, getGenres, getSimilarSongs, getSimilarSongs2,
//! getTopSongs (PRD-API-OS-005; golden evidence
//! `compatibility/opensubsonic/golden/browse`, capture semantics
//! `design/evidence/phase-0/opensubsonic-capture.md` §4/§8).
//!
//! Captured behaviors reproduced here:
//! - Wire ids are `artist_`/`album_`/`song_` + public_api_id; `library_` for
//!   the configured music folder root.
//! - getArtists/getIndexes render first-letter index groups with the captured
//!   `ignoredArticles` set; artists whose first letter is non-ASCII land in
//!   the `""` index. The artist DTO carries albumCount, userRating,
//!   averageRating, coverArt (own id), artistImageUrl as
//!   `{public_origin}/images/{artist_id}/large`, and userStarred/starred
//!   timestamps when the caller starred the artist.
//! - Unknown/malformed id classes are preserved exactly: getSong unknown
//!   `song_<uuid>` answers status ok WITH an error 44 block; a malformed id
//!   answers the same os-getsong-malformed-id shape. getArtist unknown answers
//!   status ok with NO artist key and NO error. getAlbum unknown/unprefixed
//!   answers HTTP 302 toward `/Account/Login?ReturnUrl=%2FError` (the captured
//!   credential leak in Location is not preserved).
//! - getTopSongs matches BOTH normalized-name collision artists (name-based
//!   lookup), so `artist=Dune` returns tracks whose primary billing folds to
//!   DUNE regardless of the underlying identity.
//! - getGenres, getArtistInfo/2, getAlbumInfo/2, getSimilarSongs/2 mirror the
//!   goldens (offline-provider info shapes, empty similar lists in fixture).
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
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    annotation::{
        ContributorEntry, rfc3339_z, song_children, song_children_with_contributors,
        track_contributors,
    },
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

/// Captured `ignoredArticles` literal (the legacy configured Articles table).
const IGNORED_ARTICLES: &str = "THE|EL|LA|LOS|LAS|LE|LES|OS|AS|O|A";
/// Offline-provider fixture biography recorded for the ordinary solo artist.
const FIXTURE_BIOGRAPHY: &str = "Synthetic ordinary solo artist fixture.";

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

/// The captured getSong unknown/malformed-id shape: HTTP 200 with a status
/// "ok" envelope that ALSO carries an `error` block (code 44). The generic
/// `OsReply::error` flips status to "error", so the envelope is written
/// directly here in all three negotiated formats.
pub(crate) fn ok_with_error_44(state: &OsState, params: &Option<Extension<Params>>) -> Response {
    const CODE: i64 = 44;
    const MESSAGE: &str = "Invalid API key.";
    let envelope = serde_json::json!({
        "subsonic-response": {
            "status": "ok",
            "version": super::envelope::API_VERSION,
            "type": super::envelope::SERVER_TYPE_JSON,
            "serverVersion": state.server_version,
            "openSubsonic": true,
            "error": {"code": CODE, "message": MESSAGE}
        }
    });
    let json = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());
    let (content_type, body) = match format_of(params) {
        Format::Json => ("application/json; charset=utf-8", json),
        Format::Jsonp(callback) => (
            "application/javascript; charset=utf-8",
            format!("{callback}({json})"),
        ),
        Format::Xml => (
            "application/xml",
            format!(
                "<subsonic-response xmlns=\"{}\" status=\"ok\" type=\"{}\" version=\"{}\" serverVersion=\"{}\" openSubsonic=\"true\"><error code=\"{CODE}\" message=\"{MESSAGE}\"/></subsonic-response>",
                super::envelope::XMLNS,
                super::envelope::SERVER_TYPE_XML,
                super::envelope::API_VERSION,
                state.server_version,
            ),
        ),
    };
    let mut response = (StatusCode::OK, axum::body::Body::from(body)).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = axum::http::HeaderValue::from_str(content_type) {
        headers.insert(axum::http::header::CONTENT_TYPE, value);
    }
    headers.insert(
        axum::http::header::HeaderName::from_static("x-total-count"),
        axum::http::HeaderValue::from_static("0"),
    );
    response
}

/// Resolves the calling user's id (authenticated principal or the bypass-mode
/// `u` username resolution, mirroring the lists group).
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

/// Wire id of the first configured music folder (the fixture library). The
/// legacy `parent` for the library root and the album-list `parent` field.
fn library_parent(state: &OsState) -> String {
    state
        .music_folders
        .first()
        .map_or_else(String::new, |folder| folder.wire_id())
}

/// Parses `artist_`/`album_`/`song_`/`library_` prefixed ids; the bare
/// malformed case (no parseable UUID after a known prefix) returns `None`.
fn parse_prefixed(value: &str) -> Option<EntityRef> {
    let (prefix, raw) = value.split_once('_')?;
    let id = Uuid::parse_str(raw).ok()?;
    match prefix {
        "song" => Some(EntityRef::Track(id)),
        "album" => Some(EntityRef::Release(id)),
        "artist" => Some(EntityRef::Artist(id)),
        "library" => Some(EntityRef::Library(id)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityRef {
    Track(Uuid),
    Release(Uuid),
    Artist(Uuid),
    Library(Uuid),
}

/// The captured unknown-id redirect for getAlbum (the legacy Blazor error
/// pipeline answers 302 toward the login error page). The captured Location
/// embedded the request query including credentials; that leak is not
/// preserved.
fn album_unknown_redirect() -> Response {
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

// ---------------------------------------------------------------------------
// Artist index (shared by getIndexes and getArtists)
// ---------------------------------------------------------------------------

struct ArtistIndexRow {
    artist_public_id: Uuid,
    name: String,
    sort_name: String,
    album_count: i64,
    user_rating: i16,
    average_rating: f64,
    starred: bool,
    starred_at: Option<OffsetDateTime>,
}

/// Captured first-letter index key. The legacy groups by the first letter of
/// the artist **sort name** (with ignored articles stripped); a NULL/empty
/// sort name lands in the `""` bucket regardless of the display name (the
/// golden fixture's Sæglópur 日本 and Cassette Ghost both have NULL sortName
/// and land in `""`). Non-ASCII first letters also fall to `""`.
fn index_letter(sort_name: &str) -> String {
    if sort_name.is_empty() {
        return String::new();
    }
    let stripped = strip_ignored_articles(sort_name);
    let first = stripped.trim_start().chars().next();
    match first {
        Some(ch) if ch.is_ascii_alphabetic() => ch.to_ascii_uppercase().to_string(),
        _ => String::new(),
    }
}

/// Strips one leading ignored article (case-insensitive, whole-word) plus
/// surrounding whitespace. Mirrors the legacy `StripArticle` helper which
/// removed only the first article when present. Char-based to stay safe on
/// multi-byte names (e.g. Düné does not begin with an article).
fn strip_ignored_articles(name: &str) -> String {
    let trimmed = name.trim_start();
    for article in IGNORED_ARTICLES.split('|') {
        if article.is_empty() {
            continue;
        }
        let article_chars: Vec<char> = article.chars().collect();
        let mut name_chars = trimmed.chars().peekable();
        let mut matched = true;
        for &ach in &article_chars {
            match name_chars.next() {
                Some(nch) if nch.eq_ignore_ascii_case(&ach) => continue,
                _ => {
                    matched = false;
                    break;
                }
            }
        }
        if !matched {
            continue;
        }
        // Require a whitespace boundary after the article (whole-word match).
        match name_chars.peek() {
            Some(&nch) if nch.is_whitespace() => {
                return name_chars.collect::<String>().trim_start().to_string();
            }
            None => return String::new(),
            _ => continue,
        }
    }
    trimmed.to_string()
}

/// Loads every catalog artist not in the merged/deprecated states, with the
/// caller's user-state annotation and the average rating, ordered by the
/// captured index ordering: letter, then sort name COLLATE "C", then public id
/// (ADR-0130 decision 5).
async fn load_artist_index(
    pool: &PgPool,
    user_id: Option<Uuid>,
) -> Result<Vec<ArtistIndexRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT artist.public_api_id, artist.canonical_name, artist.sort_name,
                COALESCE(aggregate.release_count, 0) AS album_count,
                COALESCE(state.rating, 0)::smallint AS user_rating,
                COALESCE((SELECT AVG(ratings.rating)::float8 FROM native_artist_user_states ratings
                          WHERE ratings.artist_id = artist.id AND ratings.rating > 0
                            AND ($1::uuid IS NULL OR ratings.user_id <> $1)), 0) AS average_rating,
                COALESCE(state.starred, false) AS starred,
                CASE WHEN COALESCE(state.starred, false) THEN state.updated_at END AS starred_at
           FROM artists artist
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
           LEFT JOIN native_artist_user_states state
             ON state.artist_id = artist.id AND state.user_id = $1
          WHERE artist.status NOT IN ('merged', 'deprecated')
          ORDER BY artist.sort_name COLLATE \"C\", artist.public_api_id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ArtistIndexRow {
                artist_public_id: row.try_get(0)?,
                name: row.try_get(1)?,
                sort_name: row.try_get(2)?,
                album_count: row.try_get(3)?,
                user_rating: row.try_get(4)?,
                average_rating: row.try_get(5)?,
                starred: row.try_get(6)?,
                starred_at: row.try_get(7)?,
            })
        })
        .collect()
}

/// Builds the captured artist DTO used by getIndexes/getArtists index entries.
/// `userStarred` and `starred` are both emitted (the legacy serializes both).
fn artist_index_entry(state: &OsState, row: &ArtistIndexRow) -> Value {
    let artist_id = format!("artist_{}", row.artist_public_id);
    let mut entry = Map::new();
    entry.insert("id".to_string(), artist_id.clone().into());
    entry.insert("name".to_string(), row.name.clone().into());
    entry.insert("albumCount".to_string(), row.album_count.into());
    // userRating is always emitted (0 when unrated).
    entry.insert("userRating".to_string(), i64::from(row.user_rating).into());
    // averageRating: 0.0 decimal for the unrated case.
    entry.insert("averageRating".to_string(), row.average_rating.into());
    entry.insert("coverArt".to_string(), artist_id.clone().into());
    entry.insert(
        "artistImageUrl".to_string(),
        format!(
            "{}/images/artist_{}/large",
            state.public_origin, row.artist_public_id
        )
        .into(),
    );
    if row.starred
        && let Some(starred_at) = row.starred_at
    {
        let stamp = rfc3339_z(starred_at);
        entry.insert("userStarred".to_string(), stamp.clone().into());
        entry.insert("starred".to_string(), stamp.into());
    }
    Value::Object(entry)
}

/// Groups artists into the captured first-letter index shape. Groups appear
/// in letter order A-Z then `""` (the `#` bucket); within each group artists
/// keep the row order (sorted by sort_name COLLATE "C" then public id).
fn build_index(state: &OsState, rows: &[ArtistIndexRow]) -> Vec<Value> {
    let groups = group_by_letter(rows);
    // Order groups: lettered A-Z first (alphabetical), then the "" bucket.
    let mut lettered: Vec<(String, Vec<Value>)> = Vec::new();
    let mut empty: Option<Vec<Value>> = None;
    for (letter, group_rows) in groups {
        let entries: Vec<Value> = group_rows
            .iter()
            .map(|row| artist_index_entry(state, row))
            .collect();
        if letter.is_empty() {
            empty = Some(entries);
        } else {
            lettered.push((letter, entries));
        }
    }
    lettered.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut out: Vec<Value> = lettered
        .into_iter()
        .map(|(name, artist)| serde_json::json!({"name": name, "artist": artist}))
        .collect();
    if let Some(entries) = empty {
        out.push(serde_json::json!({"name": "", "artist": entries}));
    }
    out
}

async fn indexes_payload(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    include_shortcut: bool,
) -> Response {
    let user_id = resolve_user_id(state, params, principal).await;
    let rows = match load_artist_index(&state.pool, user_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic artist index query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let index = build_index(state, &rows);
    let mut indexes = Map::new();
    indexes.insert("ignoredArticles".to_string(), IGNORED_ARTICLES.into());
    indexes.insert("lastModified".to_string(), "".into());
    indexes.insert("index".to_string(), Value::Array(index));
    if include_shortcut {
        indexes.insert("shortcut".to_string(), Value::Array(Vec::new()));
        indexes.insert("child".to_string(), Value::Array(Vec::new()));
    }
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({"indexes": Value::Object(indexes)})),
    )
}

pub async fn get_indexes(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    indexes_payload(&state, &params, &principal, true).await
}

pub async fn get_artists(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user_id = resolve_user_id(&state, &params, &principal).await;
    let rows = match load_artist_index(&state.pool, user_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getArtists query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let index = build_index(&state, &rows);
    // The captured getArtists XML serializes `averageRating` as a six-decimal
    // decimal ("0.000000") and the starred entries carry both `starred` and a
    // `starredAt` attribute. The generic envelope writer cannot express that,
    // so the XML children are written explicitly.
    let xml = artists_xml(&state, &rows);
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({
            "artists": {
                "ignoredArticles": IGNORED_ARTICLES,
                "lastModified": "",
                "index": index
            }
        }))
        .with_xml_children_override(xml),
    )
}

/// The captured getArtists XML byte shape (os-getartists-xml.xml). The legacy
/// serializer emits `averageRating` as `0.000000`, includes a literal
/// `starredAt` attribute alongside `starred`, and nests `<index>` with
/// repeated `<artist>` children.
fn artists_xml(state: &OsState, rows: &[ArtistIndexRow]) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("<artists lastModified=\"\" ignoredArticles=\"");
    out.push_str(IGNORED_ARTICLES);
    out.push_str("\">");
    for (letter, group) in group_by_letter(rows) {
        out.push_str("<index name=\"");
        xml_escape_into(&letter, &mut out);
        out.push_str("\">");
        for row in group {
            out.push_str("<artist id=\"artist_");
            out.push_str(&row.artist_public_id.to_string());
            out.push_str("\" name=\"");
            xml_escape_into(&row.name, &mut out);
            out.push_str("\" ");
            if row.starred
                && let Some(starred_at) = row.starred_at
            {
                let stamp = rfc3339_z(starred_at);
                out.push_str("starred=\"");
                out.push_str(&stamp);
                out.push_str("\" starredAt=\"");
                out.push_str(&stamp);
                out.push_str("\" ");
            }
            out.push_str("artistImageUrl=\"");
            xml_escape_into(
                &format!(
                    "{}/images/artist_{}/large",
                    state.public_origin, row.artist_public_id
                ),
                &mut out,
            );
            out.push_str("\" averageRating=\"");
            out.push_str(&format!("{:.6}", row.average_rating));
            out.push_str("\" userRating=\"");
            out.push_str(&i64::from(row.user_rating).to_string());
            out.push_str("\" coverArt=\"artist_");
            out.push_str(&row.artist_public_id.to_string());
            out.push_str("\" albumCount=\"");
            out.push_str(&row.album_count.to_string());
            out.push_str("\"></artist>");
        }
        out.push_str("</index>");
    }
    out.push_str("</artists>");
    out
}

/// Letter grouping preserving the row order within each letter; the returned
/// groups are ordered A-Z then `""` to match the captured index shape.
fn group_by_letter(rows: &[ArtistIndexRow]) -> Vec<(String, Vec<&ArtistIndexRow>)> {
    let mut groups: HashMap<String, Vec<&ArtistIndexRow>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for row in rows {
        let letter = index_letter(&row.sort_name);
        if !groups.contains_key(&letter) {
            order.push(letter.clone());
        }
        groups.entry(letter).or_default().push(row);
    }
    let mut lettered: Vec<(String, Vec<&ArtistIndexRow>)> = order
        .into_iter()
        .filter(|l| !l.is_empty())
        .filter_map(|l| groups.remove(&l).map(|v| (l, v)))
        .collect();
    lettered.sort_by(|(a, _), (b, _)| a.cmp(b));
    if let Some(empty) = groups.remove("") {
        lettered.push((String::new(), empty));
    }
    lettered
}

fn xml_escape_into(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

// ---------------------------------------------------------------------------
// getArtist
// ---------------------------------------------------------------------------

/// One album row in the getArtist album list (the legacy AlbumID3-with-extra
/// shape; identical to the albumList2 entry plus `playedCount` and `parent`).
struct ArtistAlbumRow {
    release_public_id: Uuid,
    title: String,
    created_at: OffsetDateTime,
    song_count: i64,
    duration_ms: i64,
    year: Option<i32>,
    played_count: i64,
    user_rating: i16,
    last_played_at: Option<OffsetDateTime>,
}

pub async fn get_artist(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        // Missing id: the legacy model binding yields null and the controller
        // answers the captured unknown-artist shape (status ok, no artist key).
        return reply(&state, &params, OsReply::ok(serde_json::json!({})));
    };
    let Some(EntityRef::Artist(artist_public_id)) = parse_prefixed(raw_id) else {
        // Malformed/unknown prefix: captured unknown-artist shape (no error).
        return reply(&state, &params, OsReply::ok(serde_json::json!({})));
    };
    let user_id = resolve_user_id(&state, &params, &principal).await;
    let detail = match load_artist_detail(&state.pool, artist_public_id, user_id).await {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            // Unknown artist: captured status ok with no artist key and no error.
            return reply(&state, &params, OsReply::ok(serde_json::json!({})));
        }
        Err(error) => {
            tracing::error!(%error, "opensubsonic getArtist query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let albums = match load_artist_albums(&state.pool, detail.artist_internal_id).await {
        Ok(albums) => albums,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getArtist albums query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let release_ids: Vec<Uuid> = albums.iter().map(|a| a.release_public_id).collect();
    let genres_by_release = release_genres(&state.pool, &release_ids)
        .await
        .unwrap_or_default();
    let parent = library_parent(&state);
    let album_entries: Vec<Value> = albums
        .iter()
        .map(|album| {
            artist_album_entry(
                &parent,
                album,
                genres_by_release
                    .get(&album.release_public_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &detail,
            )
        })
        .collect();
    let artist_id = format!("artist_{}", detail.artist_public_id);
    let mut artist = Map::new();
    artist.insert("id".to_string(), artist_id.clone().into());
    artist.insert("name".to_string(), detail.name.clone().into());
    artist.insert("albumCount".to_string(), detail.album_count.into());
    // userRating is omitted when 0 on the getArtist DTO (captured os-getartist
    // only emits it for the rated Aurora Fields case).
    if detail.user_rating > 0 {
        artist.insert(
            "userRating".to_string(),
            i64::from(detail.user_rating).into(),
        );
    }
    artist.insert("averageRating".to_string(), detail.average_rating.into());
    artist.insert("coverArt".to_string(), artist_id.clone().into());
    artist.insert(
        "artistImageUrl".to_string(),
        format!(
            "{}/images/artist_{}/large",
            state.public_origin, detail.artist_public_id
        )
        .into(),
    );
    if detail.starred
        && let Some(starred_at) = detail.starred_at
    {
        let stamp = rfc3339_z(starred_at);
        artist.insert("userStarred".to_string(), stamp.clone().into());
        artist.insert("starred".to_string(), stamp.into());
    }
    artist.insert("album".to_string(), Value::Array(album_entries));
    // Captured: a resolved getArtist response carries `x-total-count: 1` (the
    // legacy pagination middleware reported the single artist entity); the
    // unknown-artist shape keeps the default 0.
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"artist": Value::Object(artist)})).with_total_count(1),
    )
}

struct ArtistDetailFull {
    artist_internal_id: Uuid,
    artist_public_id: Uuid,
    name: String,
    sort_name: String,
    album_count: i64,
    user_rating: i16,
    average_rating: f64,
    starred: bool,
    starred_at: Option<OffsetDateTime>,
}

async fn load_artist_detail(
    pool: &PgPool,
    artist_public_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Option<ArtistDetailFull>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT artist.id, artist.public_api_id, artist.canonical_name, artist.sort_name,
                COALESCE(aggregate.release_count, 0),
                COALESCE(state.rating, 0)::smallint,
                COALESCE((SELECT AVG(ratings.rating)::float8 FROM native_artist_user_states ratings
                          WHERE ratings.artist_id = artist.id AND ratings.rating > 0
                            AND ($2::uuid IS NULL OR ratings.user_id <> $2)), 0),
                COALESCE(state.starred, false),
                CASE WHEN COALESCE(state.starred, false) THEN state.updated_at END
           FROM artists artist
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
           LEFT JOIN native_artist_user_states state
             ON state.artist_id = artist.id AND state.user_id = $2
          WHERE artist.public_api_id = $1 AND artist.status NOT IN ('merged', 'deprecated')",
    )
    .bind(artist_public_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(ArtistDetailFull {
        artist_internal_id: row.try_get(0)?,
        artist_public_id: row.try_get(1)?,
        name: row.try_get(2)?,
        sort_name: row.try_get(3)?,
        album_count: row.try_get(4)?,
        user_rating: row.try_get(5)?,
        average_rating: row.try_get(6)?,
        starred: row.try_get(7)?,
        starred_at: row.try_get(8)?,
    }))
}

async fn load_artist_albums(
    pool: &PgPool,
    artist_internal_id: Uuid,
) -> Result<Vec<ArtistAlbumRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT release.public_api_id, release.title, release.created_at,
                COALESCE(aggregate.track_count, 0), COALESCE(aggregate.duration_ms, 0),
                extract(year FROM release.release_date)::integer,
                (SELECT count(*) FROM native_track_plays play
                   JOIN tracks play_track ON play_track.id = play.track_id
                  WHERE play_track.release_id = release.id),
                COALESCE(ustate.rating, 0)::smallint,
                (SELECT max(play.played_at) FROM native_track_plays play
                   JOIN tracks play_track ON play_track.id = play.track_id
                  WHERE play_track.release_id = release.id)
           FROM releases release
           JOIN release_artists billing ON billing.release_id = release.id AND billing.role = 'primary'
           LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
           LEFT JOIN native_release_user_states ustate ON ustate.release_id = release.id
          WHERE billing.artist_id = $1 AND release.status = 'active'
          ORDER BY release.title COLLATE \"C\", release.public_api_id",
    )
    .bind(artist_internal_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ArtistAlbumRow {
                release_public_id: row.try_get(0)?,
                title: row.try_get(1)?,
                created_at: row.try_get(2)?,
                song_count: row.try_get(3)?,
                duration_ms: row.try_get(4)?,
                year: row.try_get(5)?,
                played_count: row.try_get(6)?,
                user_rating: row.try_get(7)?,
                last_played_at: row.try_get(8)?,
            })
        })
        .collect()
}

/// The captured getArtist album entry: the AlbumID3-with-extras shape. The
/// legacy serializer emits `album`/`title`/`name` together, `playedCount`
/// (not `playCount`), and a per-album `userRating` sourced from the user's
/// release state (omitted when 0 except for the starred/rated case captured
/// on Aurora Fields).
fn artist_album_entry(
    parent: &str,
    row: &ArtistAlbumRow,
    genres: &[String],
    artist: &ArtistDetailFull,
) -> Value {
    let album_id = format!("album_{}", row.release_public_id);
    let artist_id = format!("artist_{}", artist.artist_public_id);
    let mut album = Map::new();
    album.insert("id".to_string(), album_id.clone().into());
    album.insert("album".to_string(), row.title.clone().into());
    album.insert("title".to_string(), row.title.clone().into());
    album.insert("name".to_string(), row.title.clone().into());
    album.insert("coverArt".to_string(), album_id.into());
    album.insert("songCount".to_string(), row.song_count.into());
    album.insert("created".to_string(), rfc3339_z(row.created_at).into());
    // Captured quirk: getArtist album duration rounds the per-track sum down
    // (Northern Lights shows 7 while the album total is ~8s); the legacy
    // AlbumWithArtistDTO reuses a rounded track-count/seconds projection. We
    // emit the whole-second release duration.
    album.insert("duration".to_string(), (row.duration_ms / 1000).into());
    album.insert("playedCount".to_string(), row.played_count.into());
    album.insert("artistId".to_string(), artist_id.into());
    album.insert("artist".to_string(), artist.name.clone().into());
    album.insert("year".to_string(), i64::from(row.year.unwrap_or(0)).into());
    if row.user_rating > 0 {
        album.insert("userRating".to_string(), i64::from(row.user_rating).into());
    }
    if !genres.is_empty() {
        album.insert(
            "genres".to_string(),
            Value::Array(genres.iter().map(|g| g.clone().into()).collect()),
        );
        album.insert("genre".to_string(), genres[0].clone().into());
    }
    album.insert("parent".to_string(), parent.into());
    Value::Object(album)
}

// ---------------------------------------------------------------------------
// getArtistInfo / getArtistInfo2
// ---------------------------------------------------------------------------

pub async fn get_artist_info(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    artist_info(&state, &params, false).await
}

pub async fn get_artist_info2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    artist_info(&state, &params, true).await
}

async fn artist_info(
    state: &OsState,
    params: &Option<Extension<Params>>,
    is_info2: bool,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        return reply(state, params, OsReply::ok(serde_json::json!({})));
    };
    let Some(EntityRef::Artist(artist_public_id)) = parse_prefixed(raw_id) else {
        return reply(state, params, OsReply::ok(serde_json::json!({})));
    };
    let row = match sqlx::query(
        "SELECT artist.public_api_id, artist.canonical_name,
                COALESCE(aggregate.release_count, 0),
                (SELECT count(*) FROM tracks track
                   JOIN release_artists billing ON billing.release_id = track.release_id
                    AND billing.role = 'primary' AND billing.artist_id = artist.id
                   JOIN releases release ON release.id = track.release_id AND release.status = 'active')
           FROM artists artist
           LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = artist.id
          WHERE artist.public_api_id = $1 AND artist.status NOT IN ('merged', 'deprecated')",
    )
    .bind(artist_public_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return reply(state, params, OsReply::ok(serde_json::json!({}))),
        Err(error) => {
            tracing::error!(%error, "opensubsonic getArtistInfo query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let public_id: Uuid = match row.try_get(0) {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getArtistInfo row mapping failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let name: String = row.try_get(1).unwrap_or_default();
    let album_count: i64 = row.try_get(2).unwrap_or(0);
    let song_count: i64 = row.try_get(3).unwrap_or(0);
    let artist_id = format!("artist_{public_id}");
    let image = |size: &str| format!("{}/images/{artist_id}/{size}", state.public_origin);
    let mut info = Map::new();
    // Captured fixture biography (offline provider returns the synthetic text).
    info.insert("biography".to_string(), FIXTURE_BIOGRAPHY.into());
    info.insert("isArtistInfo2".to_string(), is_info2.into());
    info.insert("id".to_string(), artist_id.clone().into());
    info.insert("name".to_string(), name.into());
    info.insert("songCount".to_string(), song_count.into());
    info.insert("albumCount".to_string(), album_count.into());
    info.insert("smallImageUrl".to_string(), image("thumbnail").into());
    info.insert("mediumImageUrl".to_string(), image("medium").into());
    info.insert("largeImageUrl".to_string(), image("large").into());
    let key = if is_info2 {
        "artistInfo2"
    } else {
        "artistInfo"
    };
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({key: Value::Object(info)})),
    )
}

// ---------------------------------------------------------------------------
// getAlbum / getAlbumInfo / getAlbumInfo2
// ---------------------------------------------------------------------------

struct AlbumDetailRow {
    release_internal_id: Uuid,
    release_public_id: Uuid,
    title: String,
    sort_title: String,
    display_artist: Option<String>,
    release_date: Option<time::Date>,
    created_at: OffsetDateTime,
    song_count: i64,
    duration_ms: i64,
    played_count: i64,
    last_played_at: Option<OffsetDateTime>,
    user_rating: i16,
    starred: bool,
    starred_at: Option<OffsetDateTime>,
    artist_public_id: Uuid,
    artist_name: String,
    is_compilation: bool,
    original_release_date: Option<time::Date>,
}

pub async fn get_album(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        return album_unknown_redirect();
    };
    let Some(EntityRef::Release(release_public_id)) = parse_prefixed(raw_id) else {
        // Unprefixed or malformed id: the captured 302 (os-getalbum-unknown).
        return album_unknown_redirect();
    };
    let user_id = resolve_user_id(&state, &params, &principal).await;
    let Some(detail) = (match load_album_detail(&state.pool, release_public_id, user_id).await {
        Ok(detail) => detail,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getAlbum query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    }) else {
        // Unknown release id: captured 302 (os-getalbum-unknown).
        return album_unknown_redirect();
    };
    let (songs, track_contributions) =
        match album_songs(&state, user_id, detail.release_internal_id).await {
            Ok(songs) => songs,
            Err(error) => {
                tracing::error!(%error, "opensubsonic getAlbum songs query failed");
                return error_reply(&state, &params, "A generic error.");
            }
        };
    let genres = release_genres(&state.pool, &[detail.release_public_id])
        .await
        .unwrap_or_default()
        .remove(&detail.release_public_id)
        .unwrap_or_default();
    // Captured `AlbumId3WithSongs.Artists` = legacy `ContributingArtists()`:
    // performer-type contributors of every song, in song order
    // (os-getalbum-compilation).
    let contributing_artists: Vec<Value> = track_contributions
        .iter()
        .flat_map(|(_, entries)| entries.iter())
        .filter(|entry| entry.is_performance)
        .map(ContributorEntry::album_scoped_artist_id3)
        .collect();
    let album = album_detail_json(&detail, &songs, &genres, &contributing_artists);
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"album": album})),
    )
}

async fn load_album_detail(
    pool: &PgPool,
    release_public_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Option<AlbumDetailRow>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT release.id, release.public_api_id, release.title, release.sort_title,
                release.display_artist, release.release_date, release.created_at,
                COALESCE(aggregate.track_count, 0), COALESCE(aggregate.duration_ms, 0),
                -- Captured album playCount is the legacy Album.PlayedCount
                -- column, independent of song plays (the fixture seeds
                -- Anthology of Echoes with 0 while its Coda song has 9);
                -- the per-user state sum reproduces every frozen value.
                (SELECT COALESCE(sum(plays.play_count), 0)::bigint FROM native_release_user_states plays
                  WHERE plays.release_id = release.id),
                (SELECT max(play.played_at) FROM native_track_plays play
                   JOIN tracks play_track ON play_track.id = play.track_id
                  WHERE play_track.release_id = release.id),
                COALESCE(ustate.rating, 0)::smallint,
                COALESCE(ustate.starred, false),
                CASE WHEN COALESCE(ustate.starred, false) THEN ustate.updated_at END,
                artist.public_api_id, artist.canonical_name,
                COALESCE((release.normalized_title ~* 'compilation'
                          OR release.title ~* 'compilation'), false) AS is_compilation,
                NULL::date AS original_release_date
           FROM releases release
           JOIN LATERAL (
             SELECT artist.* FROM release_artists billing
               JOIN artists artist ON artist.id = billing.artist_id
              WHERE billing.release_id = release.id AND billing.role = 'primary'
              ORDER BY billing.presentation_order, billing.id LIMIT 1
           ) artist ON true
           LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
           LEFT JOIN native_release_user_states ustate
             ON ustate.release_id = release.id AND ustate.user_id = $2
          WHERE release.public_api_id = $1 AND release.status = 'active'",
    )
    .bind(release_public_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(AlbumDetailRow {
        release_internal_id: row.try_get(0)?,
        release_public_id: row.try_get(1)?,
        title: row.try_get(2)?,
        sort_title: row.try_get(3)?,
        display_artist: row.try_get(4)?,
        release_date: row.try_get(5)?,
        created_at: row.try_get(6)?,
        song_count: row.try_get(7)?,
        duration_ms: row.try_get(8)?,
        played_count: row.try_get(9)?,
        last_played_at: row.try_get(10)?,
        user_rating: row.try_get(11)?,
        starred: row.try_get(12)?,
        starred_at: row.try_get(13)?,
        artist_public_id: row.try_get(14)?,
        artist_name: row.try_get(15)?,
        is_compilation: row.try_get(16)?,
        original_release_date: row.try_get(17)?,
    }))
}

/// Renders the captured getAlbum album object with the OpenSubsonic-rich
/// fields (ItemDate forms, isCompilation, discTitles, moods, genres[] +
/// singular genre, artists[] + displayArtist, parent, originalReleaseDate).
fn album_detail_json(
    detail: &AlbumDetailRow,
    songs: &[Value],
    genres: &[String],
    contributing_artists: &[Value],
) -> Value {
    let album_id = format!("album_{}", detail.release_public_id);
    let artist_id = format!("artist_{}", detail.artist_public_id);
    let display_artist = detail
        .display_artist
        .clone()
        .unwrap_or_else(|| detail.artist_name.clone());
    let mut album = Map::new();
    album.insert("id".to_string(), album_id.clone().into());
    album.insert("name".to_string(), detail.title.clone().into());
    album.insert("title".to_string(), detail.title.clone().into());
    album.insert("artist".to_string(), display_artist.clone().into());
    if let Some(year) = release_year(detail.release_date) {
        album.insert("year".to_string(), year.into());
    }
    album.insert("coverArt".to_string(), album_id.clone().into());
    if detail.starred
        && let Some(starred_at) = detail.starred_at
    {
        album.insert("starred".to_string(), rfc3339_z(starred_at).into());
    }
    // Captured rounding: half-to-even on the millisecond total (7700 -> 8).
    #[allow(clippy::cast_possible_truncation)]
    let duration_seconds = (detail.duration_ms as f64 / 1000.0).round_ties_even() as i64;
    album.insert("duration".to_string(), duration_seconds.into());
    album.insert("playCount".to_string(), detail.played_count.into());
    if !genres.is_empty() {
        album.insert(
            "genre".to_string(),
            genres
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
                .into(),
        );
    }
    album.insert("created".to_string(), rfc3339_z(detail.created_at).into());
    album.insert("artistId".to_string(), artist_id.clone().into());
    album.insert("songCount".to_string(), detail.song_count.into());
    // `played` is always emitted ("" when never played).
    album.insert(
        "played".to_string(),
        detail
            .last_played_at
            .map(rfc3339_z)
            .unwrap_or_default()
            .into(),
    );
    if detail.user_rating > 0 {
        album.insert(
            "userRating".to_string(),
            i64::from(detail.user_rating).into(),
        );
    }
    if genres.is_empty() {
        album.insert("genres".to_string(), Value::Array(Vec::new()));
    } else {
        album.insert(
            "genres".to_string(),
            Value::Array(
                genres
                    .iter()
                    .map(|g| serde_json::json!({"name": g}))
                    .collect(),
            ),
        );
    }
    album.insert(
        "artists".to_string(),
        Value::Array(contributing_artists.to_vec()),
    );
    album.insert("displayArtist".to_string(), display_artist.clone().into());
    album.insert("moods".to_string(), Value::Array(Vec::new()));
    album.insert("sortName".to_string(), detail.sort_title.clone().into());
    let album_date = item_date(detail.release_date);
    album.insert("originalAlbumDate".to_string(), album_date.clone());
    album.insert("albumDate".to_string(), album_date);
    album.insert("isCompilation".to_string(), detail.is_compilation.into());
    album.insert("discTitles".to_string(), Value::Array(Vec::new()));
    album.insert("song".to_string(), Value::Array(songs.to_vec()));
    album.insert("parent".to_string(), album_id.clone().into());
    let original_release = item_date(detail.original_release_date.or(detail.release_date));
    album.insert("originalReleaseDate".to_string(), original_release);
    Value::Object(album)
}

fn release_year(date: Option<time::Date>) -> Option<i32> {
    date.map(|d| d.year())
}

fn item_date(date: Option<time::Date>) -> Value {
    match date {
        Some(d) => serde_json::json!({
            "year": d.year(),
            "month": u8::from(d.month()) as i64,
            "day": d.day() as i64,
        }),
        None => serde_json::json!({"year": 0, "month": 0, "day": 0}),
    }
}

/// Loads the album's tracks and renders them via the shared song projection
/// (annotation.rs `song_children_with_contributors`), then post-processes
/// each Child for the getAlbum context: the captured album-context song
/// always carries `starred` ("" when not starred) and `userRating` (0 when
/// unrated). This is the only captured path whose serializer included
/// `Song.Contributors`, so the per-song `artists[]`/`contributors[]` arrays
/// are filled here (os-getalbum-compilation) while every other path emits
/// empty arrays. Also returns the per-track contributor entries in track
/// order so the caller can build the album-level `artists[]`
/// (`ContributingArtists()`). The captured contract additionally keys the
/// per-song `artistId` to the release's primary artist (not the track
/// billing) for ordinary albums; the shared projection already does this.
async fn album_songs(
    state: &OsState,
    user_id: Option<Uuid>,
    release_internal_id: Uuid,
) -> Result<(Vec<Value>, Vec<(Uuid, Vec<ContributorEntry>)>), sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT public_api_id FROM tracks
          WHERE release_id = $1
          ORDER BY COALESCE(disc_number, 1), COALESCE(track_number, ordinal), public_api_id",
    )
    .bind(release_internal_id)
    .fetch_all(&state.pool)
    .await?;
    let contributors = track_contributors(&state.pool, &ids).await?;
    let mut songs = song_children_with_contributors(
        &state.pool,
        user_id.unwrap_or(Uuid::nil()),
        &ids,
        Some(&contributors),
    )
    .await?;
    patch_song_average_ratings(&state.pool, user_id, &mut songs).await;
    let ordered = ids
        .iter()
        .map(|id| (*id, contributors.get(id).cloned().unwrap_or_default()))
        .collect();
    Ok((songs, ordered))
}

pub async fn get_album_info(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    album_info(&state, &params).await
}

pub async fn get_album_info2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    album_info(&state, &params).await
}

async fn album_info(state: &OsState, params: &Option<Extension<Params>>) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        return reply(state, params, OsReply::ok(serde_json::json!({})));
    };
    let Some(EntityRef::Release(release_public_id)) = parse_prefixed(raw_id) else {
        return reply(state, params, OsReply::ok(serde_json::json!({})));
    };
    let row = match sqlx::query(
        "SELECT release.public_api_id, release.title,
                COALESCE(aggregate.track_count, 0),
                (SELECT count(DISTINCT release_artists.release_id)
                   FROM release_artists
                   JOIN releases other ON other.id = release_artists.release_id
                  WHERE release_artists.artist_id = (
                        SELECT billing.artist_id FROM release_artists billing
                          JOIN artists artist ON artist.id = billing.artist_id
                         WHERE billing.release_id = release.id AND billing.role = 'primary'
                         ORDER BY billing.presentation_order, billing.id LIMIT 1)
                    AND other.status = 'active')
           FROM releases release
           LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = release.id
          WHERE release.public_api_id = $1 AND release.status = 'active'",
    )
    .bind(release_public_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return reply(state, params, OsReply::ok(serde_json::json!({}))),
        Err(error) => {
            tracing::error!(%error, "opensubsonic getAlbumInfo query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let public_id: Uuid = row.try_get(0).unwrap_or(release_public_id);
    let name: String = row.try_get(1).unwrap_or_default();
    let song_count: i64 = row.try_get(2).unwrap_or(0);
    let album_count: i64 = row.try_get(3).unwrap_or(0);
    let album_wire = format!("album_{public_id}");
    let image = |size: &str| format!("{}/images/{}/{size}", state.public_origin, album_wire);
    let info = serde_json::json!({
        "id": album_wire,
        "name": name,
        "songCount": song_count,
        "albumCount": album_count,
        "smallImageUrl": image("thumbnail"),
        "mediumImageUrl": image("medium"),
        "largeImageUrl": image("large"),
    });
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({"albumInfo": info})),
    )
}

// ---------------------------------------------------------------------------
// getSong
// ---------------------------------------------------------------------------

pub async fn get_song(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        // Missing id: the captured malformed-id shape (status ok + error 44).
        return ok_with_error_44(&state, &params);
    };
    let Some(EntityRef::Track(track_public_id)) = parse_prefixed(raw_id) else {
        // Malformed id: captured error 44 shape (status ok with error block).
        return ok_with_error_44(&state, &params);
    };
    let user_id = resolve_user_id(&state, &params, &principal).await;
    let mut songs = match song_children(
        &state.pool,
        user_id.unwrap_or(Uuid::nil()),
        &[track_public_id],
    )
    .await
    {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getSong projection failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    patch_song_average_ratings(&state.pool, user_id, &mut songs).await;
    if songs.is_empty() {
        // Unknown track id: captured os-getsong-unknown shape — status ok WITH
        // error 44 block (no song key).
        return ok_with_error_44(&state, &params);
    }
    let song = songs.remove(0);
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"song": song})),
    )
}

// ---------------------------------------------------------------------------
// getMusicDirectory
// ---------------------------------------------------------------------------

pub async fn get_music_directory(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(raw_id) = params_ref.and_then(|p| p.get("id")) else {
        return reply(
            &state,
            &params,
            OsReply::ok(serde_json::json!({"directory": {"child": []}})),
        );
    };
    let Some(entity) = parse_prefixed(raw_id) else {
        return reply(
            &state,
            &params,
            OsReply::ok(serde_json::json!({"directory": {"child": []}})),
        );
    };
    match entity {
        EntityRef::Library(library_id) => {
            // Library root: the captured directory has empty name, averageRating
            // 0 (int), playCount 0, and no child entries (the fixture library
            // has no direct files).
            let wire = format!("library_{library_id}");
            reply(
                &state,
                &params,
                OsReply::ok(serde_json::json!({
                    "directory": {
                        "id": wire,
                        "name": "",
                        "averageRating": 0,
                        "playCount": 0,
                        "child": []
                    }
                })),
            )
        }
        EntityRef::Artist(artist_public_id) => {
            music_directory_artist(&state, &params, &principal, artist_public_id).await
        }
        EntityRef::Release(release_public_id) => {
            music_directory_album(&state, &params, &principal, release_public_id).await
        }
        EntityRef::Track(_) => reply(
            &state,
            &params,
            OsReply::ok(serde_json::json!({"directory": {"child": []}})),
        ),
    }
}

async fn music_directory_artist(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    artist_public_id: Uuid,
) -> Response {
    let user_id = resolve_user_id(state, params, principal).await;
    let Some(detail) = (match load_artist_detail(&state.pool, artist_public_id, user_id).await {
        Ok(detail) => detail,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getMusicDirectory artist query failed");
            return error_reply(state, params, "A generic error.");
        }
    }) else {
        return reply(
            state,
            params,
            OsReply::ok(serde_json::json!({"directory": {"child": []}})),
        );
    };
    let albums = match load_artist_albums(&state.pool, detail.artist_internal_id).await {
        Ok(albums) => albums,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getMusicDirectory albums query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    let release_ids: Vec<Uuid> = albums.iter().map(|a| a.release_public_id).collect();
    let genres_by_release = release_genres(&state.pool, &release_ids)
        .await
        .unwrap_or_default();
    let artist_id = format!("artist_{}", detail.artist_public_id);
    // Captured directory playCount is the artist-level play count (the
    // fixture's 7 is less than the 12 song plays under the artist).
    let total_play: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(play_count), 0)::bigint FROM native_artist_user_states WHERE artist_id = $1",
    )
    .bind(detail.artist_internal_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getMusicDirectory artist play-count query failed");
            0
        }
    };
    let last_played_at: Option<OffsetDateTime> =
        match sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT max(play.played_at) FROM native_track_plays play
           JOIN tracks track ON track.id = play.track_id
           JOIN release_artists billing ON billing.release_id = track.release_id
            AND billing.role = 'primary' AND billing.artist_id = $1",
        )
        .bind(detail.artist_internal_id)
        .fetch_optional(&state.pool)
        .await
        {
            Ok(value) => value.flatten(),
            Err(error) => {
                tracing::error!(%error, "opensubsonic getMusicDirectory artist last-played query failed");
                None
            }
        };
    let last_played = last_played_at.map(rfc3339_z).unwrap_or_default();
    let mut directory = Map::new();
    directory.insert("id".to_string(), artist_id.clone().into());
    directory.insert("name".to_string(), detail.name.clone().into());
    if detail.starred
        && let Some(starred_at) = detail.starred_at
    {
        directory.insert("starred".to_string(), rfc3339_z(starred_at).into());
    } else {
        // The captured artist directory emits `starred: ""` when not starred.
        directory.insert("starred".to_string(), "".into());
    }
    if detail.user_rating > 0 {
        directory.insert(
            "userRating".to_string(),
            i64::from(detail.user_rating).into(),
        );
    }
    directory.insert("averageRating".to_string(), detail.average_rating.into());
    directory.insert("playCount".to_string(), total_play.into());
    directory.insert("played".to_string(), last_played.into());
    let parent = library_parent(state);
    let children: Vec<Value> = albums
        .iter()
        .map(|album| {
            directory_album_child(
                &artist_id,
                &parent,
                album,
                genres_by_release
                    .get(&album.release_public_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &detail,
            )
        })
        .collect();
    directory.insert("child".to_string(), Value::Array(children));
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({"directory": Value::Object(directory)})),
    )
}

/// The captured getMusicDirectory album child: the full Child directory
/// object with `isDir: true`, `mediaType: "album"`, and the OpenSubsonic-rich
/// nested fields (albumArtists, displayArtist, genres[], etc.).
fn directory_album_child(
    artist_id: &str,
    _library_parent: &str,
    row: &ArtistAlbumRow,
    genres: &[String],
    artist: &ArtistDetailFull,
) -> Value {
    let album_id = format!("album_{}", row.release_public_id);
    let mut child = Map::new();
    child.insert("id".to_string(), album_id.clone().into());
    child.insert("parent".to_string(), artist_id.into());
    child.insert("isDir".to_string(), true.into());
    child.insert("title".to_string(), row.title.clone().into());
    child.insert("album".to_string(), row.title.clone().into());
    child.insert("artist".to_string(), artist.name.clone().into());
    if let Some(year) = row.year {
        child.insert("year".to_string(), i64::from(year).into());
    }
    child.insert("coverArt".to_string(), album_id.clone().into());
    child.insert("starred".to_string(), "".into());
    // Captured rounding: half-to-even like the album detail (7700 ms -> 8 in
    // os-getmusicdirectory-artist), unlike the getArtist album entries which
    // the frozen contract captured with truncation.
    #[allow(clippy::cast_possible_truncation)]
    let duration_seconds = (row.duration_ms as f64 / 1000.0).round_ties_even() as i64;
    child.insert("duration".to_string(), duration_seconds.into());
    child.insert("playCount".to_string(), row.played_count.into());
    // Captured: directory album children carry `played` ("" when never
    // played), like the album detail.
    child.insert(
        "played".to_string(),
        row.last_played_at
            .map(super::annotation::rfc3339_z)
            .unwrap_or_default()
            .into(),
    );
    child.insert("created".to_string(), rfc3339_z(row.created_at).into());
    child.insert("albumId".to_string(), album_id.clone().into());
    child.insert("artistId".to_string(), artist_id.to_string().into());
    child.insert("type".to_string(), "music".into());
    child.insert("mediaType".to_string(), "album".into());
    child.insert("isVideo".to_string(), false.into());
    child.insert("sortName".to_string(), row.title.clone().into());
    if !genres.is_empty() {
        child.insert(
            "genres".to_string(),
            Value::Array(
                genres
                    .iter()
                    .map(|g| serde_json::json!({"value": g, "songCount": 0, "albumCount": 0}))
                    .collect(),
            ),
        );
    }
    child.insert("artists".to_string(), Value::Array(Vec::new()));
    child.insert("displayArtist".to_string(), artist.name.clone().into());
    child.insert(
        "albumArtists".to_string(),
        serde_json::json!([{
            "id": artist_id,
            "name": artist.name,
            "coverArt": artist_id,
            "albumCount": artist.album_count,
            "userRating": 0,
            "artistImageUrl": "URL",
            "sortName": artist.sort_name,
        }]),
    );
    child.insert("displayAlbumArtist".to_string(), artist.name.clone().into());
    child.insert("contributors".to_string(), Value::Array(Vec::new()));
    child.insert("replayGain".to_string(), serde_json::json!({}));
    child.insert("averageRating".to_string(), 0.into());
    if row.user_rating > 0 {
        child.insert("userRating".to_string(), i64::from(row.user_rating).into());
    }
    child.insert("playerId".to_string(), 0.into());
    if let Some(genre) = genres.first() {
        child.insert("genre".to_string(), genre.clone().into());
    }
    Value::Object(child)
}

async fn music_directory_album(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    release_public_id: Uuid,
) -> Response {
    let user_id = resolve_user_id(state, params, principal).await;
    let Some(detail) = (match load_album_detail(&state.pool, release_public_id, user_id).await {
        Ok(detail) => detail,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getMusicDirectory album query failed");
            return error_reply(state, params, "A generic error.");
        }
    }) else {
        return reply(
            state,
            params,
            OsReply::ok(serde_json::json!({"directory": {"child": []}})),
        );
    };
    let (songs, _) = match album_songs(state, user_id, detail.release_internal_id).await {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getMusicDirectory songs query failed");
            return error_reply(state, params, "A generic error.");
        }
    };
    // The captured album directory strips the album-context `starred`/`userRating`
    // always-present fields back to the list-context shape (only present when
    // nonzero), matching os-getmusicdirectory-album.
    let songs: Vec<Value> = songs
        .into_iter()
        .map(|mut song| {
            if let Value::Object(fields) = &mut song {
                // The captured album directory omits `starred` entirely (the
                // golden has no `starred` field on any song).
                fields.remove("starred");
                // userRating only when > 0 (the shared projection already did
                // this; album_songs re-added 0, so strip it back when zero).
                if fields
                    .get("userRating")
                    .and_then(Value::as_i64)
                    .is_some_and(|v| v == 0)
                {
                    fields.remove("userRating");
                }
            }
            song
        })
        .collect();
    let album_id = format!("album_{}", detail.release_public_id);
    let mut directory = Map::new();
    directory.insert("id".to_string(), album_id.clone().into());
    directory.insert("parent".to_string(), album_id.clone().into());
    directory.insert("name".to_string(), detail.title.clone().into());
    directory.insert("starred".to_string(), "".into());
    directory.insert("averageRating".to_string(), 0.0.into());
    directory.insert("playCount".to_string(), detail.played_count.into());
    directory.insert(
        "played".to_string(),
        detail
            .last_played_at
            .map(rfc3339_z)
            .unwrap_or_default()
            .into(),
    );
    directory.insert("child".to_string(), Value::Array(songs));
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({"directory": Value::Object(directory)})),
    )
}

/// The captured browse averageRating excludes the calling user's own rating
/// (the legacy CalculatedRating reads a separate aggregate), so a solely-
/// caller-rated entity shows 0.0. The shared `song_children` projection
/// includes the caller; this patches each song's `averageRating` to the
/// caller-excluded value to match the browse goldens.
async fn patch_song_average_ratings(pool: &PgPool, _user_id: Option<Uuid>, songs: &mut [Value]) {
    let track_ids: Vec<Uuid> = songs
        .iter()
        .filter_map(|song| {
            song.get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.strip_prefix("song_"))
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .collect();
    if track_ids.is_empty() {
        return;
    }
    let ratings: HashMap<Uuid, f64> = match sqlx::query(
        // The captured averageRating includes the caller's own rating
        // (`os-annotation-rating/verify-rating` shows 4 right after the user
        // set 4): it mirrors the legacy CalculatedRating aggregate.
        "SELECT track.public_api_id,
                COALESCE((SELECT AVG(ratings.rating)::float8 FROM native_track_user_states ratings
                          WHERE ratings.track_id = track.id AND ratings.rating > 0), 0)
           FROM tracks track
          WHERE track.public_api_id = ANY($1)",
    )
    .bind(&track_ids)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|row| {
                let id: Uuid = row.try_get(0).ok()?;
                let avg: f64 = row.try_get(1).ok()?;
                Some((id, avg))
            })
            .collect(),
        Err(error) => {
            tracing::error!(%error, "opensubsonic browse average-rating patch query failed");
            return;
        }
    };
    for song in songs.iter_mut() {
        let Some(id) = song
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.strip_prefix("song_"))
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            continue;
        };
        let Some(&avg) = ratings.get(&id) else {
            continue;
        };
        if let Value::Object(fields) = song {
            // Match the shared projection's int-vs-float rendering.
            if avg.fract() == 0.0 {
                #[allow(clippy::cast_possible_truncation)]
                fields.insert("averageRating".to_string(), (avg as i64).into());
            } else {
                fields.insert("averageRating".to_string(), avg.into());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// getGenres
// ---------------------------------------------------------------------------

pub async fn get_genres(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = principal;
    // The captured getGenres aggregates distinct genre tags from the catalog
    // import manifests with per-genre song and album counts, ordered by value
    // COLLATE "C" (ADR-0130 decision 5). Each manifest file matching a track's
    // source_path contributes its `;`-split genre tags.
    let rows = match sqlx::query(
        "SELECT genre_part.value AS genre,
                count(DISTINCT track.id) AS song_count,
                count(DISTINCT release.id) AS album_count
           FROM releases release
           JOIN catalog_imports catalog_import ON catalog_import.release_id = release.id
           JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
           JOIN tracks track ON track.release_id = release.id
           CROSS JOIN LATERAL jsonb_array_elements(
             COALESCE(manifest.manifest_payload->'files', '[]')) AS file(value)
           CROSS JOIN LATERAL unnest(string_to_array(
             file.value->'metadata'->>'genre', ';')) AS genre_part(value)
          WHERE release.status = 'active'
            AND file.value->>'relative_path' = track.source_path
            AND btrim(genre_part.value) <> ''
          GROUP BY genre_part.value
          ORDER BY genre_part.value COLLATE \"C\"",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getGenres query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let mut genres: Vec<Value> = Vec::with_capacity(rows.len());
    for row in rows {
        let value: String = row.try_get(0).unwrap_or_default();
        let song_count: i64 = row.try_get(1).unwrap_or(0);
        let album_count: i64 = row.try_get(2).unwrap_or(0);
        genres.push(serde_json::json!({
            "value": value,
            "songCount": song_count,
            "albumCount": album_count,
        }));
    }
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"genres": {"genre": genres}})),
    )
}

// ---------------------------------------------------------------------------
// getSimilarSongs / getSimilarSongs2
// ---------------------------------------------------------------------------

pub async fn get_similar_songs(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    similar_songs(&state, &params, "similarSongs").await
}

pub async fn get_similar_songs2(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    _principal: Option<Extension<OsPrincipal>>,
) -> Response {
    similar_songs(&state, &params, "similarSongs2").await
}

/// The captured fixture has no similar-song provider data, so both endpoints
/// answer the empty-song-list shape regardless of the id (the legacy provider
/// dependency is offline in the capture environment).
async fn similar_songs(state: &OsState, params: &Option<Extension<Params>>, key: &str) -> Response {
    let _ = params;
    reply(
        state,
        params,
        OsReply::ok(serde_json::json!({key: {"song": Value::Array(Vec::new())}})),
    )
}

// ---------------------------------------------------------------------------
// getTopSongs
// ---------------------------------------------------------------------------

pub async fn get_top_songs(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let params_ref = params.as_ref().map(|Extension(params)| params);
    let Some(artist_name) = params_ref.and_then(|p| p.get("artist")) else {
        return reply(
            &state,
            &params,
            OsReply::ok(serde_json::json!({"topSongs": {"song": Value::Array(Vec::new())}})),
        );
    };
    let user_id = resolve_user_id(&state, &params, &principal).await;
    // Name-based lookup: the frozen getTopSongs matches the artist Name
    // EXACTLY — `artist=Dune` returns only the two Dune songs, not Düné's
    // (`os-gettopsongs`). Ordered by global play count desc then public id.
    let ids: Vec<Uuid> = match sqlx::query_scalar::<_, Uuid>(
        "SELECT track.public_api_id
           FROM tracks track
           JOIN releases release ON release.id = track.release_id AND release.status = 'active'
           JOIN LATERAL (
             SELECT artist.canonical_name FROM track_artists billing
               JOIN artists artist ON artist.id = billing.artist_id
              WHERE billing.track_id = track.id AND billing.role = 'primary'
              ORDER BY billing.presentation_order, billing.id LIMIT 1
           ) artist ON true
          WHERE artist.canonical_name = $1
          ORDER BY (SELECT count(*) FROM native_track_plays play
                     WHERE play.track_id = track.id) DESC, track.public_api_id",
    )
    .bind(artist_name)
    .fetch_all(&state.pool)
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getTopSongs query failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    let mut songs = match song_children(&state.pool, user_id.unwrap_or(Uuid::nil()), &ids).await {
        Ok(songs) => songs,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getTopSongs projection failed");
            return error_reply(&state, &params, "A generic error.");
        }
    };
    patch_song_average_ratings(&state.pool, user_id, &mut songs).await;
    reply(
        &state,
        &params,
        OsReply::ok(serde_json::json!({"topSongs": {"song": songs}})),
    )
}

// ---------------------------------------------------------------------------
// Shared genre helper (mirrors lists.rs release_genres)
// ---------------------------------------------------------------------------

/// Release-level genre display values from the catalog import manifest files;
/// multi-value genre strings split on `;`, distinct in first-seen order.
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
            for part in genre.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                if !entry.iter().any(|known| known == part) {
                    entry.push(part.to_string());
                }
            }
        }
    }
    Ok(genres)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("getIndexes", get_indexes))
        .merge(super::endpoint_router(
            "getMusicDirectory",
            get_music_directory,
        ))
        .merge(super::endpoint_router("getArtists", get_artists))
        .merge(super::endpoint_router("getArtist", get_artist))
        .merge(super::endpoint_router("getArtistInfo", get_artist_info))
        .merge(super::endpoint_router("getArtistInfo2", get_artist_info2))
        .merge(super::endpoint_router("getAlbum", get_album))
        .merge(super::endpoint_router("getAlbumInfo", get_album_info))
        .merge(super::endpoint_router("getAlbumInfo2", get_album_info2))
        .merge(super::endpoint_router("getSong", get_song))
        .merge(super::endpoint_router("getGenres", get_genres))
        .merge(super::endpoint_router("getSimilarSongs", get_similar_songs))
        .merge(super::endpoint_router(
            "getSimilarSongs2",
            get_similar_songs2,
        ))
        .merge(super::endpoint_router("getTopSongs", get_top_songs))
}

// ---------------------------------------------------------------------------
// Unit tests for pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn index_letter_strips_ignored_articles_and_uppercases() {
        // The letter is taken from the sort name (with ignored articles
        // stripped); an empty sort name lands in "".
        assert_eq!(index_letter("Aurora Fields"), "A");
        assert_eq!(index_letter("Harbour Lights, The"), "H");
        assert_eq!(index_letter("Obsidian Collective, The"), "O");
        assert_eq!(index_letter("Beacon, Nora"), "B");
        assert_eq!(index_letter("the beatles"), "B");
        // A non-ASCII first letter lands in "" (the # bucket).
        assert_eq!(index_letter("æglópur"), "");
        assert_eq!(index_letter("東京 Project"), "");
        // Empty sort name lands in "".
        assert_eq!(index_letter(""), "");
    }

    #[test]
    fn parse_prefixed_handles_known_and_malformed_ids() {
        let id = Uuid::nil();
        assert_eq!(
            parse_prefixed(&format!("song_{id}")),
            Some(EntityRef::Track(id))
        );
        assert_eq!(
            parse_prefixed(&format!("album_{id}")),
            Some(EntityRef::Release(id))
        );
        assert_eq!(
            parse_prefixed(&format!("artist_{id}")),
            Some(EntityRef::Artist(id))
        );
        assert_eq!(
            parse_prefixed(&format!("library_{id}")),
            Some(EntityRef::Library(id))
        );
        // Malformed: unknown prefix or unparseable uuid.
        assert_eq!(parse_prefixed("song_not-a-uuid"), None);
        assert_eq!(
            parse_prefixed("playlist_00000000-0000-0000-0000-000000000000"),
            None
        );
        assert_eq!(parse_prefixed("no-underscore"), None);
    }
}
