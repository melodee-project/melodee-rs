//! Annotation endpoint group: star, unstar, setRating, scrobble
//! (PRD-API-OS-005; golden evidence `compatibility/opensubsonic/golden/annotation`).
//!
//! Captured behaviors reproduced here:
//! - star/unstar accept repeated `id` plus `albumId`/`artistId`; a malformed
//!   prefixed id answers HTTP 200 with error 44 "Invalid API key.".
//! - setRating takes `id` + `rating` 0-5 (0 clears) and persists to the
//!   shared `native_*_user_states` tables.
//! - scrobble with `submission` absent/true records an idempotent play;
//!   `submission=false` updates now-playing. A repeated `time` list whose
//!   length differs from the `id` list answers with the captured status-ok +
//!   error code 0 "Wrong number of timestamps." envelope quirk.
//!
//! This module also owns the shared OpenSubsonic Child song object used by
//! the bookmarks and play queue groups.

use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use melodee_persistence::{
    NativeScrobbleResult, NativeUserStateMutation, NativeUserStateStore, NativeUserStateStoreError,
};

use super::{
    OsState,
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

/// OpenSubsonic prefixed public id (`song_`/`album_`/`artist_` + UUID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityRef {
    Track(Uuid),
    Release(Uuid),
    Artist(Uuid),
}

/// Parses one prefixed public id; `None` is the captured malformed case.
pub(crate) fn parse_prefixed_id(value: &str) -> Option<EntityRef> {
    let (prefix, raw) = value.split_once('_')?;
    let id = Uuid::parse_str(raw).ok()?;
    match prefix {
        "song" => Some(EntityRef::Track(id)),
        "album" => Some(EntityRef::Release(id)),
        "artist" => Some(EntityRef::Artist(id)),
        _ => None,
    }
}

/// Captured timestamp rendering: RFC 3339 seconds with a literal `Z`.
pub(crate) fn rfc3339_z(value: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.year(),
        value.month() as u8,
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(state: &OsState, params: &Option<Extension<Params>>, reply: OsReply) -> Response {
    reply.respond(&format_of(params), &state.server_version, StatusCode::OK)
}

pub(crate) fn error_reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    code: i64,
    message: &str,
) -> Response {
    reply(state, params, OsReply::error(code, message))
}

/// The captured malformed-id failure (error 44 "Invalid API key.").
fn malformed_id(state: &OsState, params: &Option<Extension<Params>>) -> Response {
    error_reply(state, params, 44, "Invalid API key.")
}

/// Endpoints in this group act on per-user state; without a principal (the
/// captured bypass synthesizes no user) there is no captured behavior, so the
/// request fails closed with the shared auth error envelope.
pub(crate) fn require_principal(principal: &Option<Extension<OsPrincipal>>) -> Option<OsPrincipal> {
    principal
        .as_ref()
        .map(|Extension(principal)| principal.clone())
}

/// Early return: extract the principal or answer the captured error 40
/// envelope. Paths are fully qualified so sibling modules can invoke it.
macro_rules! authenticated_user {
    ($state:expr, $params:expr, $principal:expr) => {
        match $crate::opensubsonic::annotation::require_principal($principal) {
            Some(user) => user,
            None => {
                return $crate::opensubsonic::annotation::error_reply(
                    $state,
                    $params,
                    40,
                    "Wrong username or password.",
                );
            }
        }
    };
}
pub(crate) use authenticated_user;

fn store_failure(
    state: &OsState,
    params: &Option<Extension<Params>>,
    error: &NativeUserStateStoreError,
) -> Response {
    match error {
        NativeUserStateStoreError::NotFound => {
            error_reply(state, params, 70, "The requested data was not found.")
        }
        other => {
            tracing::error!(error = %other, "opensubsonic annotation persistence failed");
            error_reply(state, params, 0, "A generic error.")
        }
    }
}

async fn mutate_user_state(
    state: &OsState,
    params: &Option<Extension<Params>>,
    user_id: Uuid,
    target: EntityRef,
    mutation: NativeUserStateMutation,
) -> Option<Response> {
    let now = OffsetDateTime::now_utc();
    let result = match target {
        EntityRef::Track(id) => {
            NativeUserStateStore::mutate_track(&state.pool, user_id, id, mutation, now).await
        }
        EntityRef::Release(id) => {
            NativeUserStateStore::mutate_release(&state.pool, user_id, id, mutation, now).await
        }
        EntityRef::Artist(id) => {
            NativeUserStateStore::mutate_artist(&state.pool, user_id, id, mutation, now).await
        }
    };
    result
        .err()
        .map(|error| store_failure(state, params, &error))
}

/// star/unstar share one captured flow: every `id`, `albumId`, and `artistId`
/// value (repeats preserved) is parsed and applied in submission order.
async fn star_impl(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    starred: bool,
) -> Response {
    let user = authenticated_user!(state, params, principal);
    let Some(Extension(params_ref)) = params.as_ref() else {
        return reply(state, params, OsReply::ok(serde_json::json!({})));
    };
    let mut raw_ids: Vec<&str> = Vec::new();
    for key in ["id", "albumid", "artistid"] {
        raw_ids.extend(params_ref.get_all(key).iter().map(String::as_str));
    }
    for raw in raw_ids {
        let Some(target) = parse_prefixed_id(raw) else {
            return malformed_id(state, params);
        };
        if let Some(response) = mutate_user_state(
            state,
            params,
            user.user_id,
            target,
            NativeUserStateMutation::Starred(starred),
        )
        .await
        {
            return response;
        }
    }
    reply(state, params, OsReply::ok(serde_json::json!({})))
}

pub async fn star(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    star_impl(&state, &params, &principal, true).await
}

pub async fn unstar(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    star_impl(&state, &params, &principal, false).await
}

pub async fn set_rating(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let (Some(raw_id), Some(rating)) = (params_ref.get("id"), params_ref.get_i64("rating")) else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    if !(0..=5).contains(&rating) {
        return error_reply(&state, &params, 10, "Rating must be between 0 and 5.");
    }
    let Some(target) = parse_prefixed_id(raw_id) else {
        return malformed_id(&state, &params);
    };
    // i16 range is proven by the 0..=5 check above.
    #[allow(clippy::cast_possible_truncation)]
    let mutation = NativeUserStateMutation::Rating(rating as i16);
    if let Some(response) = mutate_user_state(&state, &params, user.user_id, target, mutation).await
    {
        return response;
    }
    reply(&state, &params, OsReply::ok(serde_json::json!({})))
}

/// The captured scrobble quirk: a `time` list whose length differs from the
/// `id` list answers with status "ok" AND an error object (code 0). The
/// generic envelope cannot express this, so the envelope is written directly
/// in all three negotiated formats.
fn scrobble_time_mismatch(state: &OsState, params: &Option<Extension<Params>>) -> Response {
    const CODE: i64 = 0;
    const MESSAGE: &str = "Wrong number of timestamps.";
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

fn millis_to_timestamp(millis: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000).ok()
}

/// Deterministic idempotency key for one scrobble submission:
/// `native_track_plays.client_event_id` is UNIQUE(user_id, client_event_id),
/// so a repeated submission with the same song and time adopts durably.
fn scrobble_event_id(track_public_id: Uuid, millis: i64) -> String {
    format!("opensubsonic-scrobble:{track_public_id}:{millis}")
}

pub async fn scrobble(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let user = authenticated_user!(&state, &params, &principal);
    let Some(Extension(params_ref)) = params.as_ref() else {
        return reply(&state, &params, OsReply::ok(serde_json::json!({})));
    };
    let ids = params_ref.get_all("id");
    let raw_times = params_ref.get_all("time");
    if ids.is_empty() {
        return reply(&state, &params, OsReply::ok(serde_json::json!({})));
    }
    let mut times: Vec<i64> = Vec::with_capacity(raw_times.len());
    for raw in raw_times {
        match raw.parse::<i64>() {
            Ok(value) => times.push(value),
            Err(_) => return scrobble_time_mismatch(&state, &params),
        }
    }
    if !times.is_empty() && times.len() != ids.len() {
        return scrobble_time_mismatch(&state, &params);
    }
    let submission = params_ref.get_bool("submission").unwrap_or(true);
    let now = OffsetDateTime::now_utc();
    for (index, raw_id) in ids.iter().enumerate() {
        let Some(EntityRef::Track(track_public_id)) = parse_prefixed_id(raw_id) else {
            return malformed_id(&state, &params);
        };
        let millis = times.get(index).copied().unwrap_or_else(|| {
            // now in whole milliseconds; overflow of i64 ms is not reachable
            // from unix time for any realistic clock.
            #[allow(clippy::cast_possible_truncation)]
            let millis = now.unix_timestamp_nanos() / 1_000_000;
            millis as i64
        });
        let Some(played_at) = millis_to_timestamp(millis) else {
            return error_reply(&state, &params, 0, "A generic error.");
        };
        let event_id = scrobble_event_id(track_public_id, millis);
        let result = if submission {
            match NativeUserStateStore::record_scrobble(
                &state.pool,
                user.user_id,
                track_public_id,
                &event_id,
                played_at,
                now,
                None,
            )
            .await
            {
                Ok(NativeScrobbleResult::Applied) => {
                    // fire-and-record Last.fm dispatch; never blocks
                    // or fails the write path, and an adopted idempotent
                    // replay must not double-scrobble Last.fm.
                    crate::lastfm::LastfmDispatch::new(
                        state.pool.clone(),
                        state.lastfm_api_base_url.as_deref(),
                    )
                    .spawn_scrobble(user.user_id, track_public_id);
                    Ok(())
                }
                Ok(NativeScrobbleResult::Adopted) => Ok(()),
                Err(error) => Err(error),
            }
        } else {
            let result = NativeUserStateStore::record_now_playing(
                &state.pool,
                user.user_id,
                track_public_id,
                &event_id,
                played_at,
                now,
            )
            .await;
            if result.is_ok() {
                crate::lastfm::LastfmDispatch::new(
                    state.pool.clone(),
                    state.lastfm_api_base_url.as_deref(),
                )
                .spawn_now_playing(user.user_id, track_public_id);
            }
            result
        };
        match result {
            Ok(()) => {}
            Err(NativeUserStateStoreError::Conflict) => {
                // Same idempotency key with different facts: the captured
                // server accepted repeats silently; the durable record wins.
                tracing::warn!(%event_id, "conflicting opensubsonic scrobble replay ignored");
            }
            Err(error) => return store_failure(&state, &params, &error),
        }
    }
    reply(&state, &params, OsReply::ok(serde_json::json!({})))
}

/// One row of the Child projection query.
struct ChildRow {
    track_public_id: Uuid,
    title: String,
    sort_title: String,
    duration_ms: i64,
    number: i32,
    disc_number: i32,
    created_at: OffsetDateTime,
    source_path: Option<String>,
    release_public_id: Uuid,
    release_title: String,
    year: Option<i32>,
    byte_size: i64,
    mime_type: String,
    bit_rate: i32,
    sample_rate_hz: Option<i32>,
    track_artist_public_id: Uuid,
    track_artist_name: String,
    release_artist_public_id: Uuid,
    release_artist_name: String,
    release_artist_sort_name: String,
    release_artist_album_count: i64,
    /// None when the user has no state row for the track (the legacy JSON
    /// writer omitted `starred`/`userRating` entirely in that case).
    starred: Option<bool>,
    rating: Option<i16>,
    play_count: i64,
    tempo: Option<f64>,
    average_rating: f64,
}

const CHILD_SELECT: &str = "SELECT track.public_api_id, track.title, track.sort_title,
    COALESCE(track.duration_ms, media.duration_ms, 0),
    CASE WHEN COALESCE(track.disc_number, 1) > 1
      THEN COALESCE(track.disc_number, 1) * 100 + COALESCE(track.track_number, track.ordinal)
      ELSE COALESCE(track.track_number, track.ordinal) END,
    COALESCE(track.disc_number, 1), track.created_at, track.source_path,
    release.public_api_id, release.title,
    extract(year FROM release.release_date)::integer,
    media.byte_size, media.mime_type,
    COALESCE((media.bitrate_bps / 1000)::integer, 0), media.sample_rate_hz,
    track_artist.public_api_id, track_artist.canonical_name,
    release_artist.public_api_id, release_artist.canonical_name, release_artist.sort_name,
    COALESCE(release_artist_aggregate.release_count, 0),
    state.starred,
    state.rating::smallint,
    (SELECT count(*) FROM native_track_plays plays WHERE plays.track_id = track.id),
    features.tempo,
    COALESCE((SELECT AVG(ratings.rating)::float8 FROM native_track_user_states ratings
      WHERE ratings.track_id = track.id AND ratings.rating > 0), 0)
  FROM tracks track
  JOIN releases release ON release.id = track.release_id AND release.status = 'active'
  JOIN media_assets media ON media.id = track.playable_media_asset_id
  JOIN LATERAL (
    SELECT artist.* FROM track_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.track_id = track.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) track_artist ON true
  JOIN LATERAL (
    SELECT artist.* FROM release_artists billing JOIN artists artist ON artist.id = billing.artist_id
    WHERE billing.release_id = release.id AND billing.role = 'primary'
    ORDER BY billing.presentation_order, billing.id LIMIT 1
  ) release_artist ON true
  LEFT JOIN artist_catalog_aggregates release_artist_aggregate
    ON release_artist_aggregate.artist_id = release_artist.id
  LEFT JOIN native_track_user_states state
    ON state.track_id = track.id AND state.user_id = $1
  LEFT JOIN native_track_audio_features features ON features.track_id = track.id
  WHERE track.public_api_id = ANY($2)";

/// Genre display values use ADR-0165's indexed serving projection. Audio bit
/// depth remains immutable manifest evidence and is read only for the bounded
/// set of selected Tracks.
const CHILD_MANIFEST_SELECT: &str = "SELECT DISTINCT ON (track.public_api_id)
    track.public_api_id,
    genre.value,
    (file.value->'audio'->>'bit_depth')::integer
  FROM tracks track
  LEFT JOIN LATERAL (
    SELECT string_agg(term.match_value, ';' ORDER BY term.source_order) AS value
    FROM catalog_genre_terms term
    WHERE term.track_id=track.id AND term.surface='opensubsonic'
  ) genre ON true
  JOIN catalog_imports catalog_import ON catalog_import.release_id = track.release_id
  JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
  CROSS JOIN LATERAL jsonb_array_elements(
    COALESCE(manifest.manifest_payload->'files', '[]')) file(value)
  WHERE track.public_api_id = ANY($1)
    AND file.value->>'relative_path' = track.source_path
  ORDER BY track.public_api_id, catalog_import.imported_at DESC";

fn suffix_from(source_path: Option<&str>) -> String {
    source_path
        .and_then(|path| path.rsplit_once('.'))
        .filter(|(_, extension)| !extension.is_empty() && !extension.contains('/'))
        .map_or_else(String::new, |(_, extension)| format!(".{extension}"))
}

/// OpenSubsonic exposes the legacy library-relative path: the release
/// directory and file, without the canonical Artist directory that the Rust
/// migration stores in `tracks.source_path`. Keep the migration authority
/// unchanged; this is only the compatibility adapter's wire projection.
fn open_subsonic_path(source_path: Option<&str>) -> String {
    let Some(path) = source_path else {
        return String::new();
    };
    let mut components = path.split('/');
    let Some(_artist_directory) = components.next() else {
        return String::new();
    };
    let Some(release_directory) = components.next() else {
        return path.to_string();
    };
    let remainder = components.collect::<Vec<_>>().join("/");
    if remainder.is_empty() {
        path.to_string()
    } else {
        format!("{release_directory}/{remainder}")
    }
}

/// Legacy `StringExtensions.ToNormalizedString` for contributor pseudo-ids:
/// upper-case, then keep only alphanumeric characters
/// ("The Phase0 Strings" -> "THEPHASE0STRINGS").
fn normalized_contributor_name(name: &str) -> String {
    name.chars()
        .flat_map(char::to_uppercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Converts the canonical lowercase migration role into the legacy
/// OpenSubsonic enum spelling. PostgreSQL stores the normalized value so
/// catalog identity remains stable; the compatibility adapter owns wire-case
/// restoration.
fn legacy_contributor_role(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "primaryartist" | "primary_artist" => "PrimaryArtist".to_string(),
        "featuredartist" | "featured_artist" => "FeaturedArtist".to_string(),
        "guestartist" | "guest_artist" => "GuestArtist".to_string(),
        "performer" => "Performer".to_string(),
        "bandmember" | "band_member" => "BandMember".to_string(),
        "vocalist" => "Vocalist".to_string(),
        "instrumentalist" => "Instrumentalist".to_string(),
        "composer" => "Composer".to_string(),
        "lyricist" => "Lyricist".to_string(),
        "producer" => "Producer".to_string(),
        "remixer" => "Remixer".to_string(),
        "engineer" => "Engineer".to_string(),
        "other" => "Other".to_string(),
        _ => value.to_string(),
    }
}

/// Subject of a captured contribution: either a resolved Artist identity or a
/// name-only observation (legacy `ContributorName` with no Artist).
#[derive(Debug, Clone)]
pub(crate) enum ContributorSubject {
    Artist {
        public_id: Uuid,
        name: String,
        sort_name: String,
    },
    NameOnly {
        name: String,
    },
}

/// One captured contributor credit on a Track, in presentation order.
#[derive(Debug, Clone)]
pub(crate) struct ContributorEntry {
    pub role: String,
    pub sub_role: Option<String>,
    /// Legacy `ContributorTypeValue == ContributorType.Performer`: performer
    /// credits also surface in the Child/Album `artists[]` arrays.
    pub is_performance: bool,
    pub subject: ContributorSubject,
}

impl ContributorEntry {
    /// Renders the legacy `ArtistID3` for this contributor. Name-only
    /// contributors are pseudo-artists keyed `contributor_<NORMALIZED>` whose
    /// `coverArt` is caller-scoped: the legacy Child serializer used the
    /// song's cover-art id, while the album-level `ContributingArtists()`
    /// used the pseudo-id itself.
    fn artist_id3(&self, name_only_cover_art: &str) -> serde_json::Value {
        match &self.subject {
            ContributorSubject::Artist {
                public_id,
                name,
                sort_name,
            } => serde_json::json!({
                "id": format!("artist_{public_id}"),
                "name": name,
                "coverArt": format!("artist_{public_id}"),
                // The legacy contributor serializer did not hydrate the
                // catalog aggregate used by standalone Artist DTOs.
                "albumCount": 0,
                // Captured: these paths never pass a UserArtist, so the
                // rating is the `userArtist?.Rating ?? 0` fallback.
                "userRating": 0,
                // Literal placeholder preserved from the legacy serializer.
                "artistImageUrl": "URL",
                "sortName": sort_name,
            }),
            ContributorSubject::NameOnly { name } => {
                let id = format!("contributor_{}", normalized_contributor_name(name));
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "coverArt": name_only_cover_art,
                    "albumCount": 0,
                    "userRating": 0,
                })
            }
        }
    }

    /// Album-level `ContributingArtists()` rendering: name-only contributors
    /// use their own pseudo-id as `coverArt` there.
    pub(crate) fn album_scoped_artist_id3(&self) -> serde_json::Value {
        match &self.subject {
            ContributorSubject::NameOnly { name } => {
                let id = format!("contributor_{}", normalized_contributor_name(name));
                self.artist_id3(&id)
            }
            ContributorSubject::Artist { .. } => self.artist_id3(""),
        }
    }
}

/// Loads contribution credits for the given Track public ids, keyed by Track
/// public id, each list in presentation order. Artist-linked credits come
/// from `contributions`; name-only credits come from `observed_artists` rows
/// carrying a contribution role in `source_payload` (name-only contributors
/// have no Artist identity, and `contributions.artist_id` is NOT NULL). Only
/// the captured getAlbum path renders these: the legacy playlist/starred
/// serializers never included `Song.Contributors`, so their Child objects
/// carry empty arrays.
pub(crate) async fn track_contributors(
    pool: &PgPool,
    track_public_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<ContributorEntry>>, sqlx::Error> {
    let mut map: std::collections::HashMap<Uuid, Vec<(i32, ContributorEntry)>> =
        std::collections::HashMap::new();
    if track_public_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let linked_rows = sqlx::query(
        "SELECT track.public_api_id, contribution.role, contribution.role_detail,
                contribution.is_performance, contribution.presentation_order,
                artist.public_api_id, artist.canonical_name, artist.sort_name
           FROM contributions contribution
           JOIN tracks track ON track.id = contribution.track_id
           JOIN artists artist ON artist.id = contribution.artist_id
          WHERE track.public_api_id = ANY($1)
          ORDER BY contribution.presentation_order, contribution.id",
    )
    .bind(track_public_ids)
    .fetch_all(pool)
    .await?;
    for row in linked_rows {
        let track_public_id: Uuid = row.try_get(0)?;
        let entry = ContributorEntry {
            role: legacy_contributor_role(row.try_get::<String, _>(1)?.as_str()),
            sub_role: row.try_get(2)?,
            is_performance: row.try_get(3)?,
            subject: ContributorSubject::Artist {
                public_id: row.try_get(5)?,
                name: row.try_get(6)?,
                sort_name: row.try_get(7)?,
            },
        };
        let ordinal: i32 = row.try_get(4)?;
        map.entry(track_public_id)
            .or_default()
            .push((ordinal, entry));
    }
    let observed_rows = sqlx::query(
        "SELECT track.public_api_id, observed.display_value,
                observed.source_payload->>'role', observed.source_payload->>'subRole',
                COALESCE((observed.source_payload->>'contributorType')::integer = 1, false),
                observed.source_ordinal
           FROM observed_artists observed
           JOIN tracks track ON track.id = observed.track_id
          WHERE track.public_api_id = ANY($1) AND observed.scope = 'track'
            AND observed.source_payload ? 'role'
          ORDER BY observed.source_ordinal, observed.id",
    )
    .bind(track_public_ids)
    .fetch_all(pool)
    .await?;
    for row in observed_rows {
        let track_public_id: Uuid = row.try_get(0)?;
        let name: String = row.try_get(1)?;
        let entry = ContributorEntry {
            role: legacy_contributor_role(
                row.try_get::<Option<String>, _>(2)?
                    .as_deref()
                    .unwrap_or_default(),
            ),
            sub_role: row.try_get(3)?,
            is_performance: row.try_get(4)?,
            subject: ContributorSubject::NameOnly { name },
        };
        let ordinal: i32 = row.try_get(5)?;
        map.entry(track_public_id)
            .or_default()
            .push((ordinal, entry));
    }
    Ok(map
        .into_iter()
        .map(|(track_id, mut entries)| {
            entries.sort_by_key(|(ordinal, _)| *ordinal);
            (
                track_id,
                entries.into_iter().map(|(_, entry)| entry).collect(),
            )
        })
        .collect())
}

fn child_json(
    row: &ChildRow,
    genres: &[String],
    bit_depth: Option<i32>,
    contributors: &[ContributorEntry],
) -> serde_json::Value {
    let song_id = format!("song_{}", row.track_public_id);
    let album_id = format!("album_{}", row.release_public_id);
    let artist_id = format!("artist_{}", row.track_artist_public_id);
    let album_artist_id = format!("artist_{}", row.release_artist_public_id);
    let mut child = serde_json::Map::new();
    child.insert("id".to_string(), song_id.clone().into());
    child.insert("parent".to_string(), album_id.clone().into());
    child.insert("isDir".to_string(), false.into());
    child.insert("title".to_string(), row.title.clone().into());
    child.insert("album".to_string(), row.release_title.clone().into());
    child.insert("artist".to_string(), row.track_artist_name.clone().into());
    child.insert("song".to_string(), row.number.into());
    if let Some(year) = row.year {
        child.insert("year".to_string(), year.into());
    }
    child.insert("coverArt".to_string(), song_id.into());
    child.insert("size".to_string(), row.byte_size.into());
    child.insert("contentType".to_string(), row.mime_type.clone().into());
    child.insert(
        "suffix".to_string(),
        suffix_from(row.source_path.as_deref()).into(),
    );
    // Captured rule: `starred` is present only when the user row exists and
    // is starred, and its value is the legacy `Nullable<DateTime>.ToString()`
    // artifact — the fixture rows carried no LastUpdatedAt, so the frozen
    // goldens show "" (runtime-starred timestamps are masked volatile in the
    // after-star goldens).
    if row.starred == Some(true) {
        child.insert("starred".to_string(), "".into());
    }
    // Captured rounding: legacy `Duration.ToSeconds()` rounds half-to-even
    // (2500 ms -> 2, 2700 ms -> 3, 1800 ms -> 2).
    #[allow(clippy::cast_possible_truncation)]
    let duration_seconds = (row.duration_ms as f64 / 1000.0).round_ties_even() as i64;
    child.insert("duration".to_string(), duration_seconds.into());
    child.insert("bitRate".to_string(), row.bit_rate.into());
    if let Some(bit_depth) = bit_depth {
        child.insert("bitDepth".to_string(), bit_depth.into());
    }
    if let Some(sample_rate) = row.sample_rate_hz {
        child.insert("samplingRate".to_string(), sample_rate.into());
    }
    child.insert(
        "path".to_string(),
        open_subsonic_path(row.source_path.as_deref()).into(),
    );
    child.insert("playCount".to_string(), row.play_count.into());
    child.insert("discNumber".to_string(), row.disc_number.into());
    child.insert("created".to_string(), rfc3339_z(row.created_at).into());
    child.insert("albumId".to_string(), album_id.into());
    child.insert("artistId".to_string(), artist_id.into());
    child.insert("type".to_string(), "music".into());
    child.insert("mediaType".to_string(), "song".into());
    child.insert("isVideo".to_string(), false.into());
    // The legacy BPM column defaulted to 0 when analysis had not run.
    #[allow(clippy::cast_possible_truncation)]
    let bpm = row.tempo.map_or(0, |tempo| tempo.round() as i64);
    child.insert("bpm".to_string(), bpm.into());
    child.insert("sortName".to_string(), row.sort_title.clone().into());
    if !genres.is_empty() {
        child.insert(
            "genres".to_string(),
            serde_json::Value::Array(
                genres
                    .iter()
                    .map(|genre| {
                        serde_json::json!({"value": genre, "songCount": 0, "albumCount": 0})
                    })
                    .collect(),
            ),
        );
    }
    // Captured rule: `artists[]` holds performer-type contributors only;
    // name-only contributors render as `contributor_<NORMALIZED>` pseudo
    // artists whose coverArt is the song id (legacy `song.ToCoverArtId()`).
    let song_cover_art = format!("song_{}", row.track_public_id);
    child.insert(
        "artists".to_string(),
        serde_json::Value::Array(
            contributors
                .iter()
                .filter(|contributor| contributor.is_performance)
                .map(|contributor| contributor.artist_id3(&song_cover_art))
                .collect(),
        ),
    );
    child.insert(
        "displayArtist".to_string(),
        row.track_artist_name.clone().into(),
    );
    child.insert(
        "albumArtists".to_string(),
        serde_json::json!([{
            "id": album_artist_id,
            "name": row.release_artist_name,
            "coverArt": format!("artist_{}", row.release_artist_public_id),
            "albumCount": row.release_artist_album_count,
            // Captured: the child albumArtists entry always carries 0 — the
            // legacy child serializer never mapped the artist user rating.
            "userRating": 0,
            // Literal placeholder preserved from the legacy serializer.
            "artistImageUrl": "URL",
            "sortName": row.release_artist_sort_name,
        }]),
    );
    child.insert(
        "displayAlbumArtist".to_string(),
        row.release_artist_name.clone().into(),
    );
    child.insert(
        "contributors".to_string(),
        serde_json::Value::Array(
            contributors
                .iter()
                .map(|contributor| {
                    let mut entry = serde_json::json!({
                        "role": contributor.role,
                        "artistId3": contributor.artist_id3(&song_cover_art),
                    });
                    if let Some(sub_role) = &contributor.sub_role {
                        entry["subRole"] = sub_role.clone().into();
                    }
                    entry
                })
                .collect(),
        ),
    );
    child.insert("replayGain".to_string(), serde_json::json!({}));
    let average = row.average_rating;
    if average.fract() == 0.0 {
        #[allow(clippy::cast_possible_truncation)]
        child.insert("averageRating".to_string(), (average as i64).into());
    } else {
        child.insert("averageRating".to_string(), average.into());
    }
    // Captured rule: `userRating` is present whenever a user-state row exists
    // (rating 0 included — os-getalbum-minimal's hated track), absent without
    // a row.
    if let Some(rating) = row.rating {
        child.insert("userRating".to_string(), rating.into());
    }
    child.insert("playerId".to_string(), 0.into());
    child.insert("track".to_string(), row.number.into());
    if let Some(genre) = genres.first() {
        child.insert("genre".to_string(), genre.clone().into());
    }
    serde_json::Value::Object(child)
}

/// Builds the captured OpenSubsonic Child objects for the given Track public
/// ids, in request order. Tracks that no longer resolve are skipped, matching
/// the legacy list serializers that tolerated dangling references.
pub(crate) async fn song_children(
    pool: &PgPool,
    user_id: Uuid,
    track_public_ids: &[Uuid],
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    song_children_with_contributors(pool, user_id, track_public_ids, None).await
}

/// `song_children` variant used by the getAlbum detail path, the only
/// captured serializer that included `Song.Contributors`
/// (os-getalbum-compilation); every other legacy path left the navigation
/// property unloaded and emitted empty `artists`/`contributors` arrays.
pub(crate) async fn song_children_with_contributors(
    pool: &PgPool,
    user_id: Uuid,
    track_public_ids: &[Uuid],
    contributors: Option<&std::collections::HashMap<Uuid, Vec<ContributorEntry>>>,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    if track_public_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(CHILD_SELECT)
        .bind(user_id)
        .bind(track_public_ids)
        .fetch_all(pool)
        .await?;
    let mut children = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let child_row = ChildRow {
            track_public_id: row.try_get(0)?,
            title: row.try_get(1)?,
            sort_title: row.try_get(2)?,
            duration_ms: row.try_get(3)?,
            number: row.try_get(4)?,
            disc_number: row.try_get(5)?,
            created_at: row.try_get(6)?,
            source_path: row.try_get(7)?,
            release_public_id: row.try_get(8)?,
            release_title: row.try_get(9)?,
            year: row.try_get(10)?,
            byte_size: row.try_get(11)?,
            mime_type: row.try_get(12)?,
            bit_rate: row.try_get(13)?,
            sample_rate_hz: row.try_get(14)?,
            track_artist_public_id: row.try_get(15)?,
            track_artist_name: row.try_get(16)?,
            release_artist_public_id: row.try_get(17)?,
            release_artist_name: row.try_get(18)?,
            release_artist_sort_name: row.try_get(19)?,
            release_artist_album_count: row.try_get(20)?,
            starred: row.try_get(21)?,
            rating: row.try_get(22)?,
            play_count: row.try_get(23)?,
            tempo: row.try_get(24)?,
            average_rating: row.try_get(25)?,
        };
        children.insert(child_row.track_public_id, child_row);
    }
    let manifest_rows = sqlx::query(CHILD_MANIFEST_SELECT)
        .bind(track_public_ids)
        .fetch_all(pool)
        .await?;
    let mut manifest_by_track = std::collections::HashMap::with_capacity(manifest_rows.len());
    for row in manifest_rows {
        let public_id: Uuid = row.try_get(0)?;
        let genre: Option<String> = row.try_get(1)?;
        let bit_depth: Option<i32> = row.try_get(2)?;
        manifest_by_track.insert(public_id, (genre, bit_depth));
    }
    let mut out = Vec::with_capacity(track_public_ids.len());
    for public_id in track_public_ids {
        let Some(child_row) = children.remove(public_id) else {
            continue;
        };
        let (genre, bit_depth) = manifest_by_track.remove(public_id).unwrap_or((None, None));
        let genres: Vec<String> = genre
            .map(|value| {
                value
                    .split(';')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let empty: &[ContributorEntry] = &[];
        let track_contributors = contributors
            .and_then(|map| map.get(public_id))
            .map_or(empty, Vec::as_slice);
        out.push(child_json(
            &child_row,
            &genres,
            bit_depth,
            track_contributors,
        ));
    }
    Ok(out)
}

/// Resolves a Track public id to its internal id for the group tables that
/// key on internal ids (bookmarks, play queue).
pub(crate) async fn track_id_for_public_id(
    pool: &PgPool,
    track_public_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM tracks WHERE public_api_id = $1")
        .bind(track_public_id)
        .fetch_optional(pool)
        .await
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("star", star))
        .merge(super::endpoint_router("unstar", unstar))
        .merge(super::endpoint_router("setRating", set_rating))
        .merge(super::endpoint_router("scrobble", scrobble))
}

#[cfg(test)]
mod tests {
    use super::{legacy_contributor_role, open_subsonic_path};

    #[test]
    fn contributor_roles_restore_legacy_wire_casing_without_changing_unknown_values() {
        assert_eq!(legacy_contributor_role("featuredartist"), "FeaturedArtist");
        assert_eq!(legacy_contributor_role("performer"), "Performer");
        assert_eq!(legacy_contributor_role("featured_artist"), "FeaturedArtist");
        assert_eq!(legacy_contributor_role("future-role"), "future-role");
    }

    #[test]
    fn open_subsonic_path_projects_canonical_artist_prefix_away() {
        assert_eq!(
            open_subsonic_path(Some("Dune/Desert Rock Radio/02 - Caravan.wav")),
            "Desert Rock Radio/02 - Caravan.wav"
        );
        assert_eq!(
            open_subsonic_path(Some("Various Artists/Compilation/disc-01/01.wav")),
            "Compilation/disc-01/01.wav"
        );
    }

    #[test]
    fn open_subsonic_path_preserves_legacy_relative_paths() {
        assert_eq!(
            open_subsonic_path(Some("Desert Rock Radio/01 - Sandstone.wav")),
            "Desert Rock Radio/01 - Sandstone.wav"
        );
        assert_eq!(open_subsonic_path(None), "");
    }
}
