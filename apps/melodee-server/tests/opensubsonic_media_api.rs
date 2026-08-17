//! OpenSubsonic media endpoint compatibility evidence (Phase 8).
//!
//! Asserts the captured stream/download/coverArt/avatar/lyrics contracts and
//! the `/images/{id}/{size}` bypass route (PRD-API-OS-004, PRD-STREAM-002,
//! ADR-0130 D2 single-pass range fix; golden evidence
//! `compatibility/opensubsonic/golden/media`).
//!
//! Behaviors asserted:
//! - **stream** full GET/HEAD headers + body, single-pass ranges (including the
//!   D2 regression `bytes=100000-101023` → 206 with the correct bytes, NOT the
//!   legacy 416/one-byte corruption), invalid/unsatisfiable range + timeOffset
//!   → 404 + error 70, unknown song id → credential-free 302. Configured
//!   transcoding honors `format`/`maxBitRate` per ADR-0147 (superseding the
//!   frozen silent-ignore clause): supported formats stream FFmpeg output,
//!   unsupported/disabled/unavailable templates answer the error-0 envelope,
//!   and non-triggered requests keep byte-identical direct play.
//! - **download** 200 + `Content-Disposition: attachment`, gated by
//!   `downloads_enabled`; unknown → credential-free 302.
//! - **getCoverArt** catalog overrides → stored artwork (a Song uses its
//!   Release override), catalog entities without artwork → captured default,
//!   playlist id → stored artwork with sniffed/stored content-type + ETag +
//!   304 on match.
//! - **getAvatar** by username and by `user_<id>`, 404 when absent.
//! - **getLyrics/getLyricsBySongId** captured baseline shape (`.lrc` sidecars
//!   are intentionally not parsed at this baseline; getLyrics echoes the
//!   artist/title with an empty value, getLyricsBySongId returns bare ok).
//! - `/images/{id}/{size}` serves playlist artwork with no auth.

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
    http::{HeaderMap, HeaderName, Method, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{
    AuthStore, CatalogImageAudit, CatalogImageEntity, CatalogImageStore, NativeUserImport,
};
use melodee_server::native_stream::{CanonicalAssetRoot, DirectPlayService};
use melodee_server::opensubsonic::{MusicFolder, OsState};
use tempfile::TempDir;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const FIXTURE_SECRET: &str = "fixture-os-secret-NOT-A-REAL-SECRET";
const SERVER_VERSION: &str = "0.0.0-test";
const PUBLIC_ORIGIN: &str = "http://melodee.test";

/// PCM payload length for the synthetic WAV fixture; total file size is
/// `44 + 220_500 = 220_544` bytes (mirrors the captured stream fixture).
const WAV_DATA_SIZE: usize = 220_500;
const WAV_TOTAL_SIZE: usize = 44 + WAV_DATA_SIZE;

fn music_folders() -> Vec<std::sync::Arc<MusicFolder>> {
    vec![std::sync::Arc::new(MusicFolder {
        id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
        name: "Phase0 Fixture Library".to_string(),
    })]
}

/// Imports the fixture admin user (Administrator + HasStreamRole) and seeds the
/// OpenSubsonic secret used for the `t`/`s` token contract.
async fn seed_fixture_user(db: &common::TestDb) {
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

async fn state(db: &common::TestDb) -> OsState {
    seed_fixture_user(db).await;
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

fn build_request(method: Method, path: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
    request
}

fn get(path: &str) -> Request<Body> {
    build_request(Method::GET, path, &[])
}

fn head(path: &str) -> Request<Body> {
    build_request(Method::HEAD, path, &[])
}

fn range_get(path: &str, range: &str) -> Request<Body> {
    build_request(Method::GET, path, &[("range", range)])
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

fn header_str(headers: &HeaderMap, name: HeaderName) -> String {
    headers
        .get(name)
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

fn assert_header(headers: &HeaderMap, name: HeaderName, expected: &str) {
    assert_eq!(
        header_str(headers, name.clone()),
        expected,
        "header {name:?}"
    );
}

fn assert_stream_headers(headers: &HeaderMap) {
    assert_header(headers, header::CONTENT_TYPE, "audio/wav");
    assert_header(headers, header::ACCEPT_RANGES, "bytes");
    assert_header(
        headers,
        header::CACHE_CONTROL,
        "no-store, must-revalidate, no-cache, max-age=0",
    );
    assert_header(headers, header::EXPIRES, "Mon, 01 Jan 1990 00:00:00 GMT");
    assert_header(headers, header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    assert_header(headers, HeaderName::from_static("content-duration"), "2.5");
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds a minimal RIFF/WAVE file (44-byte header + PCM payload) whose bytes
/// are deterministic so range slices can be compared exactly.
fn wav_bytes(data_size: usize) -> Vec<u8> {
    let data_size_u32 = u32::try_from(data_size).expect("wav data fits u32");
    let riff_size = 36u32
        .checked_add(data_size_u32)
        .expect("riff size fits u32");
    let mut out = Vec::with_capacity(WAV_TOTAL_SIZE);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&44_100u32.to_le_bytes());
    out.extend_from_slice(&44_100u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size_u32.to_le_bytes());
    let pattern: Vec<u8> = (0u8..=255).collect();
    for index in 0..data_size {
        out.push(pattern[index % 256]);
    }
    out
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

/// Writes the synthetic WAV into a temp canonical asset root and repoints the
/// seeded track's media asset at it so `DirectPlayService` can stream it. The
/// returned `TempDir` must outlive all requests in the test.
async fn setup_media(
    db: &common::TestDb,
    ordinal: u32,
    title: &str,
) -> (OsState, TempDir, Vec<u8>, Uuid) {
    let state = state(db).await;
    let track = seed_track(db, ordinal, title).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let rel = "media/track.wav";
    std::fs::create_dir_all(temp.path().join("media")).expect("mkdir");
    let wav = wav_bytes(WAV_DATA_SIZE);
    std::fs::write(temp.path().join(rel), &wav).expect("write wav");
    let byte_size = i64::try_from(wav.len()).expect("wav size fits i64");
    sqlx::query(
        "UPDATE media_assets SET canonical_relative_path = $2, byte_size = $3, \
         mime_type = 'audio/wav', duration_ms = 2500 \
         WHERE id = (SELECT playable_media_asset_id FROM tracks WHERE public_api_id = $1)",
    )
    .bind(track)
    .bind(rel)
    .bind(byte_size)
    .execute(&db.pool)
    .await
    .expect("attach media asset");
    sqlx::query(
        "UPDATE tracks SET source_path = 'Northern Lights/01 - Solar Wind.wav' WHERE public_api_id = $1",
    )
    .bind(track)
    .execute(&db.pool)
    .await
    .expect("set source path");
    let root = CanonicalAssetRoot::open(temp.path()).expect("canonical asset root");
    let service = DirectPlayService::new(root, 4).expect("direct play service");
    let mut state = state;
    state.direct_play = Some(service);
    (state, temp, wav, track)
}

fn stream_path(track: Uuid) -> String {
    format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}")
}

fn download_path(track: Uuid) -> String {
    format!("/rest/download.view?f=json&v=1.16.1&c=phase0&id=song_{track}")
}

#[tokio::test]
async fn stream_full_get_returns_captured_headers_and_body() {
    let db = common::TestDb::create("os_media_stream_full_get").await;
    let (state, _temp, wav, track) = setup_media(&db, 9001, "Solar Wind").await;
    let (status, headers, body) = respond(state.clone(), get(&authed(&stream_path(track)))).await;
    assert_eq!(status, StatusCode::OK);
    assert_stream_headers(&headers);
    assert_header(
        &headers,
        header::CONTENT_LENGTH,
        WAV_TOTAL_SIZE.to_string().as_str(),
    );
    assert_eq!(body, wav);
}

#[tokio::test]
async fn stream_head_returns_same_headers_with_empty_body() {
    let db = common::TestDb::create("os_media_stream_head").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9002, "Solar Wind").await;
    let (status, headers, body) = respond(state.clone(), head(&authed(&stream_path(track)))).await;
    assert_eq!(status, StatusCode::OK);
    assert_stream_headers(&headers);
    assert_header(
        &headers,
        header::CONTENT_LENGTH,
        WAV_TOTAL_SIZE.to_string().as_str(),
    );
    assert!(body.is_empty(), "HEAD body must be empty");
}

#[tokio::test]
async fn stream_initial_range_returns_206_with_exact_bytes() {
    let db = common::TestDb::create("os_media_stream_range_start").await;
    let (state, _temp, wav, track) = setup_media(&db, 9003, "Solar Wind").await;
    let (status, headers, body) = respond(
        state.clone(),
        range_get(&authed(&stream_path(track)), "bytes=0-1023"),
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_header(&headers, header::CONTENT_RANGE, "bytes 0-1023/220544");
    assert_header(&headers, header::CONTENT_LENGTH, "1024");
    assert_eq!(body, &wav[0..1024]);
}

#[tokio::test]
async fn stream_middle_range_d2_regression_returns_206_not_legacy_416() {
    // D2 (ADR-0130): the legacy double-range bug answered `bytes=100000-101023`
    // for a 220_544-byte asset with HTTP 416 + `Content-Range: bytes */1024`
    // and a one-byte corrupted body. The single-pass Rust fix returns 206 with
    // the correct 1024 bytes and the correct total size.
    let db = common::TestDb::create("os_media_stream_range_d2").await;
    let (state, _temp, wav, track) = setup_media(&db, 9004, "Solar Wind").await;
    let (status, headers, body) = respond(
        state.clone(),
        range_get(&authed(&stream_path(track)), "bytes=100000-101023"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PARTIAL_CONTENT,
        "D2 regression: middle range must be 206, not the legacy 416"
    );
    assert_header(
        &headers,
        header::CONTENT_RANGE,
        "bytes 100000-101023/220544",
    );
    assert_header(&headers, header::CONTENT_LENGTH, "1024");
    assert_eq!(body, &wav[100_000..101_024]);
}

#[tokio::test]
async fn stream_invalid_range_returns_404_error_70() {
    let db = common::TestDb::create("os_media_stream_range_invalid").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9005, "Solar Wind").await;
    let (status, headers, body) = respond(
        state.clone(),
        range_get(&authed(&stream_path(track)), "bytes=abc-def"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_header(
        &headers,
        header::CONTENT_TYPE,
        "application/json; charset=utf-8",
    );
    let env = envelope(&body);
    assert_eq!(env["status"], "error");
    assert_eq!(env["error"]["code"], 70);
    assert_eq!(env["error"]["message"], "The requested data was not found.");
}

#[tokio::test]
async fn stream_unsatisfiable_range_returns_404_error_70() {
    let db = common::TestDb::create("os_media_stream_range_unsat").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9006, "Solar Wind").await;
    let (status, _headers, body) = respond(
        state.clone(),
        range_get(&authed(&stream_path(track)), "bytes=999999999-"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let env = envelope(&body);
    assert_eq!(env["error"]["code"], 70);
}

#[tokio::test]
async fn stream_time_offset_returns_404_error_70() {
    let db = common::TestDb::create("os_media_stream_timeoffset").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9007, "Solar Wind").await;
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&timeOffset=1");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let env = envelope(&body);
    assert_eq!(env["error"]["code"], 70);
}

#[tokio::test]
async fn stream_unknown_song_redirects_credential_free_302() {
    let db = common::TestDb::create("os_media_stream_unknown").await;
    let (state, _temp, _wav, _track) = setup_media(&db, 9008, "Solar Wind").await;
    let path =
        "/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_00000000-0000-0000-0000-000000000000";
    let (status, headers, _body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = header_str(&headers, header::LOCATION);
    assert!(
        location.contains("/Account/Login?ReturnUrl=%2FError"),
        "credential-free 302 target: {location}"
    );
    assert!(!location.contains("t="), "no token in location: {location}");
    assert!(!location.contains("s="), "no salt in location: {location}");
    assert!(!location.contains("u="), "no user in location: {location}");
}

#[tokio::test]
async fn stream_format_raw_and_high_max_bitrate_stays_direct_play() {
    let db = common::TestDb::create("os_media_stream_maxbitrate").await;
    let (state, _temp, wav, track) = setup_media(&db, 9009, "Solar Wind").await;
    // format=raw with a bitrate cap above the source (705 kbps) keeps the
    // captured direct-play response byte-identical (ADR-0147 decision 3).
    let path = format!(
        "/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&maxBitRate=5000&format=raw"
    );
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(
        &headers,
        header::CONTENT_LENGTH,
        WAV_TOTAL_SIZE.to_string().as_str(),
    );
    assert_header(&headers, header::CONTENT_TYPE, "audio/wav");
    assert_eq!(body, wav);
}

/// Attaches the shared transcoding service to a media fixture state.
fn attach_transcoding(state: &mut OsState, temp: &TempDir, db: &common::TestDb, max: usize) {
    let root = CanonicalAssetRoot::open(temp.path()).expect("canonical asset root");
    state.transcoding = Some(
        melodee_server::transcode::TranscodingService::new(root, db.pool.clone(), max)
            .expect("transcoding service"),
    );
}

/// Asserts the ADR-0147 error-0 envelope (HTTP 200, captured `status:
/// "error"` serialization with `error.code == 0`).
fn assert_error0_envelope(body: &[u8]) {
    let json: serde_json::Value = serde_json::from_slice(body).expect("json envelope");
    assert_eq!(json["subsonic-response"]["status"], "error");
    assert_eq!(json["subsonic-response"]["error"]["code"], 0);
}

fn assert_mp3_bytes(body: &[u8]) {
    assert!(!body.is_empty(), "transcoded body is non-empty");
    assert!(
        body.starts_with(b"ID3") || (body.len() > 2 && body[0] == 0xFF && body[1] & 0xE0 == 0xE0),
        "body starts with an ID3 tag or MP3 frame sync"
    );
}

#[tokio::test]
async fn stream_format_mp3_transcodes_with_configured_template() {
    let db = common::TestDb::create("os_media_stream_transcode_mp3").await;
    let (mut state, temp, wav, track) = setup_media(&db, 9020, "Solar Wind").await;
    attach_transcoding(&mut state, &temp, &db, 2);
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=mp3");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "audio/mpeg");
    // ADR-0147: no Content-Length/ETag on transcoded responses; the direct
    // play no-store/CORS/Content-Duration headers are preserved.
    assert!(!headers.contains_key(header::CONTENT_LENGTH));
    assert!(!headers.contains_key(header::ETAG));
    assert_header(
        &headers,
        header::CACHE_CONTROL,
        "no-store, must-revalidate, no-cache, max-age=0",
    );
    assert!(headers.contains_key(HeaderName::from_static("content-duration")));
    assert_mp3_bytes(&body);
    assert_ne!(body, wav);
}

#[tokio::test]
async fn stream_max_bitrate_below_source_transcodes_mp3_at_cap() {
    let db = common::TestDb::create("os_media_stream_bitrate_cap").await;
    let (mut state, temp, _wav, track) = setup_media(&db, 9021, "Solar Wind").await;
    attach_transcoding(&mut state, &temp, &db, 2);
    // maxBitRate alone (64 kbps) below the 705 kbps source → mp3 transcode.
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&maxBitRate=64");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "audio/mpeg");
    assert_mp3_bytes(&body);
}

#[tokio::test]
async fn stream_range_header_is_ignored_on_transcoded_requests() {
    let db = common::TestDb::create("os_media_stream_transcode_range").await;
    let (mut state, temp, _wav, track) = setup_media(&db, 9022, "Solar Wind").await;
    attach_transcoding(&mut state, &temp, &db, 2);
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=mp3");
    let request = Request::builder()
        .method("GET")
        .uri(authed(&path))
        .header(header::RANGE, "bytes=0-99")
        .body(Body::empty())
        .expect("request");
    let (status, headers, body) = respond(state.clone(), request).await;
    // Range is ignored: 200 (never 206), no Content-Range, full transcode.
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::CONTENT_RANGE));
    assert_mp3_bytes(&body);
}

#[tokio::test]
async fn stream_unsupported_format_returns_error0_envelope() {
    let db = common::TestDb::create("os_media_stream_bad_format").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9023, "Solar Wind").await;
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=flac");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_error0_envelope(&body);
}

#[tokio::test]
async fn stream_transcode_rejected_when_transcoding_disabled() {
    let db = common::TestDb::create("os_media_stream_transcode_off").await;
    // Plain fixture state has no transcoding service (maxConcurrent=0 shape).
    let (state, _temp, _wav, track) = setup_media(&db, 9024, "Solar Wind").await;
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=mp3");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_error0_envelope(&body);
}

#[tokio::test]
async fn stream_malformed_template_fails_closed() {
    let db = common::TestDb::create("os_media_stream_bad_template").await;
    let (mut state, temp, _wav, track) = setup_media(&db, 9025, "Solar Wind").await;
    attach_transcoding(&mut state, &temp, &db, 2);
    // The malformed legacy seed shape (ADR-0108/ADR-0147): unparseable
    // template must fail closed with the error-0 envelope.
    melodee_persistence::SettingsStore::new()
        .set(
            &db.pool,
            "transcoding.command.mp3",
            &serde_json::Value::String(
                "{ 'format': 'Mp3', 'bitrate: 192, 'command': 'ffmpeg -i %s -f mp3 -' }"
                    .to_string(),
            ),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("malformed override");
    let path = format!("/rest/stream.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=mp3");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_error0_envelope(&body);
}

#[tokio::test]
async fn download_format_aac_transcodes_and_swaps_attachment_extension() {
    let db = common::TestDb::create("os_media_download_transcode").await;
    let (mut state, temp, _wav, track) = setup_media(&db, 9026, "Solar Wind").await;
    attach_transcoding(&mut state, &temp, &db, 2);
    let path = format!("/rest/download.view?f=json&v=1.16.1&c=phase0&id=song_{track}&format=aac");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "audio/aac");
    assert_eq!(
        header_str(&headers, header::CONTENT_DISPOSITION),
        "attachment; filename=\"01 - Solar Wind.aac\"; filename*=UTF-8''01%20-%20Solar%20Wind.aac"
    );
    // ADTS frames carry the 0xFFF sync word.
    assert!(!body.is_empty());
    assert_eq!(body[0], 0xFF);
    assert_eq!(body[1] & 0xF6, 0xF0, "ADTS sync word");
}

#[tokio::test]
async fn download_returns_attachment_with_captured_disposition() {
    let db = common::TestDb::create("os_media_download").await;
    let (state, _temp, wav, track) = setup_media(&db, 9010, "Solar Wind").await;
    let (status, headers, body) = respond(state.clone(), get(&authed(&download_path(track)))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "audio/wav");
    assert_header(
        &headers,
        header::CONTENT_LENGTH,
        WAV_TOTAL_SIZE.to_string().as_str(),
    );
    assert_eq!(
        header_str(&headers, header::CONTENT_DISPOSITION),
        "attachment; filename=\"01 - Solar Wind.wav\"; filename*=UTF-8''01%20-%20Solar%20Wind.wav"
    );
    assert_eq!(body, wav);
}

#[tokio::test]
async fn download_unknown_redirects_credential_free_302() {
    let db = common::TestDb::create("os_media_download_unknown").await;
    let (state, _temp, _wav, _track) = setup_media(&db, 9011, "Solar Wind").await;
    let path =
        "/rest/download.view?f=json&v=1.16.1&c=phase0&id=song_00000000-0000-0000-0000-000000000000";
    let (status, headers, _body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(header_str(&headers, header::LOCATION).contains("/Account/Login?ReturnUrl=%2FError"));
}

#[tokio::test]
async fn download_disabled_redirects_credential_free_302() {
    let db = common::TestDb::create("os_media_download_disabled").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9012, "Solar Wind").await;
    let mut disabled = state.clone();
    disabled.downloads_enabled = false;
    let (status, headers, _body) = respond(disabled, get(&authed(&download_path(track)))).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(header_str(&headers, header::LOCATION).contains("/Account/Login?ReturnUrl=%2FError"));
}

#[tokio::test]
async fn get_cover_art_song_serves_the_captured_default_when_no_artwork() {
    // Frozen `os-getcoverart-song`: a sized request for a song without stored
    // artwork is served the processed default JPEG (785 bytes).
    let db = common::TestDb::create("os_media_coverart_song").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9013, "Solar Wind").await;
    let path = format!("/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id=song_{track}&size=300");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/jpeg");
    assert_eq!(body.len(), 785, "processed default artwork bytes");
}

#[tokio::test]
async fn get_cover_art_catalog_overrides_are_served_for_artist_release_and_song() {
    let db = common::TestDb::create("os_media_coverart_catalog_override").await;
    let state = state(&db).await;
    let track = seed_track(&db, 90131, "Override Probe").await;
    let (release_id, release_public_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT release.id, release.public_api_id
         FROM tracks track JOIN releases release ON release.id = track.release_id
         WHERE track.public_api_id = $1",
    )
    .bind(track)
    .fetch_one(&db.pool)
    .await
    .expect("release ids");
    let (artist_id, artist_public_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT artist.id, artist.public_api_id
         FROM release_artists billing JOIN artists artist ON artist.id = billing.artist_id
         WHERE billing.release_id = $1 AND billing.role = 'primary'",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("artist ids");
    let release_artwork = b"\xff\xd8\xff\xe0catalog-release-override".to_vec();
    let artist_artwork = b"\x89PNG\r\n\x1a\ncatalog-artist-override".to_vec();
    let audit = CatalogImageAudit {
        actor: "os-media-test",
        reason: "verify adapter artwork precedence",
        now: OffsetDateTime::now_utc(),
    };
    CatalogImageStore::set_image(
        &db.pool,
        CatalogImageEntity::Release,
        release_id,
        "image/jpeg",
        &release_artwork,
        audit,
    )
    .await
    .expect("store release override");
    CatalogImageStore::set_image(
        &db.pool,
        CatalogImageEntity::Artist,
        artist_id,
        "image/png",
        &artist_artwork,
        audit,
    )
    .await
    .expect("store artist override");

    for id in [
        format!("album_{release_public_id}"),
        format!("song_{track}"),
    ] {
        let path = format!("/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id={id}");
        let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_header(&headers, header::CONTENT_TYPE, "image/jpeg");
        assert_header(&headers, header::CACHE_CONTROL, "public, max-age=86400");
        assert_eq!(body, release_artwork, "{id}");
    }

    let path =
        format!("/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id=artist_{artist_public_id}");
    let (status, headers, body) = respond(state, get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "image/png");
    assert_eq!(body, artist_artwork);
}

#[tokio::test]
async fn get_cover_art_unknown_id_returns_200_empty() {
    let db = common::TestDb::create("os_media_coverart_unknown").await;
    let state = state(&db).await;
    let path = "/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id=album_00000000-0000-0000-0000-000000000000";
    let (status, headers, body) = respond(state.clone(), get(&authed(path))).await;
    // Captured os-getcoverart-unknown: 200 with an empty body.
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_TYPE).is_none());
    assert!(body.is_empty());
}

/// Creates a playlist through the public API and stores artwork bytes on it.
/// Reuses the caller's `state` (which has already seeded the fixture user) so
/// the user is imported exactly once per test.
async fn seed_playlist_with_artwork(db: &common::TestDb, state: &OsState, jpeg: &[u8]) -> String {
    let track = seed_track(db, 9050, "Cover Probe").await;
    let create = format!("/rest/createPlaylist.view?f=json&name=ArtworkProbe&songId=song_{track}");
    let (_, _, body) = respond(state.clone(), get(&authed(&create))).await;
    let wire_id = envelope(&body)["playlist"]["id"]
        .as_str()
        .expect("playlist id")
        .to_string();
    let public_id = wire_id
        .strip_prefix("playlist_")
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .expect("playlist public id");
    sqlx::query(
        "UPDATE native_playlists SET image_mime_type = 'image/jpeg', image_bytes = $2 \
         WHERE public_api_id = $1",
    )
    .bind(public_id)
    .bind(jpeg)
    .execute(&db.pool)
    .await
    .expect("seed playlist artwork");
    wire_id
}

#[tokio::test]
async fn get_cover_art_playlist_returns_artwork_with_etag() {
    let db = common::TestDb::create("os_media_coverart_playlist").await;
    let state = state(&db).await;
    let jpeg = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00cover-artwork-probe".to_vec();
    let wire_id = seed_playlist_with_artwork(&db, &state, &jpeg).await;
    let path = format!("/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id={wire_id}");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "image/jpeg");
    assert_header(&headers, header::CACHE_CONTROL, "public, max-age=86400");
    assert_eq!(
        header_str(&headers, header::ETAG),
        format!("\"{}\"", sha256_hex(&jpeg))
    );
    assert_eq!(body, jpeg);
}

#[tokio::test]
async fn get_cover_art_if_none_match_returns_304() {
    let db = common::TestDb::create("os_media_coverart_304").await;
    let state = state(&db).await;
    let jpeg = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00cover-artwork-probe".to_vec();
    let wire_id = seed_playlist_with_artwork(&db, &state, &jpeg).await;
    let etag = format!("\"{}\"", sha256_hex(&jpeg));
    let path = format!("/rest/getCoverArt.view?f=json&v=1.16.1&c=phase0&id={wire_id}");
    let request = build_request(Method::GET, &authed(&path), &[("if-none-match", &etag)]);
    let (status, headers, body) = respond(state.clone(), request).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(header_str(&headers, header::ETAG), etag);
    assert!(body.is_empty());
}

#[tokio::test]
async fn get_avatar_missing_serves_the_captured_default() {
    // Frozen `os-getavatar`: a user without stored avatar bytes is served the
    // legacy default avatar image (image/png), not a 404.
    let db = common::TestDb::create("os_media_avatar_missing").await;
    let state = state(&db).await;
    let path = "/rest/getAvatar.view?f=json&v=1.16.1&c=phase0&username=phase0-fixture";
    let (status, headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/png");
    assert_eq!(body.len(), 2_271_384, "default avatar bytes");
}

#[tokio::test]
async fn get_avatar_by_username_returns_stored_bytes() {
    let db = common::TestDb::create("os_media_avatar_username").await;
    let state = state(&db).await;
    let avatar = b"\x89PNG\r\n\x1a\nprobe-avatar-bytes".to_vec();
    sqlx::query(
        "UPDATE native_users SET image_mime_type = 'image/png', image_bytes = $1 \
         WHERE normalized_username = 'PHASE0-FIXTURE'",
    )
    .bind(&avatar)
    .execute(&db.pool)
    .await
    .expect("seed avatar");
    let path = "/rest/getAvatar.view?f=json&v=1.16.1&c=phase0&username=phase0-fixture";
    let (status, headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "image/png");
    assert_eq!(body, avatar);
}

#[tokio::test]
async fn get_avatar_by_user_id_returns_stored_bytes() {
    let db = common::TestDb::create("os_media_avatar_userid").await;
    let state = state(&db).await;
    let avatar = b"\x89PNG\r\n\x1a\nprobe-avatar-bytes".to_vec();
    let public_api_id = fixture_public_api_id(&db).await;
    sqlx::query(
        "UPDATE native_users SET image_mime_type = 'image/png', image_bytes = $1 \
         WHERE normalized_username = 'PHASE0-FIXTURE'",
    )
    .bind(&avatar)
    .execute(&db.pool)
    .await
    .expect("seed avatar");
    let path = format!("/rest/getAvatar.view?f=json&v=1.16.1&c=phase0&id=user_{public_api_id}");
    let (status, headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "image/png");
    assert_eq!(body, avatar);
}

#[tokio::test]
async fn get_avatar_reads_phase9_avatar_store() {
    // WP-03 (ADR-0134 §2): `native_user_avatars` is the avatar authority; the
    // migration-0010 `native_users` image columns are the legacy-import
    // fallback. The response shape (bytes + content type) is unchanged.
    let db = common::TestDb::create("os_media_avatar_phase9_store").await;
    let state = state(&db).await;
    let internal_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("user id");
    // Legacy-imported bytes remain visible when no new-store row exists.
    let legacy = b"\x89PNG\r\n\x1a\nlegacy-imported-avatar".to_vec();
    sqlx::query(
        "UPDATE native_users SET image_mime_type = 'image/png', image_bytes = $1 WHERE id = $2",
    )
    .bind(&legacy)
    .bind(internal_id)
    .execute(&db.pool)
    .await
    .expect("seed legacy avatar");
    let path = "/rest/getAvatar.view?f=json&v=1.16.1&c=phase0&username=phase0-fixture";
    let (status, _, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, legacy, "legacy column fallback");

    // A new-store row wins over the legacy columns.
    let uploaded = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00wp03-uploaded-avatar".to_vec();
    melodee_persistence::UserAdminStore::new()
        .set_avatar(
            &db.pool,
            internal_id,
            "image/jpeg",
            &uploaded,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("set avatar");
    let (status, headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_header(&headers, header::CONTENT_TYPE, "image/jpeg");
    assert_eq!(body, uploaded, "native_user_avatars row wins");

    // Deleting the avatar row restores the legacy fallback.
    melodee_persistence::UserAdminStore::new()
        .delete_avatar(&db.pool, internal_id)
        .await
        .expect("delete avatar");
    let (status, _, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, legacy, "fallback restored after delete");
    db.drop().await;
}

#[tokio::test]
async fn get_lyrics_echoes_artist_and_title_with_empty_value() {
    let db = common::TestDb::create("os_media_lyrics").await;
    let state = state(&db).await;
    let path =
        "/rest/getLyrics.view?f=json&v=1.16.1&c=phase0&artist=Aurora%20Fields&title=Polar%20Veil";
    let (status, _headers, body) = respond(state.clone(), get(&authed(path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert_eq!(env["lyrics"]["value"], "");
    assert_eq!(env["lyrics"]["artist"], "Aurora Fields");
    assert_eq!(env["lyrics"]["title"], "Polar Veil");
}

#[tokio::test]
async fn get_lyrics_missing_returns_empty_artist_title() {
    let db = common::TestDb::create("os_media_lyrics_missing").await;
    let state = state(&db).await;
    let (status, _headers, body) =
        respond(state.clone(), get(&authed("/rest/getLyrics.view?f=json"))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["lyrics"]["value"], "");
    assert_eq!(env["lyrics"]["artist"], "");
    assert_eq!(env["lyrics"]["title"], "");
}

#[tokio::test]
async fn get_lyrics_by_song_id_returns_bare_ok_envelope() {
    let db = common::TestDb::create("os_media_lyrics_by_songid").await;
    let (state, _temp, _wav, track) = setup_media(&db, 9020, "Solar Wind").await;
    let path = format!("/rest/getLyricsBySongId.view?f=json&v=1.16.1&c=phase0&id=song_{track}");
    let (status, _headers, body) = respond(state.clone(), get(&authed(&path))).await;
    assert_eq!(status, StatusCode::OK);
    let env = envelope(&body);
    assert_eq!(env["status"], "ok");
    assert_eq!(env["openSubsonic"], true);
    assert!(
        env.get("lyrics").is_none(),
        "getLyricsBySongId returns bare ok with no lyrics key at this baseline"
    );
}

// `/images/{id}/{size}` is owned by the native adapter: it is absent from the
// OpenSubsonic route inventory, and the frozen baseline serves the native
// placeholder/empty classes there (see native image captures). The
// OpenSubsonic auth bypass for `/images/` paths is covered by the auth tests.
