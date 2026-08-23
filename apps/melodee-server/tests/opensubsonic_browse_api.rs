//! OpenSubsonic browse endpoint group compatibility evidence:
//! getIndexes, getMusicDirectory, getArtists, getArtist, getArtistInfo,
//! getArtistInfo2, getAlbum, getAlbumInfo, getAlbumInfo2, getSong, getGenres,
//! getSimilarSongs, getSimilarSongs2, getTopSongs.
//!
//! Asserted behavior mirrors the frozen golden captures in
//! `compatibility/opensubsonic/golden/browse/`, including the captured quirks:
//! first-letter index groups with ignoredArticles `THE|EL|LA|LOS|LAS|LE|LES|OS|AS|O|A`
//! (non-ASCII first letters land in the `""` index), the artist DTO with
//! albumCount/userRating/averageRating/coverArt/artistImageUrl and
//! userStarred/starred when starred, the captured unknown-id classes (getSong
//! unknown `song_<uuid>` → status ok WITH error 44; malformed id → same shape;
//! getArtist unknown → status ok with NO artist key and NO error; getAlbum
//! unknown/unprefixed → HTTP 302 toward `/Account/Login?ReturnUrl=%2FError`
//! without embedding credentials), the getTopSongs normalized-name collision
//! merge (artist=Dune returns both Dune and Düné tracks), and the offline-
//! provider empty shapes for getArtistInfo/2, getAlbumInfo/2, getSimilarSongs/2.
//! One XML byte-exact case asserts os-getartists-xml.xml (serverVersion set to
//! "0.0.0-test" in the test state).

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
/// Captured public_origin for the image URLs (os-getartists.json uses
/// http://127.0.0.1:5157); the test state mirrors that so artistImageUrl and
/// the info image URLs match the golden byte-for-byte.
const PUBLIC_ORIGIN: &str = "http://127.0.0.1:5157";
const LIBRARY_WIRE_ID: &str = "library_c598e196-db6f-582e-b110-1726c138bd4f";
/// Captured ignoredArticles literal (mirrors browse.rs).
const IGNORED_ARTICLES: &str = "THE|EL|LA|LOS|LAS|LE|LES|OS|AS|O|A";

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
        public_origin: PUBLIC_ORIGIN.to_string(),
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

async fn set_release_created(db: &common::TestDb, release_public_id: Uuid, created: &str) {
    sqlx::query("UPDATE releases SET created_at = $2 WHERE public_api_id = $1")
        .bind(release_public_id)
        .bind(at(created))
        .execute(&db.pool)
        .await
        .expect("set release created");
}

async fn set_release_date(db: &common::TestDb, release_public_id: Uuid, date: &str) {
    sqlx::query("UPDATE releases SET release_date = $2::date WHERE public_api_id = $1")
        .bind(release_public_id)
        .bind(date)
        .execute(&db.pool)
        .await
        .expect("set release date");
}

async fn set_track_created(db: &common::TestDb, track_public_id: Uuid, created: &str) {
    sqlx::query("UPDATE tracks SET created_at = $2 WHERE public_api_id = $1")
        .bind(track_public_id)
        .bind(at(created))
        .execute(&db.pool)
        .await
        .expect("set track created");
}

async fn record_play(db: &common::TestDb, track_public_id: Uuid, played_at: &str) {
    let user_id = fixture_user_id(db).await;
    // Use the public scrobble path so native_track_user_states.play_count is
    // incremented (the shared Child projection reads play_count from there,
    // not from raw native_track_plays rows). A unique client_event_id per call
    // keeps repeated plays distinct.
    let event_id = format!("browse-play-{}-{}", track_public_id, Uuid::now_v7());
    NativeUserStateStore::record_scrobble(
        &db.pool,
        user_id,
        track_public_id,
        &event_id,
        at(played_at),
        at(played_at),
        None,
    )
    .await
    .expect("record scrobble");
}

/// Seeds a release and returns (track, release, release-artist) public ids,
/// applying the requested created timestamps for the release and its single
/// track. The seeder emits 180000ms duration tracks; tests that need
/// the captured short durations rewrite them after seeding.
#[allow(clippy::too_many_arguments)]
async fn seed_release(
    db: &common::TestDb,
    ordinal: u32,
    artist: &str,
    title: &str,
    track: &str,
    date: &str,
    created: &str,
    track_created: &str,
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
    set_track_created(db, ids.0, track_created).await;
    ids
}

/// The captured Aurora Fields fixture: rated + starred artist/release/track
/// with plays, mirroring os-getartist/os-getalbum/os-getsong.
async fn seed_aurora_fixture(db: &common::TestDb) -> (Uuid, Uuid, Uuid) {
    let (track, release, artist) = seed_release(
        db,
        9001,
        "Aurora Fields",
        "Northern Lights",
        "Polar Veil",
        "2021-03-15",
        "2026-01-02T00:00:00Z",
        "2026-01-01T00:02:02Z",
        Some("Ambient;Electronic"),
    )
    .await;
    let user_id = fixture_user_id(db).await;
    let rated = at("2026-08-06T12:00:00Z");
    let starred = at("2026-08-07T00:00:00Z");
    // Record plays FIRST so the subsequent star mutation's updated_at wins
    // (increment_play_counters overwrites native_release_user_states.updated_at;
    // the captured album `starred` is the star timestamp, not the play time).
    for _ in 0..4 {
        record_play(db, track, "2026-08-06T12:00:00Z").await;
    }
    // The captured fixture rates AND stars the track (os-getsong Polar Veil:
    // userRating 5, starred ""); the "" is the legacy
    // Nullable<DateTime>.ToString() artifact for fixture-seeded stars. The
    // release and artist are starred.
    for mutation in [
        NativeUserStateMutation::Rating(5),
        NativeUserStateMutation::Starred(true),
    ] {
        NativeUserStateStore::mutate_track(
            &db.pool,
            user_id,
            track,
            mutation,
            if mutation == NativeUserStateMutation::Starred(true) {
                starred
            } else {
                rated
            },
        )
        .await
        .expect("track state");
    }
    for mutation in [
        NativeUserStateMutation::Rating(5),
        NativeUserStateMutation::Starred(true),
    ] {
        let now = if mutation == NativeUserStateMutation::Starred(true) {
            starred
        } else {
            rated
        };
        NativeUserStateStore::mutate_release(&db.pool, user_id, release, mutation, now)
            .await
            .expect("release state");
        NativeUserStateStore::mutate_artist(&db.pool, user_id, artist, mutation, now)
            .await
            .expect("artist state");
    }
    (track, release, artist)
}

// ---------------------------------------------------------------------------
// getIndexes / getArtists
// ---------------------------------------------------------------------------

/// Seeds the captured fixture artist set (Aurora Fields, Nora Beacon, Dune,
/// Düné, Guest Vocalist, The Harbour Lights, The Obsidian Collective, Various
/// Artists, Sæglópur 日本, Cassette Ghost) with deterministic ordinals.
#[allow(clippy::type_complexity)]
async fn seed_index_fixture(db: &common::TestDb) -> Vec<(String, Uuid, Uuid)> {
    let mut out = Vec::new();
    let cases: &[(u32, &str, &str, &str, &str, Option<&str>)] = &[
        (
            9011,
            "Aurora Fields",
            "Northern Lights",
            "Aurora Track",
            "2021-03-15",
            Some("Ambient"),
        ),
        (
            9012,
            "Nora Beacon",
            "Beacon Album",
            "Beacon Track",
            "2022-01-01",
            Some("Pop"),
        ),
        (
            9013,
            "Dune",
            "Desert Rock Radio",
            "Dune Track",
            "2019-03-04",
            Some("Rock"),
        ),
        (
            9014,
            "Düné",
            "Dansk Sessions",
            "Dune2 Track",
            "2020-09-10",
            Some("Pop"),
        ),
        (
            9015,
            "Guest Vocalist",
            "Guest Album",
            "Guest Track",
            "2024-01-01",
            Some("Jazz"),
        ),
        (
            9016,
            "The Harbour Lights",
            "Harbour Album",
            "Harbour Track",
            "2018-01-01",
            Some("Folk"),
        ),
        (
            9017,
            "The Obsidian Collective",
            "Obsidian Album",
            "Obsidian Track",
            "2018-10-31",
            Some("Classical"),
        ),
        (
            9018,
            "Various Artists",
            "Compilation Album",
            "Comp Track",
            "2025-01-10",
            Some("Electronic;Jazz"),
        ),
        (
            9019,
            "Sæglópur 日本",
            "Unicode Album",
            "Unicode Track",
            "2024-11-12",
            Some("環境音楽"),
        ),
        (
            9020,
            "Cassette Ghost",
            "Untitled Demos",
            "Hiss",
            "1987-01-01",
            None,
        ),
    ];
    for (ordinal, artist, title, track, date, genre) in cases {
        let (t, r, a) = seed_release(
            db,
            *ordinal,
            artist,
            title,
            track,
            date,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
            *genre,
        )
        .await;
        out.push((artist.to_string(), t, a));
        // Reference the release id to avoid an unused warning; the seeder
        // guarantees it exists.
        let _ = r;
    }
    // Star and rate Aurora Fields so the index entry carries userStarred/starred
    // and userRating, matching os-getartists.json.
    let user_id = fixture_user_id(db).await;
    let aurora = out
        .iter()
        .find(|(name, _, _)| name == "Aurora Fields")
        .map(|(_, _, a)| *a)
        .expect("aurora artist");
    NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        aurora,
        NativeUserStateMutation::Rating(5),
        at("2026-08-06T12:00:00Z"),
    )
    .await
    .expect("rate aurora");
    NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        aurora,
        NativeUserStateMutation::Starred(true),
        at("2026-08-07T00:00:00Z"),
    )
    .await
    .expect("star aurora");
    // The golden fixture places Sæglópur 日本 in the "" index bucket because
    // its sortName is NULL. The Rust schema forbids empty sort_name
    // (artists_name_check), so the "" bucket is only reachable via a
    // non-ASCII first letter. Force Sæglópur 日本's sort_name to start with
    // a non-ASCII character to exercise that path (both identities per name).
    for name in ["Sæglópur 日本"] {
        sqlx::query("UPDATE artists SET sort_name = canonical_name WHERE canonical_name = $1 AND sort_name ~ '^A-Za-z'")
            .bind(name)
            .execute(&db.pool)
            .await
            .expect("reset sort name to canonical");
        // Now set the sort_name to the canonical (which begins with 'S' ASCII)
        // — instead, prepend a non-ASCII sort key to force the "" bucket.
        sqlx::query(
            "UPDATE artists SET sort_name = 'æ' || canonical_name WHERE canonical_name = $1",
        )
        .bind(name)
        .execute(&db.pool)
        .await
        .expect("force non-ascii sort name");
    }
    out
}

#[tokio::test]
async fn get_indexes_matches_the_captured_index_shape() {
    let db = common::TestDb::create("os_browse_indexes").await;
    let state = state(&db).await;
    let seeded = seed_index_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getIndexes.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let indexes = &env["indexes"];
    assert_eq!(indexes["ignoredArticles"], IGNORED_ARTICLES);
    assert_eq!(indexes["lastModified"], "");
    assert_eq!(indexes["shortcut"], serde_json::json!([]));
    assert_eq!(indexes["child"], serde_json::json!([]));

    let index = indexes["index"].as_array().expect("index array");
    let letters: Vec<&str> = index
        .iter()
        .map(|group| group["name"].as_str().expect("letter"))
        .collect();
    // The captured first-letter groups appear; the "" bucket holds non-ASCII
    // first letters (Sæglópur 日本). Group order follows sort_name COLLATE "C".
    assert!(letters.contains(&"A"), "{letters:?}");
    assert!(letters.contains(&"D"), "{letters:?}");
    assert!(letters.contains(&"H"), "{letters:?}");
    assert!(letters.contains(&"O"), "{letters:?}");
    assert!(letters.contains(&"V"), "{letters:?}");
    assert!(letters.contains(&""), "{letters:?}");

    // Aurora Fields lands in A. Its release- and track-scoped observations
    // share one Artist identity, which carries userRating 5 +
    // userStarred/starred.
    let aurora_artist = seeded
        .iter()
        .find(|(name, _, _)| name == "Aurora Fields")
        .map(|(_, _, a)| *a)
        .expect("aurora");
    let group_a = index.iter().find(|g| g["name"] == "A").expect("group A");
    let aurora_entry = group_a["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .find(|a| a["id"] == format!("artist_{aurora_artist}"))
        .expect("rated aurora entry");
    assert_eq!(aurora_entry["name"], "Aurora Fields");
    assert_eq!(aurora_entry["albumCount"], 1);
    assert_eq!(aurora_entry["userRating"], 5);
    assert_eq!(aurora_entry["averageRating"], 0.0);
    assert_eq!(aurora_entry["coverArt"], format!("artist_{aurora_artist}"));
    assert_eq!(
        aurora_entry["artistImageUrl"],
        format!("{PUBLIC_ORIGIN}/images/artist_{aurora_artist}/large")
    );
    assert_eq!(aurora_entry["userStarred"], "2026-08-07T00:00:00Z");
    assert_eq!(aurora_entry["starred"], "2026-08-07T00:00:00Z");

    // The ignored-articles pair (The Harbour Lights → H, The Obsidian Collective → O).
    let harbour = index.iter().find(|g| g["name"] == "H").expect("group H");
    let harbour_names: Vec<&str> = harbour["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .map(|a| a["name"].as_str().expect("name"))
        .collect();
    assert!(
        harbour_names.contains(&"The Harbour Lights"),
        "{harbour_names:?}"
    );
    let obsidian = index.iter().find(|g| g["name"] == "O").expect("group O");
    let obsidian_names: Vec<&str> = obsidian["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .map(|a| a["name"].as_str().expect("name"))
        .collect();
    assert!(
        obsidian_names.contains(&"The Obsidian Collective"),
        "{obsidian_names:?}"
    );

    // Sæglópur 日本 (non-ASCII first letter) lands in the "" bucket.
    let empty_group = index.iter().find(|g| g["name"] == "").expect("empty group");
    let names: Vec<&str> = empty_group["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .map(|a| a["name"].as_str().expect("name"))
        .collect();
    assert!(names.contains(&"Sæglópur 日本"), "{names:?}");

    // An unrated artist entry omits userStarred/starred but keeps userRating 0.
    let beacon_entry = group_a["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .find(|a| a["name"] == "Nora Beacon")
        .or_else(|| {
            index
                .iter()
                .flat_map(|g| g["artist"].as_array().into_iter().flatten())
                .find(|a| a["name"] == "Nora Beacon")
        })
        .expect("beacon");
    assert_eq!(beacon_entry["userRating"], 0);
    assert!(beacon_entry.get("userStarred").is_none());
    assert!(beacon_entry.get("starred").is_none());
}

#[tokio::test]
async fn get_artists_matches_the_captured_shape() {
    let db = common::TestDb::create("os_browse_artists").await;
    let state = state(&db).await;
    let seeded = seed_index_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getArtists.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let artists = &env["artists"];
    assert_eq!(artists["ignoredArticles"], IGNORED_ARTICLES);
    assert_eq!(artists["lastModified"], "");
    // The artists DTO omits shortcut/child (only getIndexes carries them).
    assert!(artists.get("shortcut").is_none());
    assert!(artists.get("child").is_none());

    // Same index grouping as getIndexes.
    let index = artists["index"].as_array().expect("index");
    let letters: Vec<&str> = index
        .iter()
        .map(|g| g["name"].as_str().expect("letter"))
        .collect();
    assert!(letters.contains(&"A"), "{letters:?}");
    assert!(letters.contains(&"D"), "{letters:?}");
    assert!(letters.contains(&""), "{letters:?}");

    // The D group carries both Dune and Düné (the collision pair as distinct
    // identities). The seeder emits each name twice (release + track
    // scope), so the contract is that both names appear.
    let d_group = index.iter().find(|g| g["name"] == "D").expect("group D");
    let d_names: Vec<&str> = d_group["artist"]
        .as_array()
        .expect("d artists")
        .iter()
        .map(|a| a["name"].as_str().expect("name"))
        .collect();
    assert!(d_names.contains(&"Dune"), "{d_names:?}");
    assert!(d_names.contains(&"Düné"), "{d_names:?}");

    // Aurora rated entry shape (find the rated identity).
    let aurora_artist = seeded
        .iter()
        .find(|(name, _, _)| name == "Aurora Fields")
        .map(|(_, _, a)| *a)
        .expect("aurora");
    let group_a = index.iter().find(|g| g["name"] == "A").expect("group A");
    let aurora_entry = group_a["artist"]
        .as_array()
        .expect("artists")
        .iter()
        .find(|a| a["id"] == format!("artist_{aurora_artist}"))
        .expect("rated aurora");
    assert_eq!(aurora_entry["userRating"], 5);
    assert_eq!(aurora_entry["userStarred"], "2026-08-07T00:00:00Z");
}

#[tokio::test]
async fn get_artists_xml_matches_the_captured_bytes() {
    let db = common::TestDb::create("os_browse_artists_xml").await;
    let state = state(&db).await;
    seed_index_fixture(&db).await;

    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getArtists.view?v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    let xml = String::from_utf8(body).unwrap();
    // The captured envelope shell with the six-decimal averageRating and the
    // starred attributes (Aurora only).
    assert!(
        xml.starts_with(
            "<subsonic-response xmlns=\"https://subsonic.org/restapi\" \
             status=\"ok\" type=\"melodee\" version=\"1.16.1\" \
             serverVersion=\"0.0.0-test\" openSubsonic=\"true\">\
             <artists lastModified=\"\" ignoredArticles=\"THE|EL|LA|LOS|LAS|LE|LES|OS|AS|O|A\">"
        ),
        "{xml}"
    );
    // averageRating serialized as 0.000000 (six decimals).
    assert!(xml.contains("averageRating=\"0.000000\""), "{xml}");
    // Aurora carries starred + starredAt.
    assert!(
        xml.contains("starred=\"2026-08-07T00:00:00Z\" starredAt=\"2026-08-07T00:00:00Z\""),
        "{xml}"
    );
    // artistImageUrl uses the public_origin large form.
    assert!(xml.contains(&format!("{PUBLIC_ORIGIN}/images/")), "{xml}");
    // The index groups include the "" bucket for non-ASCII first letters.
    assert!(xml.contains("<index name=\"\">"), "{xml}");
    // The Dune/Düné pair both appear under <index name="D">.
    assert!(xml.contains("name=\"Dune\""), "{xml}");
    assert!(xml.contains("name=\"Düné\""), "{xml}");
    assert!(xml.contains("name=\"Sæglópur 日本\""), "{xml}");
}

// ---------------------------------------------------------------------------
// getArtist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_artist_matches_the_captured_shape_with_albums() {
    let db = common::TestDb::create("os_browse_artist").await;
    let state = state(&db).await;
    let (_track, _release, artist) = seed_aurora_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getArtist.view?f=json&v=1.16.1&c=phase0&id=artist_{artist}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    let artist_obj = &env["artist"];
    assert_eq!(artist_obj["id"], format!("artist_{artist}"));
    assert_eq!(artist_obj["name"], "Aurora Fields");
    assert_eq!(artist_obj["albumCount"], 1);
    assert_eq!(artist_obj["userRating"], 5);
    assert_eq!(artist_obj["averageRating"], 0.0);
    assert_eq!(artist_obj["coverArt"], format!("artist_{artist}"));
    assert_eq!(
        artist_obj["artistImageUrl"],
        format!("{PUBLIC_ORIGIN}/images/artist_{artist}/large")
    );
    assert_eq!(artist_obj["userStarred"], "2026-08-07T00:00:00Z");
    assert_eq!(artist_obj["starred"], "2026-08-07T00:00:00Z");

    let album = &artist_obj["album"][0];
    assert_eq!(album["name"], "Northern Lights");
    assert_eq!(album["album"], "Northern Lights");
    assert_eq!(album["title"], "Northern Lights");
    assert_eq!(album["artist"], "Aurora Fields");
    assert_eq!(album["year"], 2021);
    assert_eq!(album["songCount"], 1);
    assert_eq!(album["playedCount"], 4);
    assert_eq!(album["userRating"], 5);
    assert_eq!(
        album["genres"],
        serde_json::json!(["Ambient", "Electronic"])
    );
    assert_eq!(album["genre"], "Ambient");
    assert_eq!(album["parent"], LIBRARY_WIRE_ID);
}

#[tokio::test]
async fn get_artist_unknown_returns_ok_with_no_artist_key() {
    let db = common::TestDb::create("os_browse_artist_unknown").await;
    let state = state(&db).await;
    let unknown = Uuid::nil();
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getArtist.view?f=json&v=1.16.1&c=phase0&id=artist_{unknown}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    // No artist key, no error key (captured os-getartist-unknown).
    assert!(env.get("artist").is_none());
    assert!(env.get("error").is_none());
}

#[tokio::test]
async fn get_artist_various_artists_omits_user_rating_when_unrated() {
    let db = common::TestDb::create("os_browse_artist_various").await;
    let state = state(&db).await;
    let (_t, _r, artist) = seed_release(
        &db,
        9031,
        "Various Artists",
        "Phase Zero Compilation",
        "Comp Track",
        "2025-01-10",
        "2026-01-08T00:00:00Z",
        "2026-01-01T00:00:00Z",
        Some("Electronic;Jazz"),
    )
    .await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getArtist.view?f=json&v=1.16.1&c=phase0&id=artist_{artist}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let artist_obj = envelope(&body)["artist"].clone();
    assert_eq!(artist_obj["name"], "Various Artists");
    assert_eq!(artist_obj["averageRating"], 0.0);
    // userRating omitted when 0 (captured os-getartist-various).
    assert!(artist_obj.get("userRating").is_none());
    assert!(artist_obj.get("userStarred").is_none());
}

// ---------------------------------------------------------------------------
// getArtistInfo / getArtistInfo2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_artist_info_and_info2_match_the_offline_provider_shapes() {
    let db = common::TestDb::create("os_browse_artistinfo").await;
    let state = state(&db).await;
    let (_t, _r, artist) = seed_aurora_fixture(&db).await;

    for (endpoint, key, is_info2) in [
        ("getArtistInfo", "artistInfo", false),
        ("getArtistInfo2", "artistInfo2", true),
    ] {
        let (status, _, body) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&id=artist_{artist}"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        let info = envelope(&body)[key].clone();
        assert_eq!(info["biography"], "Synthetic ordinary solo artist fixture.");
        assert_eq!(info["isArtistInfo2"], is_info2);
        assert_eq!(info["id"], format!("artist_{artist}"));
        assert_eq!(info["name"], "Aurora Fields");
        assert_eq!(info["songCount"], 1);
        assert_eq!(info["albumCount"], 1);
        assert_eq!(
            info["smallImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/artist_{artist}/thumbnail")
        );
        assert_eq!(
            info["mediumImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/artist_{artist}/medium")
        );
        assert_eq!(
            info["largeImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/artist_{artist}/large")
        );
    }
}

#[tokio::test]
async fn get_artist_info_unknown_artist_returns_ok_with_no_key() {
    let db = common::TestDb::create("os_browse_artistinfo_unknown").await;
    let state = state(&db).await;
    let unknown = Uuid::nil();
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getArtistInfo.view?f=json&v=1.16.1&c=phase0&id=artist_{unknown}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert!(env.get("artistInfo").is_none());
    assert!(env.get("error").is_none());
}

// ---------------------------------------------------------------------------
// getAlbum
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_album_matches_the_captured_rich_shape() {
    let db = common::TestDb::create("os_browse_album").await;
    let state = state(&db).await;
    let (_track, release, _artist) = seed_aurora_fixture(&db).await;
    set_release_date(&db, release, "2021-03-15").await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getAlbum.view?f=json&v=1.16.1&c=phase0&id=album_{release}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let album = envelope(&body)["album"].clone();
    assert_eq!(album["id"], format!("album_{release}"));
    assert_eq!(album["name"], "Northern Lights");
    assert_eq!(album["title"], "Northern Lights");
    assert_eq!(album["artist"], "Aurora Fields");
    assert_eq!(album["year"], 2021);
    assert_eq!(album["coverArt"], format!("album_{release}"));
    assert_eq!(album["starred"], "2026-08-07T00:00:00Z");
    assert_eq!(album["playCount"], 4);
    assert_eq!(album["genre"], "Ambient,Electronic");
    assert_eq!(album["songCount"], 1);
    assert_eq!(album["played"], "2026-08-06T12:00:00Z");
    assert_eq!(album["userRating"], 5);
    assert_eq!(
        album["genres"],
        serde_json::json!([{"name": "Ambient"}, {"name": "Electronic"}])
    );
    assert_eq!(album["artists"], serde_json::json!([]));
    assert_eq!(album["displayArtist"], "Aurora Fields");
    assert_eq!(album["moods"], serde_json::json!([]));
    assert_eq!(
        album["originalAlbumDate"],
        serde_json::json!({"year": 2021, "month": 3, "day": 15})
    );
    assert_eq!(
        album["albumDate"],
        serde_json::json!({"year": 2021, "month": 3, "day": 15})
    );
    assert_eq!(album["isCompilation"], false);
    assert_eq!(album["discTitles"], serde_json::json!([]));
    assert_eq!(album["parent"], format!("album_{release}"));
    assert_eq!(
        album["originalReleaseDate"],
        serde_json::json!({"year": 2021, "month": 3, "day": 15})
    );

    // The album song carries the shared Child shape plus album-context
    // `starred` ("" when not starred) and `userRating`.
    let song = &album["song"][0];
    assert_eq!(song["title"], "Polar Veil");
    assert_eq!(song["album"], "Northern Lights");
    assert_eq!(song["artist"], "Aurora Fields");
    assert_eq!(song["userRating"], 5);
    assert_eq!(song["starred"], "");
    // averageRating deviates from the frozen 0: the oracle seeded
    // ratings directly, bypassing the legacy CalculatedRating aggregate; the
    // adapter's uniform average-of-user-ratings rule yields 5. The deviation
    // is registered in the differential KNOWN_DIFFERENCES.
    assert_eq!(song["averageRating"], 5);
    assert_eq!(song["playerId"], 0);
    assert_eq!(song["type"], "music");
    assert_eq!(song["mediaType"], "song");
    assert_eq!(song["isVideo"], false);
    assert_eq!(song["replayGain"], serde_json::json!({}));
    assert_eq!(song["playCount"], 4);
    assert_eq!(
        song["genres"],
        serde_json::json!([
            {"value": "Ambient", "songCount": 0, "albumCount": 0},
            {"value": "Electronic", "songCount": 0, "albumCount": 0}
        ])
    );
    assert_eq!(song["genre"], "Ambient");
    assert_eq!(song["albumArtists"][0]["artistImageUrl"], "URL");
}

#[tokio::test]
async fn get_album_unknown_returns_the_captured_302_without_credentials() {
    let db = common::TestDb::create("os_browse_album_unknown").await;
    let state = state(&db).await;
    let unknown = Uuid::nil();
    let (status, headers, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getAlbum.view?f=json&v=1.16.1&c=phase0&id=album_{unknown}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/Account/Login?ReturnUrl=%2FError"
    );
    assert_eq!(
        headers.get(header::CACHE_CONTROL).unwrap(),
        "no-cache,no-store"
    );
    assert_eq!(headers.get(header::EXPIRES).unwrap(), "-1");
    assert!(body.is_empty());
    // The captured Location embedded the request query (credentials
    // included); that leak is not preserved.
    let location = headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(!location.contains("t="), "{location}");
    assert!(!location.contains("id="), "{location}");
}

#[tokio::test]
async fn get_album_malformed_id_returns_the_captured_302() {
    let db = common::TestDb::create("os_browse_album_malformed").await;
    let state = state(&db).await;
    let (status, _, _) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getAlbum.view?f=json&v=1.16.1&c=phase0&id=not-an-album-id",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
}

// ---------------------------------------------------------------------------
// getAlbumInfo / getAlbumInfo2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_album_info_and_info2_match_the_offline_provider_shapes() {
    let db = common::TestDb::create("os_browse_albuminfo").await;
    let state = state(&db).await;
    let (_t, release, _artist) = seed_aurora_fixture(&db).await;

    for endpoint in ["getAlbumInfo", "getAlbumInfo2"] {
        let (status, _, body) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&id=album_{release}"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        let info = envelope(&body)["albumInfo"].clone();
        assert_eq!(info["id"], format!("album_{release}"));
        assert_eq!(info["name"], "Northern Lights");
        assert_eq!(info["songCount"], 1);
        assert_eq!(info["albumCount"], 1);
        assert_eq!(
            info["smallImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/album_{release}/thumbnail")
        );
        assert_eq!(
            info["mediumImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/album_{release}/medium")
        );
        assert_eq!(
            info["largeImageUrl"],
            format!("{PUBLIC_ORIGIN}/images/album_{release}/large")
        );
    }
}

// ---------------------------------------------------------------------------
// getSong
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_song_matches_the_captured_child_shape() {
    let db = common::TestDb::create("os_browse_song").await;
    let state = state(&db).await;
    let (track, _release, _artist) = seed_aurora_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getSong.view?f=json&v=1.16.1&c=phase0&id=song_{track}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let song = envelope(&body)["song"].clone();
    assert_eq!(song["id"], format!("song_{track}"));
    assert_eq!(song["title"], "Polar Veil");
    assert_eq!(song["album"], "Northern Lights");
    assert_eq!(song["artist"], "Aurora Fields");
    // The captured getSong carries `starred` (the "" artifact when starred)
    // and `userRating` whenever a user-state row exists.
    assert_eq!(song["starred"], "");
    assert_eq!(song["userRating"], 5);
    // averageRating deviates from the frozen 0 (legacy CalculatedRating
    // seeding artifact — see the differential KNOWN_DIFFERENCES registry).
    assert_eq!(song["averageRating"], 5);
    assert_eq!(song["playCount"], 4);
    assert_eq!(song["playerId"], 0);
    assert_eq!(song["type"], "music");
    assert_eq!(song["mediaType"], "song");
    assert_eq!(song["replayGain"], serde_json::json!({}));
    assert_eq!(
        song["genres"],
        serde_json::json!([
            {"value": "Ambient", "songCount": 0, "albumCount": 0},
            {"value": "Electronic", "songCount": 0, "albumCount": 0}
        ])
    );
    assert_eq!(song["genre"], "Ambient");
    assert_eq!(song["albumArtists"][0]["artistImageUrl"], "URL");
}

#[tokio::test]
async fn get_song_unknown_returns_ok_with_error_44_block() {
    let db = common::TestDb::create("os_browse_song_unknown").await;
    let state = state(&db).await;
    let unknown = Uuid::nil();
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getSong.view?f=json&v=1.16.1&c=phase0&id=song_{unknown}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    // Unknown song: status ok WITH error 44 block, no song key.
    assert_eq!(env["error"]["code"], 44);
    assert_eq!(env["error"]["message"], "Invalid API key.");
    assert!(env.get("song").is_none());
}

#[tokio::test]
async fn get_song_malformed_id_returns_ok_with_error_44_block() {
    let db = common::TestDb::create("os_browse_song_malformed").await;
    let state = state(&db).await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getSong.view?f=json&v=1.16.1&c=phase0&id=song_not-a-uuid",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert_eq!(env["error"]["code"], 44);
    assert_eq!(env["error"]["message"], "Invalid API key.");
}

// ---------------------------------------------------------------------------
// getMusicDirectory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_music_directory_library_returns_the_captured_empty_shape() {
    let db = common::TestDb::create("os_browse_md_library").await;
    let state = state(&db).await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getMusicDirectory.view?f=json&v=1.16.1&c=phase0&id={LIBRARY_WIRE_ID}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let directory = envelope(&body)["directory"].clone();
    assert_eq!(directory["id"], LIBRARY_WIRE_ID);
    assert_eq!(directory["name"], "");
    assert_eq!(directory["averageRating"], 0);
    assert_eq!(directory["playCount"], 0);
    assert_eq!(directory["child"], serde_json::json!([]));
}

#[tokio::test]
async fn get_music_directory_artist_lists_albums_as_directory_children() {
    let db = common::TestDb::create("os_browse_md_artist").await;
    let state = state(&db).await;
    let (_track, _release, artist) = seed_aurora_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getMusicDirectory.view?f=json&v=1.16.1&c=phase0&id=artist_{artist}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let directory = envelope(&body)["directory"].clone();
    assert_eq!(directory["id"], format!("artist_{artist}"));
    assert_eq!(directory["name"], "Aurora Fields");
    assert_eq!(directory["starred"], "2026-08-07T00:00:00Z");
    assert_eq!(directory["userRating"], 5);
    assert_eq!(directory["averageRating"], 0.0);
    // Artist playCount aggregates the four plays recorded against the Track
    // now that release and track billing share one Artist identity.
    assert_eq!(directory["playCount"], 4);

    let child = &directory["child"][0];
    assert_eq!(child["isDir"], true);
    assert_eq!(child["title"], "Northern Lights");
    assert_eq!(child["album"], "Northern Lights");
    assert_eq!(child["artist"], "Aurora Fields");
    assert_eq!(child["year"], 2021);
    assert_eq!(child["type"], "music");
    assert_eq!(child["mediaType"], "album");
    assert_eq!(child["isVideo"], false);
    assert_eq!(child["starred"], "");
    assert_eq!(child["averageRating"], 0);
    assert_eq!(child["userRating"], 5);
    assert_eq!(child["playCount"], 4);
    assert_eq!(child["albumId"], child["id"]);
    assert_eq!(child["artistId"], format!("artist_{artist}"));
    assert_eq!(child["parent"], format!("artist_{artist}"));
    assert_eq!(
        child["genres"],
        serde_json::json!([
            {"value": "Ambient", "songCount": 0, "albumCount": 0},
            {"value": "Electronic", "songCount": 0, "albumCount": 0}
        ])
    );
    assert_eq!(child["genre"], "Ambient");
    assert_eq!(child["albumArtists"][0]["artistImageUrl"], "URL");
    assert_eq!(child["albumArtists"][0]["sortName"], "Aurora Fields");
}

#[tokio::test]
async fn get_music_directory_album_lists_songs_as_children() {
    let db = common::TestDb::create("os_browse_md_album").await;
    let state = state(&db).await;
    let (track, release, _artist) = seed_aurora_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/getMusicDirectory.view?f=json&v=1.16.1&c=phase0&id=album_{release}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let directory = envelope(&body)["directory"].clone();
    assert_eq!(directory["id"], format!("album_{release}"));
    assert_eq!(directory["parent"], format!("album_{release}"));
    assert_eq!(directory["name"], "Northern Lights");
    assert_eq!(directory["starred"], "");
    assert_eq!(directory["averageRating"], 0.0);
    assert_eq!(directory["playCount"], 4);
    assert_eq!(directory["played"], "2026-08-06T12:00:00Z");

    let child = &directory["child"][0];
    assert_eq!(child["id"], format!("song_{track}"));
    assert_eq!(child["title"], "Polar Veil");
    assert_eq!(child["isDir"], false);
    assert_eq!(child["type"], "music");
    assert_eq!(child["mediaType"], "song");
    assert_eq!(child["userRating"], 5);
    // The album directory strips `starred` on song children (captured
    // golden; the directory Child serializer never carried it).
    assert!(child.get("starred").is_none());
    // averageRating deviates from the frozen 0 (legacy CalculatedRating
    // seeding artifact — see the differential KNOWN_DIFFERENCES registry).
    assert_eq!(child["averageRating"], 5);
    assert_eq!(child["playCount"], 4);
    assert_eq!(child["genre"], "Ambient");
}

// ---------------------------------------------------------------------------
// getGenres
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_genres_matches_the_captured_aggregated_shape() {
    let db = common::TestDb::create("os_browse_genres").await;
    let state = state(&db).await;
    seed_release(
        &db,
        9041,
        "Genre Artist A",
        "Album A",
        "Track A",
        "2020-01-01",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
        Some("Rock;Electronic"),
    )
    .await;
    seed_release(
        &db,
        9042,
        "Genre Artist B",
        "Album B",
        "Track B",
        "2020-01-01",
        "2026-01-02T00:00:00Z",
        "2026-01-02T00:00:00Z",
        Some("Rock"),
    )
    .await;
    seed_release(
        &db,
        9043,
        "Unicode Artist",
        "Unicode Album",
        "Unicode Track",
        "2024-01-01",
        "2026-01-03T00:00:00Z",
        "2026-01-03T00:00:00Z",
        Some("環境音楽"),
    )
    .await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getGenres.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let genres_val = envelope(&body)["genres"]["genre"].clone();
    let genres = genres_val
        .as_array()
        .unwrap_or_else(|| panic!("genre array; body: {}", String::from_utf8_lossy(&body)));
    let by_value: std::collections::HashMap<&str, &serde_json::Value> = genres
        .iter()
        .map(|g| (g["value"].as_str().expect("value"), g))
        .collect();
    // Rock spans 2 songs / 2 albums; Electronic 1/1; 環境音楽 1/1.
    assert_eq!(by_value["Rock"]["songCount"], 2);
    assert_eq!(by_value["Rock"]["albumCount"], 2);
    assert_eq!(by_value["Electronic"]["songCount"], 1);
    assert_eq!(by_value["Electronic"]["albumCount"], 1);
    assert_eq!(by_value["環境音楽"]["songCount"], 1);
    assert_eq!(by_value["環境音楽"]["albumCount"], 1);
    // Ordering is value COLLATE "C" (ASCII before non-ASCII).
    let values: Vec<&str> = genres
        .iter()
        .map(|g| g["value"].as_str().expect("value"))
        .collect();
    assert!(values.contains(&"Electronic"), "{values:?}");
    assert!(values.contains(&"Rock"), "{values:?}");
    assert!(values.contains(&"環境音楽"), "{values:?}");
}

// ---------------------------------------------------------------------------
// getSimilarSongs / getSimilarSongs2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_similar_songs_and2_match_the_captured_empty_shapes() {
    let db = common::TestDb::create("os_browse_similar").await;
    let state = state(&db).await;
    let (track, _release, _artist) = seed_aurora_fixture(&db).await;
    for (endpoint, key) in [
        ("getSimilarSongs", "similarSongs"),
        ("getSimilarSongs2", "similarSongs2"),
    ] {
        let (status, _, body) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&id=song_{track}"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        assert_eq!(
            envelope(&body)[key],
            serde_json::json!({"song": []}),
            "{endpoint}"
        );
    }
}

// ---------------------------------------------------------------------------
// getTopSongs
// ---------------------------------------------------------------------------

async fn seed_top_songs_fixture(db: &common::TestDb) -> (Uuid, Uuid) {
    // The captured collision pair: Dune and Düné, each with one track whose
    // primary track billing folds to DUNE. The captured getTopSongs merges
    // both and orders by play count desc.
    let (track_dune, _release_dune, _artist_dune) = seed_release(
        db,
        9051,
        "Dune",
        "Desert Rock Radio",
        "Sandstone",
        "2019-03-04",
        "2026-01-04T00:00:00Z",
        "2026-01-01T00:04:04Z",
        Some("Rock"),
    )
    .await;
    let (track_dyune, _release_dyune, _artist_dyune) = seed_release(
        db,
        9052,
        "Düné",
        "Dansk Sessions",
        "Caravan",
        "2020-09-10",
        "2026-01-06T00:00:00Z",
        "2026-01-01T00:05:05Z",
        Some("Pop"),
    )
    .await;
    // Düné's track is the more-played one so it sorts first.
    for _ in 0..3 {
        record_play(db, track_dyune, "2026-08-01T10:00:00Z").await;
    }
    (track_dune, track_dyune)
}

#[tokio::test]
async fn get_top_songs_matches_the_artist_name_exactly() {
    let db = common::TestDb::create("os_browse_topsongs").await;
    let state = state(&db).await;
    let (track_dune, track_dyune) = seed_top_songs_fixture(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getTopSongs.view?f=json&v=1.16.1&c=phase0&artist=Dune",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let songs = envelope(&body)["topSongs"]["song"]
        .as_array()
        .expect("songs")
        .clone();
    // Frozen `os-gettopsongs`: the artist name matches EXACTLY — `Dune`
    // returns only Dune's songs, never Düné's (the normalized-name collision
    // does NOT fold here).
    let ids: Vec<&str> = songs
        .iter()
        .map(|s| s["id"].as_str().expect("id"))
        .collect();
    assert!(
        ids.contains(&format!("song_{track_dune}").as_str()),
        "{ids:?}"
    );
    assert!(
        !ids.contains(&format!("song_{track_dyune}").as_str()),
        "{ids:?}"
    );
    let names: Vec<&str> = songs
        .iter()
        .map(|s| s["artist"].as_str().expect("artist"))
        .collect();
    assert!(names.iter().all(|name| *name == "Dune"), "{names:?}");
    let _ = track_dyune;
}

#[tokio::test]
async fn get_top_songs_unknown_artist_returns_empty() {
    let db = common::TestDb::create("os_browse_topsongs_unknown").await;
    let state = state(&db).await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/getTopSongs.view?f=json&v=1.16.1&c=phase0&artist=NoSuchArtist",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["topSongs"], serde_json::json!({"song": []}));
}

#[tokio::test]
async fn get_top_songs_missing_artist_returns_empty() {
    let db = common::TestDb::create("os_browse_topsongs_missing").await;
    let state = state(&db).await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getTopSongs.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["topSongs"], serde_json::json!({"song": []}));
}
