//! OpenSubsonic playlist compatibility evidence (Phase 8).
//!
//! Asserts the captured playlist shapes (getPlaylists/getPlaylist lists and
//! entries, createPlaylist returning the new playlist, deletePlaylist bare
//! ok) and the do-not-preserve mutation-breakage deviation (coverage-matrix
//! D4): updatePlaylist applies cleanly instead of the legacy 302 cover-
//! resolution crash, and unknown playlists answer a credential-free 302
//! rather than embedding the request query in the redirect Location.

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
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::opensubsonic::{MusicFolder, OsState};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const FIXTURE_SECRET: &str = "fixture-os-secret-NOT-A-REAL-SECRET";
const SERVER_VERSION: &str = "0.0.0-test";

fn music_folders() -> Vec<std::sync::Arc<MusicFolder>> {
    vec![std::sync::Arc::new(MusicFolder {
        id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
        name: "Phase0 Fixture Library".to_string(),
    })]
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

fn authed(path: &str) -> String {
    let salt = path.len().to_string();
    format!("{path}&u=phase0-fixture&t={}&s={salt}", token_for(&salt))
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

fn envelope(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(bytes).expect("json body")["subsonic-response"]
        .clone()
}

async fn seed_track(db: &common::TestDb, ordinal: u32, title: &str) -> Uuid {
    phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: title,
            track_title: title,
            track_artist: title,
            release_date: "2021-01-01",
        },
    )
    .await;
    sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE title = $1")
        .bind(title)
        .fetch_one(&db.pool)
        .await
        .expect("track id")
}

#[tokio::test]
async fn playlists_round_trip_create_get_update_delete() {
    let db = common::TestDb::create("os_playlists_round_trip").await;
    let state = state(&db).await;
    let song_a = seed_track(&db, 8001, "Solar Wind").await;
    let song_b = seed_track(&db, 8002, "Neon Tokyo").await;

    // Empty list first.
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlaylists.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["playlists"]["playlist"],
        serde_json::json!([])
    );

    // createPlaylist with two songs returns the new playlist with entries.
    let path = format!(
        "/rest/createPlaylist.view?f=json&name=OS-Probe&songId=song_{song_a}&songId=song_{song_b}"
    );
    let (status, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let playlist = &envelope(&body)["playlist"];
    assert_eq!(playlist["name"], "OS-Probe");
    assert_eq!(playlist["owner"], "phase0-fixture");
    assert_eq!(playlist["public"], false);
    assert_eq!(playlist["songCount"], 2);
    assert_eq!(playlist["allowedUsers"], serde_json::json!([]));
    assert_eq!(playlist["entry"].as_array().unwrap().len(), 2);
    let playlist_id = playlist["id"].as_str().unwrap().to_string();

    // getPlaylist returns the same playlist with entries in order.
    let path = format!("/rest/getPlaylist.view?f=json&id={playlist_id}");
    let (status, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let playlist = &envelope(&body)["playlist"];
    assert_eq!(playlist["id"], playlist_id);
    assert_eq!(playlist["songCount"], 2);

    // getPlaylists now lists it.
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlaylists.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = envelope(&body)["playlists"]["playlist"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "OS-Probe");

    // updatePlaylist: rename + add a third song + remove index 0. This is the
    // D4 do-not-preserve deviation: the legacy crashed with a 302 mid-
    // serialization; the Rust adapter applies the change cleanly.
    let song_c = seed_track(&db, 8003, "Coda").await;
    let path = format!(
        "/rest/updatePlaylist.view?f=json&playlistId={playlist_id}&name=Renamed&songIdToAdd=song_{song_c}&songIndexToRemove=0"
    );
    let (status, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "updatePlaylist must not 302 (D4 fix)"
    );
    assert_eq!(envelope(&body)["status"], "ok");

    let path = format!("/rest/getPlaylist.view?f=json&id={playlist_id}");
    let (status, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let playlist = &envelope(&body)["playlist"];
    assert_eq!(playlist["name"], "Renamed");
    assert_eq!(playlist["songCount"], 2);

    // deletePlaylist answers the captured bare ok.
    let path = format!("/rest/deletePlaylist.view?f=json&id={playlist_id}");
    let (status, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "ok");

    // After deletion the list is empty again.
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&authed("/rest/getPlaylists.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["playlists"]["playlist"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn unknown_playlist_answers_a_credential_free_302() {
    let db = common::TestDb::create("os_playlist_unknown_302").await;
    let state = state(&db).await;
    let path = "/rest/getPlaylist.view?f=json&id=playlist_00000000-0000-0000-0000-000000000000"
        .to_string();
    let (status, headers, _body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        location.contains("/Account/Login?ReturnUrl=%2FError"),
        "{location}"
    );
    // The credential-leak class is not preserved: no t/s/u query material.
    assert!(!location.contains("t="), "{location}");
    assert!(!location.contains("s="), "{location}");
    assert!(!location.contains("u="), "{location}");
}

#[tokio::test]
async fn private_playlist_is_invisible_to_non_owners() {
    let db = common::TestDb::create("os_playlist_private").await;
    let state = state(&db).await;
    let song = seed_track(&db, 8101, "Lantern").await;
    let path = format!("/rest/createPlaylist.view?f=json&name=Private&songId=song_{song}");
    let (_, _, body) = respond(state.clone(), enforced_get(&authed(&path))).await;
    let playlist_id = envelope(&body)["playlist"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A second user sees no playlists (the private one is filtered out).
    let now = OffsetDateTime::now_utc();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "other-user",
                normalized_username: "OTHER-USER",
                email: "other@example.test",
                normalized_email: "OTHER@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "other-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasStreamRole".to_string()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import other user");
    sqlx::query("INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) SELECT id, 'other-secret', now(), now() FROM native_users WHERE normalized_username='OTHER-USER'")
        .execute(&db.pool)
        .await
        .expect("seed other secret");
    let salt = "priv".to_string();
    let token = {
        let mut hasher = Md5::new();
        hasher.update(b"other-secret");
        hasher.update(salt.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let path = format!("/rest/getPlaylists.view?f=json&u=other-user&t={token}&s={salt}");
    let (status, _, body) = respond(state.clone(), enforced_get(&path)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["playlists"]["playlist"],
        serde_json::json!([])
    );

    // The private playlist is not directly fetchable by the non-owner (302,
    // credential-free).
    let path =
        format!("/rest/getPlaylist.view?f=json&id={playlist_id}&u=other-user&t={token}&s={salt}");
    let (status, headers, _body) = respond(state.clone(), enforced_get(&path)).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .contains("/Account/Login?ReturnUrl=%2FError")
    );
}
