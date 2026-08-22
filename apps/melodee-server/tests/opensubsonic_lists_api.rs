//! OpenSubsonic lists/search compatibility evidence: getAlbumList,
//! getAlbumList2, getRandomSongs, getNowPlaying, getStarred, getStarred2,
//! getSongsByGenre, search2, search3, and the melodeeExtensions[1] endpoints.
//! Asserted behavior mirrors the frozen golden captures in
//! `compatibility/opensubsonic/golden/lists/`, including the captured quirks:
//! `type=recent` sorts by release date (not last played), search song entries
//! carry hardcoded placeholder fields (bitRate 0, suffix "mp3", path
//! "unknown", contentType "audio/mpeg"), the Dune/Düné normalized-name
//! collision matches both artists, the extension endpoints answer 302 without
//! embedding credentials in Location, and `artistImageUrl: "URL"` stays the
//! literal placeholder on ID3 artist entries.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{
    AuthStore, NativeUserImport, NativeUserStateMutation, NativeUserStateStore,
};
use melodee_server::opensubsonic::{MusicFolder, OsState};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const FIXTURE_SECRET: &str = "fixture-os-secret-NOT-A-REAL-SECRET";
const SERVER_VERSION: &str = "0.0.0-test";
const LIBRARY_WIRE_ID: &str = "library_c598e196-db6f-582e-b110-1726c138bd4f";

fn music_folders() -> Vec<std::sync::Arc<MusicFolder>> {
    vec![
        std::sync::Arc::new(MusicFolder {
            id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
            name: "Phase0 Fixture Library".to_string(),
        }),
        std::sync::Arc::new(MusicFolder {
            id: Uuid::parse_str("f63a6428-55d5-847b-3d09-3fa3b69b66ae").expect("folder id"),
            name: "Storage".to_string(),
        }),
    ]
}

async fn state(db: &common::TestDb) -> OsState {
    let now = OffsetDateTime::now_utc();
    let roles = vec!["Administrator".to_string(), "HasStreamRole".to_string()];
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "phase0-fixture",
                normalized_username: "PHASE0-FIXTURE",
                email: "phase0-fixture@example.test",
                normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("fixture hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
                is_admin: true,
                is_editor: true,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: Some("en-US"),
                preferred_theme: Some("dark"),
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import user");
    let internal_id = fixture_user_id(db).await;
    sqlx::query("INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) VALUES ($1, $2, now(), now())")
        .bind(internal_id)
        .bind(FIXTURE_SECRET)
        .execute(&db.pool)
        .await
        .expect("seed secret");
    OsState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        server_version: SERVER_VERSION.to_string(),
        public_origin: "http://melodee.test".to_string(),
        base_url_host: Some("capture-fixture.example.test".to_string()),
        music_folders: music_folders(),
        podcasts_enabled: true,
        downloads_enabled: true,
        jukebox_enabled: false,
        direct_play: None,
        transcoding: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
    }
}

async fn fixture_user_id(db: &common::TestDb) -> Uuid {
    sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'")
        .fetch_one(&db.pool)
        .await
        .expect("user id")
}

fn token_for(salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(FIXTURE_SECRET.as_bytes());
    hasher.update(salt.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds an authenticated request path with per-call salt.
fn authed(path: &str) -> String {
    let salt = path.len().to_string();
    format!("{path}&u=phase0-fixture&t={}&s={salt}", token_for(&salt))
}

async fn respond(state: OsState, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = melodee_server::opensubsonic::configured_router(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

fn enforced_get(path: &str) -> Request<Body> {
    // Mirrors the enforced-auth capture: a synthetic public client
    // address defeats the localhost bypass without touching the server.
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
    request
}

/// The captured baseUrl bypass: Host equals the configured base URL host.
fn bypass_get(path: &str) -> Request<Body> {
    let mut request = enforced_get(path);
    request.headers_mut().insert(
        header::HOST,
        "capture-fixture.example.test".parse().expect("host"),
    );
    request
}

fn envelope(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(bytes).expect("json body")["subsonic-response"]
        .clone()
}

fn at(text: &str) -> OffsetDateTime {
    OffsetDateTime::parse(text, &Rfc3339).expect("timestamp")
}

/// One seeded release's (track, release, artist) public ids by track title.
async fn catalog_ids(db: &common::TestDb, track_title: &str) -> (Uuid, Uuid, Uuid) {
    sqlx::query_as(
        "SELECT track.public_api_id, release.public_api_id, artist.public_api_id
         FROM tracks track
         JOIN releases release ON release.id = track.release_id
         JOIN release_artists billing ON billing.release_id = release.id AND billing.role = 'primary'
         JOIN artists artist ON artist.id = billing.artist_id
         WHERE track.title = $1",
    )
    .bind(track_title)
    .fetch_one(&db.pool)
    .await
    .expect("seeded catalog ids")
}

/// The Child `artistId` is the track-level billing artist, which the catalog
/// resolver keeps distinct from the release-level identity.
async fn track_artist_id(db: &common::TestDb, track_public_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT artist.public_api_id FROM track_artists billing
         JOIN artists artist ON artist.id = billing.artist_id
         WHERE billing.track_id = (SELECT id FROM tracks WHERE public_api_id = $1)
           AND billing.role = 'primary'
         ORDER BY billing.presentation_order, billing.id LIMIT 1",
    )
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("track artist")
}

async fn set_release_created(db: &common::TestDb, release_public_id: Uuid, created: &str) {
    sqlx::query("UPDATE releases SET created_at = $2 WHERE public_api_id = $1")
        .bind(release_public_id)
        .bind(at(created))
        .execute(&db.pool)
        .await
        .expect("set release created");
}

async fn record_play(db: &common::TestDb, track_public_id: Uuid, played_at: &str) {
    let user_id = fixture_user_id(db).await;
    sqlx::query(
        "INSERT INTO native_track_plays (id, user_id, track_id, client_event_id, played_at, submitted_at)
         SELECT $1, $2, id, $3, $4, $4 FROM tracks WHERE public_api_id = $5",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(format!("event-{}", Uuid::now_v7()))
    .bind(at(played_at))
    .bind(track_public_id)
    .execute(&db.pool)
    .await
    .expect("record play");
}

#[allow(clippy::too_many_arguments)]
async fn seed_release(
    db: &common::TestDb,
    ordinal: u32,
    artist: &str,
    title: &str,
    track: &str,
    date: &str,
    created: &str,
    genre: Option<&str>,
) -> (Uuid, Uuid, Uuid) {
    let presentation = CatalogFixturePresentation {
        release_title: title,
        release_artist: artist,
        track_title: track,
        track_artist: artist,
        release_date: date,
    };
    match genre {
        // Genre fidelity is under test, so the tag must be present in the
        // immutable published manifest (manifest v3 forbids post-hoc edits).
        Some(genre) => {
            phase5_common::seed_cataloged_release_with_genre(db, ordinal, presentation, genre)
                .await;
        }
        None => {
            phase5_common::seed_cataloged_release(db, ordinal, presentation).await;
        }
    }
    let ids = catalog_ids(db, track).await;
    set_release_created(db, ids.1, created).await;
    ids
}

// ---------------------------------------------------------------------------
// getAlbumList / getAlbumList2
// ---------------------------------------------------------------------------

async fn album_list_body(state: &OsState, path: &str) -> (HeaderMap, serde_json::Value) {
    let (status, headers, body) = respond(state.clone(), enforced_get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK, "{path}");
    (headers, envelope(&body))
}

#[tokio::test]
async fn album_list2_newest_matches_captured_shape_and_total() {
    let db = common::TestDb::create("os_lists_albumlist2_newest").await;
    let state = state(&db).await;
    let (track_b, release_b, artist_b) = seed_release(
        &db,
        7801,
        "Dune",
        "Desert Rock Radio",
        "Sandstone",
        "2019-03-04",
        "2026-01-04T00:00:00Z",
        Some("Rock"),
    )
    .await;
    let (_track_a, release_a, artist_a) = seed_release(
        &db,
        7802,
        "Aurora Fields",
        "Northern Lights",
        "Solar Wind",
        "2021-05-06",
        "2026-01-02T00:00:00Z",
        Some("Ambient;Electronic"),
    )
    .await;
    let (_track_c, release_c, _artist_c) = seed_release(
        &db,
        7803,
        "Cassette Ghost",
        "Untitled Demos",
        "Demo One",
        "1987-07-08",
        "2026-01-20T00:00:00Z",
        None,
    )
    .await;
    let user_id = fixture_user_id(&db).await;
    // Two plays and a rating on the newest-first release prove playedCount
    // and userRating flow into the list shape.
    record_play(&db, track_b, "2026-08-01T10:00:00Z").await;
    record_play(&db, track_b, "2026-08-02T10:00:00Z").await;
    NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        release_b,
        NativeUserStateMutation::Rating(5),
        at("2026-08-03T00:00:00Z"),
    )
    .await
    .expect("rate release");

    let (headers, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&v=1.16.1&c=phase0&type=newest&size=10",
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "3");
    let albums = body["albumList2"]["album"].as_array().expect("albums");
    let ids: Vec<&str> = albums
        .iter()
        .map(|album| album["id"].as_str().expect("id"))
        .collect();
    // newest = created desc with staggered fixture timestamps.
    assert_eq!(
        ids,
        [
            format!("album_{release_c}"),
            format!("album_{release_b}"),
            format!("album_{release_a}")
        ]
    );
    let album = albums
        .iter()
        .find(|album| album["id"] == format!("album_{release_b}"))
        .expect("desert rock entry");
    assert_eq!(
        *album,
        serde_json::json!({
                   "id": format!("album_{release_b}"),
                   "album": "Desert Rock Radio",
                   "title": "Desert Rock Radio",
                   "name": "Desert Rock Radio",
                   "coverArt": format!("album_{release_b}"),
                   "songCount": 1,
                   "created": "2026-01-04T00:00:00Z",
                   "duration": 180,
        // playedCount is the captured Album.PlayedCount class: user-state
        // sums; this fixture seeds only global play rows, so 0.
                   "playedCount": 0,
                   "artistId": format!("artist_{artist_b}"),
                   "artist": "Dune",
                   "year": 2019,
                   "userRating": 5,
                   "genres": ["Rock"],
                   "genre": "Rock",
                   "parent": LIBRARY_WIRE_ID,
               })
    );
    // Multi-value genres split in first-seen order; singular genre is first.
    let album = albums
        .iter()
        .find(|album| album["id"] == format!("album_{release_a}"))
        .expect("northern lights entry");
    assert_eq!(
        album["genres"],
        serde_json::json!(["Ambient", "Electronic"])
    );
    assert_eq!(album["genre"], "Ambient");
    assert_eq!(album["artistId"], format!("artist_{artist_a}"));
    assert_eq!(album["playedCount"], 0);
    assert_eq!(album["userRating"], 0);
    // The shared seeder always writes a genre tag ("Fixture" when the
    // test does not name one); the omit-when-empty path is unit-tested in the
    // lists module itself.
    let album = albums
        .iter()
        .find(|album| album["id"] == format!("album_{release_c}"))
        .expect("untitled entry");
    assert_eq!(album["genres"], serde_json::json!(["Fixture"]));
    assert_eq!(album["genre"], "Fixture");
    assert_eq!(album["year"], 1987);
}

#[tokio::test]
async fn album_list_type_sorts_match_the_captured_semantics() {
    let db = common::TestDb::create("os_lists_albumlist_types").await;
    let state = state(&db).await;
    // a: middle created, newest release date, most plays.
    let (track_a, release_a, _) = seed_release(
        &db,
        7811,
        "Beta Artist",
        "Beta Album",
        "Beta Track",
        "2024-01-01",
        "2026-01-10T00:00:00Z",
        Some("Rock"),
    )
    .await;
    // b: newest created, oldest release date.
    let (_t, _release_b, _) = seed_release(
        &db,
        7812,
        "Alpha Artist",
        "Gamma Album",
        "Gamma Track",
        "2018-01-01",
        "2026-01-15T00:00:00Z",
        Some("Jazz"),
    )
    .await;
    // c: oldest created, middle release date, starred.
    let (_t, release_c, _) = seed_release(
        &db,
        7813,
        "Gamma Artist",
        "Alpha Album",
        "Alpha Track",
        "2021-01-01",
        "2026-01-05T00:00:00Z",
        Some("Rock"),
    )
    .await;
    record_play(&db, track_a, "2026-08-01T10:00:00Z").await;
    let user_id = fixture_user_id(&db).await;
    // The captured frequent sort reads album-level play counts (the user
    // release state sum); give Beta Album the highest count.
    sqlx::query(
        "INSERT INTO native_release_user_states (user_id, release_id, starred, hated, rating, pinned, play_count, created_at, updated_at)
         VALUES ($1, $2, false, false, 0, false, 5, now(), now())",
    )
    .bind(user_id)
    .bind(release_a)
    .execute(&db.pool)
    .await
    .expect("play state");
    NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        release_c,
        NativeUserStateMutation::Starred(true),
        at("2026-08-07T00:00:00Z"),
    )
    .await
    .expect("star release");

    let order = |body: &serde_json::Value, key: &str| -> Vec<String> {
        body[key]["album"]
            .as_array()
            .expect("albums")
            .iter()
            .map(|album| album["name"].as_str().expect("name").to_string())
            .collect()
    };

    // Captured quirk: `recent` sorts by ReleaseDate, not last played.
    let (_, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=recent&size=10",
    )
    .await;
    assert_eq!(
        order(&body, "albumList2"),
        ["Beta Album", "Alpha Album", "Gamma Album"]
    );

    let (_, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=alphabeticalByName&size=10",
    )
    .await;
    assert_eq!(
        order(&body, "albumList2"),
        ["Alpha Album", "Beta Album", "Gamma Album"]
    );

    let (_, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=alphabeticalByArtist&size=10",
    )
    .await;
    assert_eq!(
        order(&body, "albumList2"),
        ["Gamma Album", "Beta Album", "Alpha Album"]
    );

    // frequent: most played first.
    let (_, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=frequent&size=10",
    )
    .await;
    assert_eq!(order(&body, "albumList2")[0], "Beta Album");

    // byYear filters on the release year (inclusive bounds).
    let (headers, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=byYear&fromYear=2019&toYear=2021&size=10",
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "1");
    assert_eq!(order(&body, "albumList2"), ["Alpha Album"]);

    // byGenre: exact genre match, total reflects the filter.
    let (headers, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=byGenre&genre=Rock&size=10",
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "2");
    let mut names = order(&body, "albumList2");
    names.sort();
    assert_eq!(names, ["Alpha Album", "Beta Album"]);

    // starred: only the calling user's starred releases.
    let (headers, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=starred&size=10",
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "1");
    assert_eq!(order(&body, "albumList2"), ["Alpha Album"]);
}

#[tokio::test]
async fn album_list_legacy_shape_adds_average_rating_and_paginates() {
    let db = common::TestDb::create("os_lists_albumlist_legacy").await;
    let state = state(&db).await;
    for (ordinal, title, track, created) in [
        (7821_u32, "First", "First Track", "2026-01-01T00:00:00Z"),
        (7822, "Second", "Second Track", "2026-01-02T00:00:00Z"),
        (7823, "Third", "Third Track", "2026-01-03T00:00:00Z"),
    ] {
        seed_release(
            &db,
            ordinal,
            "Legacy Artist",
            title,
            track,
            "2020-06-07",
            created,
            None,
        )
        .await;
    }

    // Legacy albumList entries equal albumList2 entries plus averageRating.
    let (_, legacy) =
        album_list_body(&state, "/rest/getAlbumList.view?f=json&type=newest&size=10").await;
    let (_, id3) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=newest&size=10",
    )
    .await;
    let legacy_albums = legacy["albumList"]["album"].as_array().expect("albums");
    let id3_albums = id3["albumList2"]["album"].as_array().expect("albums");
    assert_eq!(legacy_albums.len(), 3);
    for (legacy_album, id3_album) in legacy_albums.iter().zip(id3_albums) {
        assert_eq!(legacy_album["averageRating"], serde_json::json!(0.0));
        let mut stripped = legacy_album.clone();
        stripped.as_object_mut().unwrap().remove("averageRating");
        assert_eq!(&stripped, id3_album);
        assert!(id3_album.get("averageRating").is_none());
    }

    // size/offset page the list; x-total-count stays the unpaged total.
    let (headers, body) = album_list_body(
        &state,
        "/rest/getAlbumList2.view?f=json&type=newest&size=1&offset=1",
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "3");
    let albums = body["albumList2"]["album"].as_array().expect("albums");
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0]["name"], "Second");
}

#[tokio::test]
async fn album_list2_xml_uses_the_captured_container_names() {
    let db = common::TestDb::create("os_lists_albumlist_xml").await;
    let state = state(&db).await;
    let (_t, release, _a) = seed_release(
        &db,
        7831,
        "Xml Artist",
        "Xml Album",
        "Xml Track",
        "2020-01-02",
        "2026-01-01T00:00:00Z",
        Some("Rock"),
    )
    .await;
    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getAlbumList2.view?v=1.16.1&c=phase0&type=newest&size=10",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    let xml = String::from_utf8(body).unwrap();
    assert!(xml.contains("<albumList2>"), "{xml}");
    assert!(
        xml.contains(&format!("<album id=\"album_{release}\"")),
        "{xml}"
    );
    assert!(xml.contains("name=\"Xml Album\""), "{xml}");
}

// ---------------------------------------------------------------------------
// getRandomSongs / getNowPlaying
// ---------------------------------------------------------------------------

#[tokio::test]
async fn random_songs_envelope_and_size_bound_the_selection() {
    let db = common::TestDb::create("os_lists_random").await;
    let state = state(&db).await;
    for (ordinal, title, track) in [
        (7841_u32, "Random A", "Random A Track"),
        (7842, "Random B", "Random B Track"),
        (7843, "Random C", "Random C Track"),
    ] {
        seed_release(
            &db,
            ordinal,
            "Random Artist",
            title,
            track,
            "2020-01-01",
            "2026-01-01T00:00:00Z",
            Some("Rock"),
        )
        .await;
    }

    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getRandomSongs.view?f=json&v=1.16.1&c=phase0&size=2",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
    let songs = body_songs(&body, "randomSongs");
    assert_eq!(songs.len(), 2);
    // Selection is nondeterministic (masked in the golden); the Child shape
    // is the contract.
    for song in songs {
        assert!(song["id"].as_str().expect("id").starts_with("song_"));
        assert_eq!(song["mediaType"], "song");
        assert_eq!(song["type"], "music");
        assert_eq!(song["isVideo"], false);
        assert_eq!(song["replayGain"], serde_json::json!({}));
        assert_eq!(song["playerId"], 0);
        assert_eq!(
            song["genres"],
            serde_json::json!([{"value": "Rock", "songCount": 0, "albumCount": 0}])
        );
        assert_eq!(song["genre"], "Rock");
        assert_eq!(song["albumArtists"][0]["artistImageUrl"], "URL");
    }

    // Missing size binds to the legacy int default 0: an empty list.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getRandomSongs.view?f=json")),
    )
    .await;
    assert!(body_songs(&body, "randomSongs").is_empty());

    // Genre and year filters apply.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getRandomSongs.view?f=json&size=10&genre=NoSuchGenre",
        )),
    )
    .await;
    assert!(body_songs(&body, "randomSongs").is_empty());
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getRandomSongs.view?f=json&size=10&fromYear=2022&toYear=2023",
        )),
    )
    .await;
    assert!(body_songs(&body, "randomSongs").is_empty());
}

fn body_songs(body: &[u8], key: &str) -> Vec<serde_json::Value> {
    envelope(body)[key]["song"]
        .as_array()
        .expect("song array")
        .clone()
}

#[tokio::test]
async fn now_playing_is_empty_then_lists_the_active_entry() {
    let db = common::TestDb::create("os_lists_nowplaying").await;
    let state = state(&db).await;
    let (track, _release, _artist) = seed_release(
        &db,
        7851,
        "Now Playing Artist",
        "Now Playing Album",
        "Now Playing Track",
        "2020-01-01",
        "2026-01-01T00:00:00Z",
        None,
    )
    .await;

    // Captured empty shape (os-getnowplaying-empty).
    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getNowPlaying.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
    assert_eq!(
        envelope(&body)["nowPlaying"],
        serde_json::json!({"entry": []})
    );

    let user_id = fixture_user_id(&db).await;
    sqlx::query(
        "INSERT INTO native_now_playing (user_id, track_id, client_event_id, started_at, updated_at)
         VALUES ($1, (SELECT id FROM tracks WHERE public_api_id = $2), $3, $4, $4)",
    )
    .bind(user_id)
    .bind(track)
    .bind("np-event-1")
    .bind(at("2026-08-10T10:00:00Z"))
    .execute(&db.pool)
    .await
    .expect("seed now playing");

    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getNowPlaying.view?f=json")),
    )
    .await;
    let env = envelope(&body);
    let entries = env["nowPlaying"]["entry"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["id"], format!("song_{track}"));
    assert_eq!(entry["username"], "phase0-fixture");
    assert!(entry["minutesAgo"].as_i64().expect("minutesAgo") >= 0);
    assert_eq!(entry["title"], "Now Playing Track");
}

// ---------------------------------------------------------------------------
// getStarred / getStarred2
// ---------------------------------------------------------------------------

async fn seed_starred_fixture(db: &common::TestDb) -> (Uuid, Uuid, Uuid) {
    let (track, release, artist) = seed_release(
        db,
        7861,
        "Aurora Fields",
        "Northern Lights",
        "Polar Veil",
        "2021-03-15",
        "2026-01-02T00:00:00Z",
        Some("Ambient;Electronic"),
    )
    .await;
    let user_id = fixture_user_id(db).await;
    // Rating first, star second: the star's updated_at is the starred stamp.
    let rated = at("2026-08-06T12:00:00Z");
    let starred = at("2026-08-07T00:00:00Z");
    for mutation in [
        NativeUserStateMutation::Rating(5),
        NativeUserStateMutation::Starred(true),
    ] {
        let now = if mutation == NativeUserStateMutation::Starred(true) {
            starred
        } else {
            rated
        };
        NativeUserStateStore::mutate_track(&db.pool, user_id, track, mutation, now)
            .await
            .expect("track state");
        NativeUserStateStore::mutate_release(&db.pool, user_id, release, mutation, now)
            .await
            .expect("release state");
        NativeUserStateStore::mutate_artist(&db.pool, user_id, artist, mutation, now)
            .await
            .expect("artist state");
    }
    record_play(db, track, "2026-08-06T12:00:00Z").await;
    (track, release, artist)
}

#[tokio::test]
async fn starred2_matches_the_captured_id3_shapes() {
    let db = common::TestDb::create("os_lists_starred2").await;
    let state = state(&db).await;
    let (track, release, artist) = seed_starred_fixture(&db).await;

    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getStarred2.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
    let env = envelope(&body);
    let starred2 = env["starred2"].clone();

    let artists = starred2["artist"].as_array().expect("artists");
    assert_eq!(
        artists,
        &vec![serde_json::json!({
                   "id": format!("artist_{artist}"),
                   "name": "Aurora Fields",
                   "coverArt": format!("artist_{artist}"),
                   "albumCount": 1,
                   "userRating": 5,
        // Captured literal placeholder quirk.
                   "artistImageUrl": "URL",
                   "starred": "2026-08-07T00:00:00Z",
                   "sortName": "Aurora Fields",
               })]
    );

    let albums = starred2["album"].as_array().expect("albums");
    assert_eq!(
        albums,
        &vec![serde_json::json!({
                   "id": format!("album_{release}"),
                   "name": "Northern Lights",
                   "artist": "Aurora Fields",
                   "artistId": format!("artist_{artist}"),
                   "coverArt": format!("album_{release}"),
                   "songCount": 1,
                   "duration": 180,
        // playCount reads the captured album-level class (user-state
        // sums); this fixture seeds a global play row only, so 0.
                   "playCount": 0,
                   "created": "2026-01-02T00:00:00Z",
                   "starred": "2026-08-07T00:00:00Z",
                   "year": 2021,
                   "genres": ["Ambient", "Electronic"],
                   "genre": "Ambient",
               })]
    );

    let songs = starred2["song"].as_array().expect("songs");
    assert_eq!(songs.len(), 1);
    let song = &songs[0];
    assert_eq!(song["id"], format!("song_{track}"));
    assert_eq!(song["title"], "Polar Veil");
    // Song `starred` is the legacy Nullable<DateTime>.ToString artifact —
    // "" in the frozen goldens — while the album entry carries the real
    // StarredAt timestamp (different legacy serializers).
    assert_eq!(song["starred"], "");
    assert_eq!(song["userRating"], 5);
    // Song playCount is the global legacy Song.PlayedCount class (this
    // fixture seeds one global play row).
    assert_eq!(song["playCount"], 1);
    assert_eq!(
        song["genres"],
        serde_json::json!([
            {"value": "Ambient", "songCount": 0, "albumCount": 0},
            {"value": "Electronic", "songCount": 0, "albumCount": 0}
        ])
    );
    assert_eq!(song["albumArtists"][0]["artistImageUrl"], "URL");
}

#[tokio::test]
async fn starred_legacy_matches_the_captured_child_and_artist_shapes() {
    let db = common::TestDb::create("os_lists_starred_legacy").await;
    let state = state(&db).await;
    let (track, release, artist) = seed_starred_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getStarred.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let starred = env["starred"].clone();

    // Legacy Artist DTO: renderable image URL, decimal averageRating, both
    // userStarred and starred carry the timestamp.
    let artists = starred["artist"].as_array().expect("artists");
    assert_eq!(
        artists,
        &vec![serde_json::json!({
            "id": format!("artist_{artist}"),
            "name": "Aurora Fields",
            "albumCount": 1,
            "userRating": 5,
            "averageRating": 5.0,
            "coverArt": format!("artist_{artist}"),
            "artistImageUrl": format!("/images/{artist}/medium"),
            "userStarred": "2026-08-07T00:00:00Z",
            "starred": "2026-08-07T00:00:00Z",
        })]
    );

    // Legacy album is the full Child directory object.
    let albums = starred["album"].as_array().expect("albums");
    assert_eq!(
        albums,
        &vec![serde_json::json!({
                   "id": format!("album_{release}"),
                   "parent": format!("artist_{artist}"),
                   "isDir": true,
                   "title": "Northern Lights",
                   "album": "Northern Lights",
                   "artist": "Aurora Fields",
                   "year": 2021,
                   "coverArt": format!("album_{release}"),
        // v1 Child starred is the legacy Nullable<DateTime>.ToString
        // artifact — "" for fixture-seeded rows.
                   "starred": "",
                   "duration": 180,
                   "playCount": 0,
                   "played": "2026-08-06T12:00:00Z",
                   "created": "2026-01-02T00:00:00Z",
                   "albumId": format!("album_{release}"),
                   "artistId": format!("artist_{artist}"),
                   "type": "music",
                   "mediaType": "album",
                   "isVideo": false,
                   "sortName": "Northern Lights",
                   "genres": [
                       {"value": "Ambient", "songCount": 0, "albumCount": 0},
                       {"value": "Electronic", "songCount": 0, "albumCount": 0}
                   ],
                   "artists": [],
                   "displayArtist": "Aurora Fields",
                   "albumArtists": [{
                       "id": format!("artist_{artist}"),
                       "name": "Aurora Fields",
                       "coverArt": format!("artist_{artist}"),
                       "albumCount": 1,
                       "userRating": 0,
                       "artistImageUrl": "URL",
                       "sortName": "Aurora Fields"
                   }],
                   "displayAlbumArtist": "Aurora Fields",
                   "contributors": [],
                   "replayGain": {},
                   "averageRating": 0,
                   "userRating": 5,
                   "playerId": 0,
                   "genre": "Ambient",
               })]
    );

    let songs = starred["song"].as_array().expect("songs");
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0]["id"], format!("song_{track}"));
    // v1 Child song starred is the legacy Nullable<DateTime>.ToString
    // artifact — "" for fixture-seeded rows (frozen goldens).
    assert_eq!(songs[0]["starred"], "");
}

#[tokio::test]
async fn starred_under_bypass_resolves_u_but_synthesizes_no_user_without_it() {
    let db = common::TestDb::create("os_lists_starred_bypass").await;
    let state = state(&db).await;
    let (_track, _release, artist) = seed_starred_fixture(&db).await;

    // Captured golden behavior: the bypass resolves the `u` username, so the
    // user-specific lists are populated without credentials.
    let (status, _, body) = respond(
        state.clone(),
        bypass_get("/rest/getStarred2.view?f=json&u=phase0-fixture"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let starred2 = env["starred2"].clone();
    assert_eq!(starred2["artist"][0]["id"], format!("artist_{artist}"));

    // Without `u`, no user is synthesized (BlankUserInfo): empty lists.
    let (status, _, body) =
        respond(state.clone(), bypass_get("/rest/getStarred2.view?f=json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["starred2"],
        serde_json::json!({"artist": [], "album": [], "song": []})
    );
}

// ---------------------------------------------------------------------------
// getSongsByGenre
// ---------------------------------------------------------------------------

#[tokio::test]
async fn songs_by_genre_matches_ascii_and_unicode_genres_exactly() {
    let db = common::TestDb::create("os_lists_genre_songs").await;
    let state = state(&db).await;
    let (rock_track, _r1, rock_artist) = seed_release(
        &db,
        7871,
        "Dune",
        "Desert Rock Radio",
        "Sandstone",
        "2019-03-04",
        "2026-01-04T00:00:00Z",
        Some("Rock"),
    )
    .await;
    let (unicode_track, _r2, _unicode_artist) = seed_release(
        &db,
        7872,
        "Sæglópur 日本",
        "Ünïcode Rëlease 東京",
        "Neon 東京",
        "2024-11-12",
        "2026-01-16T00:00:00Z",
        Some("Post-Rock;環境音楽"),
    )
    .await;
    seed_release(
        &db,
        7873,
        "Cassette Ghost",
        "Untitled Demos",
        "Demo One",
        "1987-07-08",
        "2026-01-20T00:00:00Z",
        None,
    )
    .await;

    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getSongsByGenre.view?f=json&v=1.16.1&c=phase0&genre=Rock&count=10",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-total-count").unwrap(), "1");
    let songs = body_songs(&body, "songsByGenre");
    assert_eq!(songs.len(), 1);
    assert_eq!(
        songs[0],
        serde_json::json!({
            "id": format!("song_{rock_track}"),
            "parent": songs[0]["parent"],
            "isDir": false,
            "title": "Sandstone",
            "album": "Desert Rock Radio",
            "artist": "Dune",
            "song": 1,
            "year": 2019,
            "coverArt": format!("song_{rock_track}"),
            "size": songs[0]["size"],
            "contentType": songs[0]["contentType"],
            "suffix": ".flac",
            "duration": 180,
            "bitRate": songs[0]["bitRate"],
            "bitDepth": 24,
            "samplingRate": 48000,
            "path": "disc-01/7871 - Sandstone.flac",
            "playCount": 0,
            "discNumber": 1,
            "created": songs[0]["created"],
            "albumId": songs[0]["albumId"],
            "artistId": format!("artist_{}", track_artist_id(&db, rock_track).await),
            "type": "music",
            "mediaType": "song",
            "isVideo": false,
            "bpm": 0,
            "sortName": "Sandstone",
            "genres": [{"value": "Rock", "songCount": 0, "albumCount": 0}],
            "artists": [],
            "displayArtist": "Dune",
            "albumArtists": [{
                "id": format!("artist_{rock_artist}"),
                "name": "Dune",
                "coverArt": format!("artist_{rock_artist}"),
                "albumCount": 1,
                "userRating": 0,
                "artistImageUrl": "URL",
                "sortName": "Dune"
            }],
            "displayAlbumArtist": "Dune",
            "contributors": [],
            "replayGain": {},
            "averageRating": 0,
            "playerId": 0,
            "track": 1,
            "genre": "Rock",
        })
    );

    // Unicode genre (captured 環境音楽) matches exactly.
    let (_, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getSongsByGenre.view?f=json&genre=%E7%92%B0%E5%A2%83%E9%9F%B3%E6%A5%BD&count=10",
        )),
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "1");
    let songs = body_songs(&body, "songsByGenre");
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0]["id"], format!("song_{unicode_track}"));
    assert_eq!(
        songs[0]["artistId"],
        format!("artist_{}", track_artist_id(&db, unicode_track).await)
    );
    assert_eq!(
        songs[0]["genres"],
        serde_json::json!([
            {"value": "Post-Rock", "songCount": 0, "albumCount": 0},
            {"value": "環境音楽", "songCount": 0, "albumCount": 0}
        ])
    );

    // Unknown genre: the captured empty shape with total 0.
    let (_, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getSongsByGenre.view?f=json&genre=NoSuchGenre&count=10",
        )),
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
    assert_eq!(
        envelope(&body)["songsByGenre"],
        serde_json::json!({"song": []})
    );

    // Case sensitivity: legacy Contains is ordinal; "rock" does not match "Rock".
    let (_, headers, _) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getSongsByGenre.view?f=json&genre=rock&count=10",
        )),
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
}

// ---------------------------------------------------------------------------
// search2 / search3
// ---------------------------------------------------------------------------

async fn seed_search_fixture(db: &common::TestDb) -> (Uuid, Uuid, Uuid, Uuid) {
    // The captured normalized-name collision pair: Dune and Düné.
    let (song, album_dune, artist_dune) = seed_release(
        db,
        7881,
        "Dune",
        "Desert Rock Radio",
        "Sandstone",
        "2019-03-04",
        "2026-01-04T00:00:00Z",
        Some("Rock"),
    )
    .await;
    let (_s2, _a2, artist_dyune) = seed_release(
        db,
        7882,
        "Düné",
        "Dansk Sessions",
        "Dansk Track",
        "2020-09-10",
        "2026-01-06T00:00:00Z",
        Some("Pop"),
    )
    .await;
    (song, album_dune, artist_dune, artist_dyune)
}

#[tokio::test]
async fn search_matches_the_normalized_name_collision_pair() {
    let db = common::TestDb::create("os_lists_search_dune").await;
    let state = state(&db).await;
    let (_song, _album, artist_dune, artist_dyune) = seed_search_fixture(&db).await;

    for endpoint in ["search2", "search3"] {
        let key = if endpoint == "search2" {
            "searchResult2"
        } else {
            "searchResult3"
        };
        let (status, headers, body) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&query=Dune&artistCount=10&albumCount=10&songCount=10"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        assert_eq!(headers.get("x-total-count").unwrap(), "0", "{endpoint}");
        let env = envelope(&body);
        let result = env[&key].clone();
        // Both collision identities match one query (captured Dune/Düné).
        // The fixture resolves release-scope and track-scope artist
        // observations to separate identities, so each name appears twice;
        // the captured contract is that one folded query matches both names.
        let artists = result["artist"].as_array().expect("artists");
        let names: Vec<&str> = artists
            .iter()
            .map(|artist| artist["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, ["Dune", "Dune", "Düné", "Düné"], "{endpoint}");
        let ids: Vec<&str> = artists
            .iter()
            .map(|artist| artist["id"].as_str().expect("id"))
            .collect();
        assert!(
            ids.contains(&format!("artist_{artist_dune}").as_str()),
            "{endpoint}"
        );
        assert!(
            ids.contains(&format!("artist_{artist_dyune}").as_str()),
            "{endpoint}"
        );
        for artist in artists {
            let id = artist["id"].as_str().expect("id");
            assert_eq!(artist["coverArt"], id, "{endpoint}");
            assert_eq!(artist["albumCount"], 1, "{endpoint}");
            let mut keys: Vec<&String> = artist.as_object().expect("object").keys().collect();
            keys.sort();
            assert_eq!(keys, ["albumCount", "coverArt", "id", "name"], "{endpoint}");
        }
        // "Dune" appears in neither album nor song titles.
        assert_eq!(result["album"], serde_json::json!([]));
        assert_eq!(result["song"], serde_json::json!([]));
    }
}

#[tokio::test]
async fn search_song_entries_keep_the_captured_placeholder_fields() {
    let db = common::TestDb::create("os_lists_search_placeholders").await;
    let state = state(&db).await;
    let (song, album, artist, _dyune) = seed_search_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=Sandstone&artistCount=10&albumCount=10&songCount=10",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let result = env["searchResult3"].clone();
    assert_eq!(
        result["song"],
        serde_json::json!([{
                   "type": "music",
                   "isDir": false,
                   "id": format!("song_{song}"),
                   "parent": format!("album_{album}"),
                   "title": "Sandstone",
                   "album": "Desert Rock Radio",
                   "artist": "Dune",
                   "coverArt": format!("song_{song}"),
                   "created": result["song"][0]["created"],
                   "duration": 180,
                   "size": result["song"][0]["size"],
        // Hardcoded legacy placeholders (SongDataInfo carries none of
        // these), preserved from the capture.
                   "bitrate": 0,
                   "track": 1,
                   "year": 2019,
                   "genres": ["Rock"],
                   "suffix": "mp3",
                   "contentType": "audio/mpeg",
                   "path": "unknown",
                   "albumId": format!("album_{album}"),
                   "artistId": format!("artist_{artist}"),
                   "genre": "Rock",
               }])
    );
    assert_eq!(result["artist"], serde_json::json!([]));

    // Album title search returns the AlbumSearchResult shape.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=Desert&artistCount=10&albumCount=10&songCount=10",
        )),
    )
    .await;
    let env = envelope(&body);
    let result = env["searchResult3"].clone();
    assert_eq!(
        result["album"],
        serde_json::json!([{
            "id": format!("album_{album}"),
            "name": "Desert Rock Radio",
            "coverArt": format!("album_{album}"),
            "songCount": 1,
            "artist": "Dune",
            "artistId": format!("artist_{artist}"),
            "duration": 180,
            "created": result["album"][0]["created"],
            "genres": ["Rock"],
        }])
    );
}

#[tokio::test]
async fn search_empty_and_unicode_queries_match_the_captured_shapes() {
    let db = common::TestDb::create("os_lists_search_empty").await;
    let state = state(&db).await;
    seed_search_fixture(&db).await;

    // Nonexistent term: the captured empty-arrays shape.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=zzzznothing&artistCount=10&albumCount=10&songCount=10",
        )),
    )
    .await;
    assert_eq!(
        envelope(&body)["searchResult3"],
        serde_json::json!({"album": [], "song": [], "artist": []})
    );

    // CJK-only query (captured 東京): folds to nothing and matches nothing,
    // even though a seeded name contains the characters.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=%E6%9D%B1%E4%BA%AC&artistCount=10&albumCount=10&songCount=10",
        )),
    )
    .await;
    assert_eq!(
        envelope(&body)["searchResult3"],
        serde_json::json!({"album": [], "song": [], "artist": []})
    );

    // Blank query + a single raw count of 1: the captured statistics quirk
    // reports that entity's total in x-total-count.
    let (_, headers, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/search3.view?f=json&query=&albumCount=1")),
    )
    .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "2");
    assert_eq!(
        envelope(&body)["searchResult3"]["album"]
            .as_array()
            .expect("albums")
            .len(),
        1
    );

    // songCount=0 suppresses the song section entirely.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=Sandstone&artistCount=10&albumCount=10&songCount=0",
        )),
    )
    .await;
    assert_eq!(
        envelope(&body)["searchResult3"],
        serde_json::json!({"album": [], "song": [], "artist": []})
    );

    // The offset rounds down to a page boundary (legacy page = offset/count+1);
    // with the dual-identity fixture, offset 2 lands on the first Düné row.
    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/search3.view?f=json&query=Dune&artistCount=1&artistOffset=2&albumCount=0&songCount=0",
        )),
    )
    .await;
    let env = envelope(&body);
    let artists = env["searchResult3"]["artist"].as_array().expect("artists");
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0]["name"], "Düné");
}

// ---------------------------------------------------------------------------
// melodeeExtensions[1] search endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extension_search_endpoints_answer_the_captured_302_without_leaking_credentials() {
    let db = common::TestDb::create("os_lists_extension_302").await;
    let state = state(&db).await;
    seed_search_fixture(&db).await;

    for endpoint in [
        "searchForArtist",
        "searchForArtistImage",
        "searchForAlbumImage",
    ] {
        // Enforced auth with valid credentials: still the captured 302.
        let path = authed(&format!(
            "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&query=Dune"
        ));
        let (status, headers, body) = respond(state.clone(), enforced_get(&path)).await;
        assert_eq!(status, StatusCode::FOUND, "{endpoint}");
        assert_eq!(
            headers.get(header::LOCATION).unwrap(),
            "/Account/Login?ReturnUrl=%2FError",
            "{endpoint}"
        );
        assert_eq!(
            headers.get(header::CACHE_CONTROL).unwrap(),
            "no-cache,no-store",
            "{endpoint}"
        );
        assert_eq!(headers.get(header::EXPIRES).unwrap(), "-1", "{endpoint}");
        assert!(body.is_empty(), "{endpoint}");
        // The legacy ReturnUrl embedded the request query (credentials
        // included); that leak is not preserved.
        let location = headers
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(!location.contains("query="), "{endpoint}");
        assert!(!location.contains("t="), "{endpoint}");

        // The plain route form behaves identically.
        let path = authed(&format!("/rest/{endpoint}?f=json&query=Aurora%20Fields"));
        let (status, _, _) = respond(state.clone(), enforced_get(&path)).await;
        assert_eq!(status, StatusCode::FOUND, "{endpoint} plain");
    }
}
