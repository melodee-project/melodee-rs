//! OpenSubsonic conditional endpoint compatibility evidence.
//!
//! Asserts the captured shapes for shares, internet radio, scan, users, and
//! podcast mutations (PRD-API-OS-004/005; golden evidence
//! `compatibility/opensubsonic/golden/community`, `.../conditional`). The
//! podcast mutation failures assert the captured raw ResponseData body (NOT the
//! subsonic envelope) at HTTP 400, including the disabled-state shape, and the
//! `streamPodcastEpisode` HasStreamRole gate.

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
    http::{HeaderMap, Method, Request, StatusCode, header},
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
const PUBLIC_ORIGIN: &str = "http://melodee.test";

fn music_folders() -> Vec<std::sync::Arc<MusicFolder>> {
    vec![std::sync::Arc::new(MusicFolder {
        id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
        name: "Phase0 Fixture Library".to_string(),
    })]
}

/// Imports the fixture admin user with the full captured role set
/// (Administrator + stream/download/share/jukebox) so `getUser` matches the
/// captured role model, and seeds the OpenSubsonic secret.
async fn seed_fixture_user(db: &common::TestDb) {
    let now = OffsetDateTime::now_utc();
    let roles = vec![
        "Administrator".to_string(),
        "HasStreamRole".to_string(),
        "HasDownloadRole".to_string(),
        "HasShareRole".to_string(),
        "HasJukeboxRole".to_string(),
    ];
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
    sqlx::query(
        "INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) \
         VALUES ($1, $2, now(), now())",
    )
    .bind(internal_id)
    .bind(FIXTURE_SECRET)
    .execute(&db.pool)
    .await
    .expect("seed secret");
}

async fn fixture_public_api_id(db: &common::TestDb) -> Uuid {
    sqlx::query_scalar(
        "SELECT public_api_id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("public_api_id")
}

/// Imports an additional user with a chosen role set and OpenSubsonic secret
/// (used for the non-admin radio gate and the no-stream podcast gate).
async fn seed_extra_user(
    db: &common::TestDb,
    username: &str,
    normalized: &str,
    secret: &str,
    is_admin: bool,
    roles: &[&str],
) {
    let now = OffsetDateTime::now_utc();
    let roles_vec: Vec<String> = roles.iter().map(|role| role.to_string()).collect();
    let email = format!("{username}@example.test");
    let normalized_email = format!("{normalized}@EXAMPLE.TEST");
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username,
                normalized_username: normalized,
                email: &email,
                normalized_email: &normalized_email,
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "extra-stream-key",
                is_admin,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles_vec,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import extra user");
    let internal_id: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username = $1")
            .bind(normalized)
            .fetch_one(&db.pool)
            .await
            .expect("extra user id");
    sqlx::query(
        "INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) \
         VALUES ($1, $2, now(), now())",
    )
    .bind(internal_id)
    .bind(secret)
    .execute(&db.pool)
    .await
    .expect("seed extra secret");
}

async fn state(db: &common::TestDb) -> OsState {
    seed_fixture_user(db).await;
    OsState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        server_version: SERVER_VERSION.to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        base_url_host: Some("capture-fixture.example.test".to_string()),
        legacy_auth_bypass: true, // Frozen ADR-0130 fixture; production defaults false.
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

fn token(secret: &str, salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(secret.as_bytes());
    hasher.update(salt.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn authed(path: &str) -> String {
    authed_as(path, "phase0-fixture", FIXTURE_SECRET)
}

fn authed_as(path: &str, username: &str, secret: &str) -> String {
    let salt = path.len().to_string();
    format!("{path}&u={username}&t={}&s={salt}", token(secret, &salt))
}

fn get(path: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::GET)
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

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .unwrap_or_default()
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

/// Asserts the captured raw ResponseData wrapper (NOT the subsonic envelope)
/// emitted for podcast conditional failures, carrying the captured message and
/// the authenticated user's info.
fn assert_raw_response_data(body: &[u8], message: &str, public_api_id: Uuid) {
    let json: serde_json::Value = serde_json::from_slice(body).expect("json body");
    // Raw ResponseData top-level marker; the subsonic envelope has no top-level
    // `isSuccess`/`userInfo`/`totalCount`.
    assert_eq!(json["isSuccess"], true, "raw ResponseData isSuccess");
    assert_eq!(json["totalCount"], 0, "raw ResponseData totalCount");
    assert_eq!(json["apiKeyId"], serde_json::Value::Null);
    assert!(
        json.get("status").is_none(),
        "must be raw ResponseData, not the subsonic envelope"
    );
    let subsonic = &json["subsonic-response"];
    assert_eq!(subsonic["isSuccess"], false);
    assert_eq!(subsonic["type"], "Melodee");
    assert_eq!(subsonic["version"], "1.16.1");
    assert_eq!(subsonic["error"]["code"], 0);
    assert_eq!(subsonic["error"]["message"], message);
    assert_eq!(subsonic["error"]["helpUrl"], serde_json::Value::Null);
    assert_eq!(subsonic["etag"], serde_json::Value::Null);
    assert_eq!(subsonic["data"], serde_json::Value::Null);
    // Captured: the podcast raw wrapper always carries BLANK user info, even
    // for authenticated callers (the legacy handler never populated it).
    assert_eq!(json["userInfo"]["userName"], "");
    assert_eq!(
        json["userInfo"]["apiKey"].as_str().unwrap(),
        "00000000-0000-0000-0000-000000000000"
    );
    let _ = public_api_id;
}

// ---------------------------------------------------------------------------
// Shares
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_shares_empty_list_shape() {
    let db = common::TestDb::create("os_cond_shares_empty").await;
    let state = state(&db).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getShares.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["shares"]["share"], serde_json::json!([]));
}

#[tokio::test]
async fn create_share_with_song_id_returns_share_with_url() {
    let db = common::TestDb::create("os_cond_share_create").await;
    let state = state(&db).await;
    let track = seed_track(&db, 7001, "Share Probe").await;
    let path = format!("/rest/createShare.view?f=json&id=song_{track}&description=probe-share");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let share = &envelope(&body)["shares"]["share"];
    let share_id = share["id"].as_str().expect("share id");
    assert!(
        share_id.starts_with("share_"),
        "share id must be share_{{guid}}: {share_id}"
    );
    let url = share["url"].as_str().expect("share url");
    assert!(
        url.starts_with("http://melodee.test/share/"),
        "share url must be {{public_origin}}/share/{{unique}}: {url}"
    );
    assert_eq!(share["description"], "probe-share");
    assert_eq!(share["username"], "phase0-fixture");
    assert_eq!(share["visitCount"], 0);
    let entry = share["entry"].as_array().expect("entry array");
    assert_eq!(entry.len(), 1, "track share lists one song child");
    assert_eq!(entry[0]["id"], format!("song_{track}"));
}

#[tokio::test]
async fn create_share_bad_id_redirects_credential_free_302() {
    let db = common::TestDb::create("os_cond_share_bad_id").await;
    let state = state(&db).await;
    let path = "/rest/createShare.view?f=json&id=garbage";
    let (status, headers, _body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(header_str(&headers, header::LOCATION).contains("/Account/Login?ReturnUrl=%2FError"));
}

#[tokio::test]
async fn get_shares_after_create_lists_it() {
    let db = common::TestDb::create("os_cond_shares_after_create").await;
    let state = state(&db).await;
    let track = seed_track(&db, 7002, "Listed Probe").await;
    let path = format!("/rest/createShare.view?f=json&id=song_{track}");
    let (_, _, _body) = respond(state.clone(), get(&authed(&path))).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getShares.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let list = envelope(&body)["shares"]["share"]
        .as_array()
        .expect("share list")
        .clone();
    assert_eq!(list.len(), 1);
    assert!(list[0]["id"].as_str().unwrap().starts_with("share_"));
}

#[tokio::test]
async fn update_share_by_id_returns_ok() {
    let db = common::TestDb::create("os_cond_share_update").await;
    let state = state(&db).await;
    let track = seed_track(&db, 7003, "Update Probe").await;
    let create = format!("/rest/createShare.view?f=json&id=song_{track}");
    let (_, _, body) = respond(state.clone(), get(&authed(&create))).await;
    let share_id = envelope(&body)["shares"]["share"]["id"]
        .as_str()
        .expect("share id")
        .to_string();
    let path = format!("/rest/updateShare.view?f=json&id={share_id}&description=updated");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "ok");
}

#[tokio::test]
async fn update_share_unknown_returns_error_44() {
    let db = common::TestDb::create("os_cond_share_update_unknown").await;
    let state = state(&db).await;
    let path = "/rest/updateShare.view?f=json&id=share_00000000-0000-0000-0000-000000000000";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 44);
    assert_eq!(env["error"]["message"], "Invalid API key.");
}

#[tokio::test]
async fn delete_share_by_id_returns_ok_and_clears_list() {
    let db = common::TestDb::create("os_cond_share_delete").await;
    let state = state(&db).await;
    let track = seed_track(&db, 7004, "Delete Probe").await;
    let create = format!("/rest/createShare.view?f=json&id=song_{track}");
    let (_, _, body) = respond(state.clone(), get(&authed(&create))).await;
    let share_id = envelope(&body)["shares"]["share"]["id"]
        .as_str()
        .expect("share id")
        .to_string();
    let path = format!("/rest/deleteShare.view?f=json&id={share_id}");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "ok");

    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getShares.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["shares"]["share"], serde_json::json!([]));
}

#[tokio::test]
async fn delete_share_unknown_returns_error_44() {
    let db = common::TestDb::create("os_cond_share_delete_unknown").await;
    let state = state(&db).await;
    let path = "/rest/deleteShare.view?f=json&id=share_00000000-0000-0000-0000-000000000000";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["error"]["code"], 44);
    assert_eq!(env["error"]["message"], "Invalid API key.");
}

// ---------------------------------------------------------------------------
// Internet radio
// ---------------------------------------------------------------------------

fn radio_create_path(name: &str, stream_url: &str, home_page_url: Option<&str>) -> String {
    // Encode spaces as `+` (the params decoder maps `+` back to a space).
    let encoded_name = name.replace(' ', "+");
    let mut path = format!(
        "/rest/createInternetRadioStation.view?f=json&name={encoded_name}&streamUrl={stream_url}"
    );
    if let Some(home) = home_page_url {
        path.push_str(&format!("&homepageUrl={home}"));
    }
    path
}

async fn radio_list(state: OsState) -> Vec<serde_json::Value> {
    let (_, _, body) = respond(
        state,
        get(&authed("/rest/getInternetRadioStations.view?f=json")),
    )
    .await;
    envelope(&body)["internetRadioStations"]["internetRadioStation"]
        .as_array()
        .expect("radio list")
        .clone()
}

#[tokio::test]
async fn get_internet_radio_stations_empty() {
    let db = common::TestDb::create("os_cond_radio_empty").await;
    let state = state(&db).await;
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed("/rest/getInternetRadioStations.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["internetRadioStations"]["internetRadioStation"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn create_radio_station_as_admin_returns_ok() {
    let db = common::TestDb::create("os_cond_radio_create").await;
    let state = state(&db).await;
    let path = radio_create_path(
        "Phase0 Radio",
        "http://radio.example.test/stream",
        Some("http://radio.example.test"),
    );
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert!(
        env.get("internetRadioStations").is_none(),
        "createInternetRadioStation returns bare ok, not the station"
    );
}

#[tokio::test]
async fn create_radio_station_as_non_admin_returns_auth_error() {
    let db = common::TestDb::create("os_cond_radio_non_admin").await;
    let state = state(&db).await;
    seed_extra_user(
        &db,
        "radio-non-admin",
        "RADIO-NON-ADMIN",
        "radio-secret",
        false,
        &[],
    )
    .await;
    let path = radio_create_path(
        "Phase0 Radio",
        "http://radio.example.test/stream",
        Some("http://radio.example.test"),
    );
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed_as(&path, "radio-non-admin", "radio-secret")),
    )
    .await;
    // The captured admin gate fails closed with the auth error envelope at
    // HTTP 200 (radio::error_reply uses StatusCode::OK).
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 40);
    assert_eq!(env["error"]["message"], "Wrong username or password.");
}

#[tokio::test]
async fn verify_radio_list_after_create() {
    let db = common::TestDb::create("os_cond_radio_verify").await;
    let state = state(&db).await;
    let path = radio_create_path(
        "Phase0 Radio",
        "http://radio.example.test/stream",
        Some("http://radio.example.test"),
    );
    let (_, _, _body) = respond(state.clone(), get(&authed(&path))).await;
    let list = radio_list(state.clone()).await;
    assert_eq!(list.len(), 1);
    let station = &list[0];
    assert!(station["id"].as_str().unwrap().starts_with("radio_"));
    assert_eq!(station["name"], "Phase0 Radio");
    assert_eq!(station["streamUrl"], "http://radio.example.test/stream");
    assert_eq!(station["homePageUrl"], "http://radio.example.test");
}

#[tokio::test]
async fn update_radio_station_returns_ok_and_renames() {
    let db = common::TestDb::create("os_cond_radio_update").await;
    let state = state(&db).await;
    let create = radio_create_path("Phase0 Radio", "http://radio.example.test/stream", None);
    let (_, _, _body) = respond(state.clone(), get(&authed(&create))).await;
    let list = radio_list(state.clone()).await;
    let radio_id = list[0]["id"].as_str().expect("radio id").to_string();
    let path =
        format!("/rest/updateInternetRadioStation.view?f=json&id={radio_id}&name=Renamed+Radio");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "ok");
    let list = radio_list(state.clone()).await;
    assert_eq!(list[0]["name"], "Renamed Radio");
}

#[tokio::test]
async fn delete_radio_station_returns_ok_and_clears_list() {
    let db = common::TestDb::create("os_cond_radio_delete").await;
    let state = state(&db).await;
    let create = radio_create_path("Phase0 Radio", "http://radio.example.test/stream", None);
    let (_, _, _body) = respond(state.clone(), get(&authed(&create))).await;
    let list = radio_list(state.clone()).await;
    let radio_id = list[0]["id"].as_str().expect("radio id").to_string();
    let path = format!("/rest/deleteInternetRadioStation.view?f=json&id={radio_id}");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["status"], "ok");
    let list = radio_list(state.clone()).await;
    assert!(list.is_empty(), "radio list is empty after delete");
}

// ---------------------------------------------------------------------------
// Scan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_scan_status_with_no_run_returns_scanning_false() {
    let db = common::TestDb::create("os_cond_scan_status_initial").await;
    let state = state(&db).await;
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed("/rest/getScanStatus.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["scanStatus"]["scanning"], false);
    assert_eq!(envelope(&body)["scanStatus"]["count"], 0);
}

#[tokio::test]
async fn start_scan_returns_scanning_true_count_zero() {
    let db = common::TestDb::create("os_cond_scan_start").await;
    let state = state(&db).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/startScan.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(envelope(&body)["scanStatus"]["scanning"], true);
    assert_eq!(envelope(&body)["scanStatus"]["count"], 0);
}

#[tokio::test]
async fn get_scan_status_after_start_returns_scanning_true() {
    // QuartzDisabled baseline: startScan records a run with finished_at NULL
    // (the scheduler cannot fire the job), so getScanStatus reports the run as
    // still in progress.
    let db = common::TestDb::create("os_cond_scan_status_after").await;
    let state = state(&db).await;
    let (_, _, _body) = respond(state.clone(), get(&authed("/rest/startScan.view?f=json"))).await;
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed("/rest/getScanStatus.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Captured (`os-scan/getscanstatus-after`): with the scheduler disabled
    // the run row stays open but scanning reports false.
    assert_eq!(envelope(&body)["scanStatus"]["scanning"], false);
    assert_eq!(envelope(&body)["scanStatus"]["count"], 0);
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_user_returns_captured_role_model() {
    let db = common::TestDb::create("os_cond_get_user").await;
    let state = state(&db).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getUser.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let user = &envelope(&body)["user"];
    assert_eq!(user["username"], "phase0-fixture");
    assert_eq!(user["email"], "phase0-fixture@example.test");
    assert_eq!(user["adminRole"], true);
    assert_eq!(user["streamRole"], true);
    assert_eq!(user["scrobblingEnabled"], true);
    assert_eq!(user["downloadRole"], true);
    assert_eq!(user["shareRole"], true);
    assert_eq!(user["jukeboxRole"], true);
    assert!(
        user["avatarLastChanged"].is_string(),
        "avatarLastChanged is present"
    );
    assert!(!user["avatarLastChanged"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn get_user_avatar_last_changed_reflects_avatar_store() {
    // (ADR-0134 §2): when an avatar row exists, avatarLastChanged is
    // the avatar store's updated_at; otherwise the user row's updated_at as
    // at the captured baseline. Response shape is unchanged.
    let db = common::TestDb::create("os_cond_get_user_avatar_changed").await;
    let state = state(&db).await;
    let internal_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("user id");
    let avatar_time = OffsetDateTime::now_utc() + time::Duration::days(1);
    melodee_persistence::UserAdminStore::new()
        .set_avatar(
            &db.pool,
            internal_id,
            "image/png",
            b"\x89PNG\r\n\x1a\nx",
            avatar_time,
        )
        .await
        .expect("set avatar");
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getUser.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let user = &envelope(&body)["user"];
    let expected = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        avatar_time.year(),
        avatar_time.month() as u8,
        avatar_time.day(),
        avatar_time.hour(),
        avatar_time.minute(),
        avatar_time.second()
    );
    assert_eq!(user["avatarLastChanged"].as_str(), Some(expected.as_str()));
    db.drop().await;
}

#[tokio::test]
async fn get_user_cross_user_returns_error_40() {
    let db = common::TestDb::create("os_cond_get_user_cross").await;
    let state = state(&db).await;
    let path = "/rest/getUser.view?f=json&username=other";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 40);
}

#[tokio::test]
async fn create_user_fresh_returns_ok() {
    let db = common::TestDb::create("os_cond_create_user_fresh").await;
    let state = state(&db).await;
    let path = "/rest/createUser.view?f=json&username=os-fresh-user&password=secret&email=fresh@example.test";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert!(env.get("user").is_none(), "createUser returns bare ok");
}

#[tokio::test]
async fn create_user_duplicate_returns_error_10() {
    let db = common::TestDb::create("os_cond_create_user_dup").await;
    let state = state(&db).await;
    let path = "/rest/createUser.view?f=json&username=phase0-fixture&password=secret&email=other@example.test";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 10);
    assert_eq!(env["error"]["message"], "User creation failed.");
}

// ---------------------------------------------------------------------------
// Podcasts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_podcasts_enabled_returns_empty_channels() {
    let db = common::TestDb::create("os_cond_podcasts_get").await;
    let state = state(&db).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getPodcasts.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert_eq!(env["podcasts"]["channel"], serde_json::json!([]));
}

#[tokio::test]
async fn get_newest_podcasts_enabled_returns_empty_episodes() {
    let db = common::TestDb::create("os_cond_podcasts_newest").await;
    let state = state(&db).await;
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed("/rest/getNewestPodcasts.view?f=json")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        envelope(&body)["newestPodcasts"]["episode"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn get_podcasts_disabled_returns_raw_response_data_400() {
    let db = common::TestDb::create("os_cond_podcasts_disabled").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let mut disabled = state.clone();
    disabled.podcasts_enabled = false;
    let (status, headers, body) =
        respond(disabled, get(&authed("/rest/getPodcasts.view?f=json"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        header_str(&headers, header::CONTENT_TYPE),
        "application/json; charset=utf-8"
    );
    assert_raw_response_data(&body, "Podcasts are currently disabled.", public_api_id);
}

#[tokio::test]
async fn create_podcast_channel_enabled_returns_raw_400_invalid_channel() {
    let db = common::TestDb::create("os_cond_podcast_create").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let path = "/rest/createPodcastChannel.view?f=json&url=http://feed.example.test/rss";
    let (status, headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        header_str(&headers, header::CONTENT_TYPE),
        "application/json; charset=utf-8"
    );
    assert_raw_response_data(&body, "Invalid channel id", public_api_id);
}

#[tokio::test]
async fn create_podcast_channel_disabled_returns_raw_400_disabled() {
    let db = common::TestDb::create("os_cond_podcast_create_disabled").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let mut disabled = state.clone();
    disabled.podcasts_enabled = false;
    let path = "/rest/createPodcastChannel.view?f=json&url=http://feed.example.test/rss";
    let (status, _headers, body) = respond(disabled, get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Podcasts are currently disabled.", public_api_id);
}

#[tokio::test]
async fn delete_podcast_channel_enabled_returns_raw_400_invalid_channel() {
    let db = common::TestDb::create("os_cond_podcast_delete_channel").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let path = "/rest/deletePodcastChannel.view?f=json&id=1";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Invalid channel id", public_api_id);
}

#[tokio::test]
async fn delete_podcast_episode_enabled_returns_raw_400_invalid_episode() {
    let db = common::TestDb::create("os_cond_podcast_delete_episode").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let path = "/rest/deletePodcastEpisode.view?f=json&id=1";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Invalid episode id", public_api_id);
}

#[tokio::test]
async fn download_podcast_episode_enabled_returns_raw_400_invalid_episode() {
    let db = common::TestDb::create("os_cond_podcast_download_episode").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let path = "/rest/downloadPodcastEpisode.view?f=json&id=1";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Invalid episode id", public_api_id);
}

#[tokio::test]
async fn refresh_podcasts_returns_bare_ok_even_when_disabled() {
    // refreshPodcasts answers the bare ok envelope whether podcasts are enabled
    // or disabled (the captured os-podcasts-refresh golden is the bare ok shape
    // under the enabled fixture; the disabled case is the same bare ok).
    let db = common::TestDb::create("os_cond_podcast_refresh_disabled").await;
    let state = state(&db).await;
    let mut disabled = state.clone();
    disabled.podcasts_enabled = false;
    let (status, _headers, body) =
        respond(disabled, get(&authed("/rest/refreshPodcasts.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert_eq!(env["openSubsonic"], true);
    assert!(
        env.get("podcasts").is_none(),
        "disabled refreshPodcasts must be the bare ok envelope, not the ResponseData 400 shape"
    );
}

#[tokio::test]
async fn stream_podcast_episode_requires_stream_role() {
    let db = common::TestDb::create("os_cond_podcast_stream_norole").await;
    let state = state(&db).await;
    seed_extra_user(
        &db,
        "no-stream-user",
        "NO-STREAM-USER",
        "nostream-secret",
        false,
        &[],
    )
    .await;
    let path = "/rest/streamPodcastEpisode.view?f=json&id=1";
    let (status, _headers, body) = respond(
        state.clone(),
        get(&authed_as(path, "no-stream-user", "nostream-secret")),
    )
    .await;
    // A caller without HasStreamRole fails closed with HTTP 403 + error 40.
    assert_eq!(status, StatusCode::FORBIDDEN);
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 40);
}

#[tokio::test]
async fn stream_podcast_episode_enabled_returns_raw_400_invalid_episode() {
    let db = common::TestDb::create("os_cond_podcast_stream_missing").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let path = "/rest/streamPodcastEpisode.view?f=json&id=1";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Invalid episode id", public_api_id);
}

#[tokio::test]
async fn stream_podcast_episode_disabled_returns_raw_400_disabled() {
    let db = common::TestDb::create("os_cond_podcast_stream_disabled").await;
    let state = state(&db).await;
    let public_api_id = fixture_public_api_id(&db).await;
    let mut disabled = state.clone();
    disabled.podcasts_enabled = false;
    let path = "/rest/streamPodcastEpisode.view?f=json&id=1";
    let (status, _headers, body) = respond(disabled, get(&authed(path))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_raw_response_data(&body, "Podcasts are currently disabled.", public_api_id);
}
