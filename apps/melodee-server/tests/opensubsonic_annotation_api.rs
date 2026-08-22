//! OpenSubsonic annotation/bookmarks/play-queue compatibility evidence
//! star, unstar, setRating, scrobble, getBookmarks,
//! createBookmark, deleteBookmark, savePlayQueue, getPlayQueue.
//!
//! Asserted behavior mirrors the frozen golden captures in
//! `compatibility/opensubsonic/golden/annotation/` and the `os-bookmarks-*` /
//! `os-playqueue-*` captures under `compatibility/opensubsonic/golden/playlists/`.

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
    http::{Request, StatusCode},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::opensubsonic::OsState;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const FIXTURE_SECRET: &str = "fixture-os-secret-NOT-A-REAL-SECRET";
const SERVER_VERSION: &str = "0.0.0-test";
/// Captured scrobble timestamp (os-annotation-scrobble-scrobble-submission).
const SCROBBLE_TIME_MS: i64 = 1_786_050_060_000;

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
    let internal_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("user id");
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
        music_folders: Vec::new(),
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

async fn respond(state: OsState, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = melodee_server::opensubsonic::configured_router(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn enforced_get(path: &str) -> Request<Body> {
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

fn json_body(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("json body")
}

fn envelope(bytes: &[u8]) -> serde_json::Value {
    json_body(bytes)["subsonic-response"].clone()
}

/// Seeds two single-Track Releases and returns
/// ((track, release, artist) public ids for A, same for B).
async fn seed_two_releases(db: &common::TestDb) -> ([Uuid; 3], [Uuid; 3]) {
    phase5_common::seed_cataloged_release(
        db,
        7701,
        CatalogFixturePresentation {
            release_title: "Northern Lights",
            release_artist: "Aurora Fields",
            track_title: "Solar Wind",
            track_artist: "Aurora Fields",
            release_date: "2021-05-06",
        },
    )
    .await;
    phase5_common::seed_cataloged_release(
        db,
        7702,
        CatalogFixturePresentation {
            release_title: "Desert Rock Radio",
            release_artist: "Dune",
            track_title: "Sandstone",
            track_artist: "Dune",
            release_date: "2019-03-04",
        },
    )
    .await;
    let mut out = [[Uuid::nil(); 3]; 2];
    for (index, title) in ["Solar Wind", "Sandstone"].iter().enumerate() {
        let (track, release, artist): (Uuid, Uuid, Uuid) = sqlx::query_as(
            "SELECT track.public_api_id, release.public_api_id, artist.public_api_id
             FROM tracks track
             JOIN releases release ON release.id = track.release_id
             JOIN release_artists billing ON billing.release_id = release.id AND billing.role = 'primary'
             JOIN artists artist ON artist.id = billing.artist_id
             WHERE track.title = $1",
        )
        .bind(title)
        .fetch_one(&db.pool)
        .await
        .expect("seeded catalog ids");
        out[index] = [track, release, artist];
    }
    (out[0], out[1])
}

async fn track_user_state(db: &common::TestDb, track_public_id: Uuid) -> (bool, i16, i64) {
    let user_id = fixture_user_id(db).await;
    sqlx::query_as::<_, (bool, i16, i64)>(
        "SELECT starred, rating, play_count FROM native_track_user_states
         WHERE user_id = $1 AND track_id = (SELECT id FROM tracks WHERE public_api_id = $2)",
    )
    .bind(user_id)
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("track user state")
}

#[tokio::test]
async fn star_and_unstar_persist_captured_user_state() {
    let db = common::TestDb::create("os_anno_star").await;
    let state = state(&db).await;
    let ([song_a, album_a, artist_a], _) = seed_two_releases(&db).await;

    // star song via id (os-annotation-star-star-song): bare ok envelope.
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/star.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body),
        serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "type": "Melodee",
                "serverVersion": SERVER_VERSION,
                "openSubsonic": true
            }
        })
    );
    assert!(track_user_state(&db, song_a).await.0);

    // unstar song (os-annotation-star-unstar-song).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/unstar.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!track_user_state(&db, song_a).await.0);

    // star/unstar album via albumId (os-annotation-star-star-album-via-albumid).
    for endpoint in ["star", "unstar"] {
        let (status, _) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/{endpoint}.view?f=json&v=1.16.1&c=phase0&albumId=album_{album_a}"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        let starred: bool = sqlx::query_scalar(
            "SELECT starred FROM native_release_user_states
             WHERE release_id = (SELECT id FROM releases WHERE public_api_id = $1)",
        )
        .bind(album_a)
        .fetch_one(&db.pool)
        .await
        .expect("release state");
        assert_eq!(starred, endpoint == "star", "{endpoint}");
    }

    // star artist via artistId.
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/star.view?f=json&v=1.16.1&c=phase0&artistId=artist_{artist_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let starred: bool = sqlx::query_scalar(
        "SELECT starred FROM native_artist_user_states
         WHERE artist_id = (SELECT id FROM artists WHERE public_api_id = $1)",
    )
    .bind(artist_a)
    .fetch_one(&db.pool)
    .await
    .expect("artist state");
    assert!(starred);
}

#[tokio::test]
async fn star_malformed_id_matches_the_captured_error_44() {
    let db = common::TestDb::create("os_anno_star_malformed").await;
    let state = state(&db).await;
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/star.view?f=json&v=1.16.1&c=phase0&id=garbage",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body),
        serde_json::json!({
            "subsonic-response": {
                "status": "error",
                "version": "1.16.1",
                "type": "Melodee",
                "serverVersion": SERVER_VERSION,
                "openSubsonic": true,
                "error": {"code": 44, "message": "Invalid API key."}
            }
        })
    );
}

#[tokio::test]
async fn set_rating_set_clear_and_invalid_id_match_the_captures() {
    let db = common::TestDb::create("os_anno_rating").await;
    let state = state(&db).await;
    let ([song_a, _, _], _) = seed_two_releases(&db).await;

    // setRating rating=4 (os-annotation-rating-setrating-song-4).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/setRating.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&rating=4"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(track_user_state(&db, song_a).await.1, 4);

    // setRating rating=0 clears (os-annotation-rating-setrating-clear).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/setRating.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&rating=0"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(track_user_state(&db, song_a).await.1, 0);

    // Invalid id (os-annotation-rating-setrating-invalid-id).
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/setRating.view?f=json&v=1.16.1&c=phase0&id=garbage&rating=3",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["error"]["code"], 44);
    assert_eq!(envelope(&body)["error"]["message"], "Invalid API key.");

    // Missing rating parameter fails closed.
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/setRating.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["error"]["code"], 10);
}

#[tokio::test]
async fn scrobble_submission_records_a_play_and_repeat_is_deduped() {
    let db = common::TestDb::create("os_anno_scrobble").await;
    let state = state(&db).await;
    let ([song_a, _, _], _) = seed_two_releases(&db).await;
    let user_id = fixture_user_id(&db).await;

    // submission=false updates now-playing only (scrobble-nowplaying).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/scrobble.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&submission=false"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let now_playing_track: Option<Uuid> =
        sqlx::query_scalar("SELECT track_id FROM native_now_playing WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&db.pool)
            .await
            .expect("now playing");
    assert!(now_playing_track.is_some());
    let play_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("plays");
    assert_eq!(play_rows, 0, "now-playing must not record a play");

    // submission=true with an explicit time records one play
    // (scrobble-submission); the same submission repeated is idempotent
    // (scrobble-submission-repeat).
    for round in 0..2 {
        let (status, body) = respond(
            state.clone(),
            enforced_get(&authed(&format!(
                "/rest/scrobble.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&submission=true&time={SCROBBLE_TIME_MS}"
            ))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "round {round}");
        assert_eq!(envelope(&body)["status"], "ok", "round {round}");
    }
    let event_id = format!("opensubsonic-scrobble:{song_a}:{SCROBBLE_TIME_MS}");
    let plays: Vec<(String, OffsetDateTime)> = sqlx::query_as(
        "SELECT client_event_id, played_at FROM native_track_plays WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&db.pool)
    .await
    .expect("plays");
    assert_eq!(plays.len(), 1, "repeat submission must be deduped");
    assert_eq!(plays[0].0, event_id);
    assert_eq!(
        plays[0].1.unix_timestamp(),
        SCROBBLE_TIME_MS / 1000,
        "played_at keeps the submitted time"
    );
    let (_, _, play_count) = track_user_state(&db, song_a).await;
    assert_eq!(play_count, 1, "play counter bumps exactly once");
    // A recorded play clears now-playing (shared store contract).
    let now_playing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_now_playing WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("now playing");
    assert_eq!(now_playing, 0);
}

#[tokio::test]
async fn scrobble_default_submission_records_a_play() {
    let db = common::TestDb::create("os_anno_scrobble_default").await;
    let state = state(&db).await;
    let ([song_a, _, _], _) = seed_two_releases(&db).await;
    let user_id = fixture_user_id(&db).await;

    // No submission parameter: the captured default is a real submission
    // (os-annotation-scrobble-scrobble-default-submission).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/scrobble.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let play_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("plays");
    assert_eq!(play_rows, 1);
}

#[tokio::test]
async fn scrobble_time_mismatch_keeps_the_captured_ok_error_quirk() {
    let db = common::TestDb::create("os_anno_scrobble_mismatch").await;
    let state = state(&db).await;
    let ([song_a, _, _], [song_b, _, _]) = seed_two_releases(&db).await;
    let user_id = fixture_user_id(&db).await;

    // Two ids, one time (os-annotation-scrobble-scrobble-time-mismatch):
    // status stays "ok" while carrying error code 0.
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/scrobble.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&id=song_{song_b}&time={SCROBBLE_TIME_MS}&submission=true"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body),
        serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "type": "Melodee",
                "serverVersion": SERVER_VERSION,
                "openSubsonic": true,
                "error": {"code": 0, "message": "Wrong number of timestamps."}
            }
        })
    );
    let play_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("plays");
    assert_eq!(play_rows, 0, "a mismatched submission records nothing");
}

#[tokio::test]
async fn scrobble_malformed_id_matches_the_captured_error_44() {
    let db = common::TestDb::create("os_anno_scrobble_malformed").await;
    let state = state(&db).await;
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed(
            "/rest/scrobble.view?f=json&v=1.16.1&c=phase0&id=garbage&submission=true",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "error");
    assert_eq!(envelope(&body)["error"]["code"], 44);
}

#[tokio::test]
async fn bookmarks_round_trip_matches_the_captures() {
    let db = common::TestDb::create("os_anno_bookmarks").await;
    let state = state(&db).await;
    let ([song_a, album_a, _], _) = seed_two_releases(&db).await;

    // getBookmarks empty (os-bookmarks-getbookmarks-empty).
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getBookmarks.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["bookmarks"],
        serde_json::json!({"bookmark": []})
    );

    // XML form of the same empty shape.
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getBookmarks.view?v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let xml = String::from_utf8(body).unwrap();
    assert!(xml.contains("<bookmarks></bookmarks>"), "{xml}");

    // Star and rate the song first so the nested Child shows the captured
    // list-context fields (starred timestamp, userRating).
    for path in [
        format!("/rest/star.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"),
        format!("/rest/setRating.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&rating=5"),
    ] {
        let (status, _) = respond(state.clone(), enforced_get(&authed(&path))).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }

    // createBookmark with the captured position verbatim
    // (os-bookmarks-create-bookmark).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/createBookmark.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&position=42000&comment=probe"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // getBookmarks verify (os-bookmarks-verify-bookmarks).
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getBookmarks.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bookmarks = &envelope(&body)["bookmarks"]["bookmark"];
    let bookmark = &bookmarks[0];
    assert_eq!(
        bookmarks.as_array().unwrap().len(),
        1,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        bookmark["position"], 42000,
        "raw position preserved verbatim"
    );
    assert_eq!(bookmark["username"], "phase0-fixture");
    assert_eq!(bookmark["comment"], "probe");
    assert!(bookmark["created"].as_str().unwrap().ends_with('Z'));
    assert!(bookmark["changed"].as_str().unwrap().ends_with('Z'));
    let entry = &bookmark["entry"][0];
    assert_eq!(entry["id"], format!("song_{song_a}"));
    assert_eq!(entry["parent"], format!("album_{album_a}"));
    assert_eq!(entry["isDir"], false);
    assert_eq!(entry["title"], "Solar Wind");
    assert_eq!(entry["album"], "Northern Lights");
    assert_eq!(entry["artist"], "Aurora Fields");
    assert_eq!(entry["song"], 1);
    assert_eq!(entry["track"], 1);
    assert_eq!(entry["year"], 2021);
    assert_eq!(entry["coverArt"], format!("song_{song_a}"));
    assert_eq!(entry["albumId"], format!("album_{album_a}"));
    assert_eq!(entry["type"], "music");
    assert_eq!(entry["mediaType"], "song");
    assert_eq!(entry["isVideo"], false);
    assert_eq!(entry["bpm"], 0, "no audio analysis in the fixture");
    assert_eq!(entry["sortName"], "Solar Wind");
    assert_eq!(entry["duration"], 180);
    assert_eq!(entry["genre"], "Fixture");
    assert_eq!(
        entry["genres"],
        serde_json::json!([{"value": "Fixture", "songCount": 0, "albumCount": 0}])
    );
    assert_eq!(entry["artists"], serde_json::json!([]));
    assert_eq!(entry["displayArtist"], "Aurora Fields");
    assert_eq!(entry["displayAlbumArtist"], "Aurora Fields");
    assert_eq!(entry["contributors"], serde_json::json!([]));
    assert_eq!(entry["replayGain"], serde_json::json!({}));
    assert_eq!(entry["averageRating"], 5);
    assert_eq!(entry["userRating"], 5);
    assert_eq!(entry["playerId"], 0);
    assert_eq!(entry["playCount"], 0);
    // Captured: song `starred` is the legacy Nullable<DateTime>.ToString()
    // artifact — "" in every frozen golden (runtime-set timestamps are masked
    // volatile there).
    assert_eq!(entry["starred"], "");
    let album_artist = &entry["albumArtists"][0];
    assert_eq!(album_artist["name"], "Aurora Fields");
    assert_eq!(album_artist["artistImageUrl"], "URL", "literal quirk");
    assert_eq!(album_artist["albumCount"], 1);
    assert_eq!(album_artist["userRating"], 0);

    // deleteBookmark (os-bookmarks-delete-bookmark) then verify empty
    // (os-bookmarks-verify-bookmarks-empty).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/deleteBookmark.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getBookmarks.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["bookmarks"],
        serde_json::json!({"bookmark": []})
    );

    // Deleting an absent bookmark is an idempotent ok.
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/deleteBookmark.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn play_queue_round_trip_preserves_the_playqueid_quirk() {
    let db = common::TestDb::create("os_anno_playqueue").await;
    let state = state(&db).await;
    let ([song_a, _, _], [song_b, _, _]) = seed_two_releases(&db).await;

    // getPlayQueue with no saved queue: synthesized default whose changedBy
    // is the username (os-playqueue-getplayqueue-empty).
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlayQueue.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let queue = &envelope(&body)["playQueue"];
    assert_eq!(queue["current"], 0);
    assert_eq!(queue["position"], 0);
    assert_eq!(queue["changedBy"], "phase0-fixture");
    assert_eq!(queue["username"], "phase0-fixture");
    assert!(queue["changed"].as_str().unwrap().ends_with('Z'));
    assert_eq!(queue["entry"], serde_json::json!([]));

    // savePlayQueue (os-playqueue-saveplayqueue): two songs, current is the
    // second song id, position preserved verbatim.
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed(&format!(
            "/rest/savePlayQueue.view?f=json&v=1.16.1&c=phase0&id=song_{song_a}&id=song_{song_b}&current=song_{song_b}&position=42000"
        ))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The captured PlayQueId quirk: `current` in the response is the internal
    // bigint entry id, not the submitted song id.
    let expected_current: i64 = sqlx::query_scalar(
        "SELECT item.id FROM opensubsonic_play_queue_items item
         JOIN tracks track ON track.id = item.track_id
         WHERE track.public_api_id = $1",
    )
    .bind(song_b)
    .fetch_one(&db.pool)
    .await
    .expect("queue entry id");

    // getPlayQueue verify (os-playqueue-verify-playqueue).
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlayQueue.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let queue = &envelope(&body)["playQueue"];
    assert_eq!(
        queue["current"],
        serde_json::json!(expected_current),
        "current is the bigint entry id (PlayQueId quirk)"
    );
    assert_eq!(queue["position"], 42000);
    assert_eq!(queue["changedBy"], "phase0", "c client name is stored");
    assert_eq!(queue["username"], "phase0-fixture");
    let entries = queue["entry"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], format!("song_{song_a}"));
    assert_eq!(entries[1]["id"], format!("song_{song_b}"));
    assert_eq!(entries[0]["title"], "Solar Wind");
    assert_eq!(entries[1]["title"], "Sandstone");
    // List context: unrated/unstarred songs omit userRating and starred.
    assert!(entries[0].get("userRating").is_none());
    assert!(entries[0].get("starred").is_none());

    // Clearing (savePlayQueue without ids) empties the queue
    // (os-playqueue-clear-playqueue + os-playqueue-verify-playqueue-empty).
    let (status, _) = respond(
        state.clone(),
        enforced_get(&authed("/rest/savePlayQueue.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM opensubsonic_play_queue_items")
        .fetch_one(&db.pool)
        .await
        .expect("items");
    assert_eq!(remaining, 0);
    let (status, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlayQueue.view?f=json&v=1.16.1&c=phase0")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let queue = &envelope(&body)["playQueue"];
    assert_eq!(queue["current"], 0);
    assert_eq!(queue["position"], 0);
    assert_eq!(queue["changedBy"], "phase0-fixture");
    assert_eq!(queue["entry"], serde_json::json!([]));
}

#[tokio::test]
async fn requests_without_a_principal_fail_closed_with_error_40() {
    let db = common::TestDb::create("os_anno_no_principal").await;
    let state = state(&db).await;

    // Unauthenticated enforced request: the auth middleware answers error 40.
    for path in [
        "/rest/star.view?f=json&id=song_00000000-0000-0000-0000-000000000000",
        "/rest/getBookmarks.view?f=json",
        "/rest/getPlayQueue.view?f=json",
    ] {
        let (status, body) = respond(state.clone(), enforced_get(path)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(envelope(&body)["error"]["code"], 40, "{path}");
    }

    // The captured localhost bypass synthesizes no user (BlankUserInfo); the
    // per-user endpoints fail closed rather than acting on a phantom user.
    let mut local = Request::builder()
        .method("GET")
        .uri("/rest/getBookmarks.view?f=json")
        .body(Body::empty())
        .expect("request");
    local.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        8000,
    )));
    let (status, body) = respond(state.clone(), local).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["error"]["code"], 40);
}
