//! Phase 9 WP-01 cross-API playlist identity evidence (PRD-PLAYLIST-001/003).
//!
//! All three surfaces share the Phase 7 `native_playlists` /
//! `native_playlist_items` tables, so one playlist row is addressable from
//! every adapter that exposes playlists. The designed ID relationship is a
//! pure re-encoding of the same `public_api_id` UUID:
//!
//! - native v1: the dashed UUID verbatim (`/api/v1/playlists/{uuid}`)
//! - OpenSubsonic: `playlist_` + dashed UUID (`getPlaylist?id=playlist_{uuid}`)
//! - Jellyfin: the N-format (dashless) UUID (`/Playlists/{simple}`)
//!
//! This test pins that mapping plus the one intentional ordering divergence:
//! native and Jellyfin serve items in playlist-position order, while the
//! captured OpenSubsonic contract serves `entry` in Track creation order
//! (legacy `GetPlaylistAsync` never applied `PlaylistOrder`; see
//! `opensubsonic/playlists.rs` and golden `os-playlists-getplaylist`).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport, StoredNativeUser};
use melodee_server::jellyfin::{JfState, SecretPepper};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use melodee_server::opensubsonic::{MusicFolder, OsState};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const OS_SECRET: &str = "cross-api-os-secret-NOT-A-REAL-SECRET";
const JF_PEPPER: &[u8] = b"melodee-jellyfin-cross-api-pepper-NOT-A-REAL-SECRET!";
const USERNAME: &str = "cross-api-user";

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        let binding = client_binding.to_string();
        Box::pin(async move {
            Ok(format!(
                "http://melodee.test/song/stream/{track_public_id}/{user_public_id}/signed-{binding}"
            ))
        })
    }
}

async fn import_user(db: &common::TestDb) -> StoredNativeUser {
    let now = OffsetDateTime::now_utc();
    let roles = vec![
        "Administrator".to_string(),
        "HasStreamRole".to_string(),
        "HasPlaylistRole".to_string(),
    ];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: USERNAME,
                normalized_username: &USERNAME.to_ascii_uppercase(),
                email: "cross-api-user@example.test",
                normalized_email: "CROSS-API-USER@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("fixture hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "cross-api-stream-key",
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
    sqlx::query("INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) VALUES ($1, $2, now(), now())")
        .bind(user.id)
        .bind(OS_SECRET)
        .execute(&db.pool)
        .await
        .expect("seed OpenSubsonic secret");
    user
}

fn native_state(db: &common::TestDb) -> NativeState {
    NativeState {
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://melodee.test".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_string(),
                90,
            )
            .expect("test auth config"),
        ),
    }
}

fn os_state(db: &common::TestDb) -> OsState {
    OsState {
        pool: db.pool.clone(),
        server_version: "0.0.0-test".to_string(),
        public_origin: "http://melodee.test".to_string(),
        base_url_host: Some("capture-fixture.example.test".to_string()),
        music_folders: vec![Arc::new(MusicFolder {
            id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
            name: "Cross API Fixture Library".to_string(),
        })],
        podcasts_enabled: true,
        downloads_enabled: true,
        jukebox_enabled: false,
        direct_play: None,
        transcoding: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
    }
}

fn jf_state(db: &common::TestDb) -> JfState {
    JfState {
        pool: db.pool.clone(),
        server_name: "Melodee".to_string(),
        server_version: "0.0.0-test".to_string(),
        server_id: "93cd3cc3a120127ccfda124620290cae".to_string(),
        local_address: "http://melodee.test".to_string(),
        public_origin: "http://melodee.test".to_string(),
        token_pepper: SecretPepper::new(JF_PEPPER.to_vec()).expect("valid fixture pepper"),
        token_expiry_hours: 168,
        max_active_tokens: 10,
        direct_play: None,
        transcoding: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
        libraries: Vec::new(),
    }
}

async fn native_call(
    state: NativeState,
    principal: NativePrincipal,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let bytes = body.map_or_else(Vec::new, |value| serde_json::to_vec(&value).expect("JSON"));
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .expect("request");
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("127.0.0.1".to_string()));
    let response = melodee_server::native::router()
        .with_state(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

fn os_token(salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(OS_SECRET.as_bytes());
    hasher.update(salt.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn os_get(state: OsState, path: &str) -> (StatusCode, serde_json::Value) {
    let salt = path.len().to_string();
    let uri = format!("{path}&u={USERNAME}&t={}&s={salt}", os_token(&salt));
    let mut request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
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
        .to_bytes();
    let envelope = serde_json::from_slice::<serde_json::Value>(&bytes).expect("JSON response")
        ["subsonic-response"]
        .clone();
    (status, envelope)
}

async fn jf_call(state: &JfState, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = melodee_server::jellyfin::configured_router(state.clone())
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

async fn jf_login(state: &JfState) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"Username":"{USERNAME}","Pw":"{FIXTURE_PASSWORD}"}}"#
        )))
        .expect("request");
    let (status, body) = jf_call(state, request).await;
    assert_eq!(status, StatusCode::OK);
    body["AccessToken"]
        .as_str()
        .expect("access token")
        .to_string()
}

fn jf_get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("X-Emby-Token", token)
        .body(Body::empty())
        .expect("request")
}

async fn seed_track(db: &common::TestDb, ordinal: u32, title: &str) -> Uuid {
    phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: "Cross API Artist",
            track_title: title,
            track_artist: "Cross API Artist",
            release_date: "2026-08-13",
        },
    )
    .await;
    sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE title = $1")
        .bind(title)
        .fetch_one(&db.pool)
        .await
        .expect("Track public ID")
}

#[tokio::test]
async fn native_created_playlist_keeps_one_identity_across_all_three_surfaces() {
    let db = common::TestDb::create("server_cross_api_playlist_ids").await;
    // Creation order is alpha, beta, gamma; the playlist position order is the
    // reverse on purpose so the per-surface ordering contract is observable.
    let alpha = seed_track(&db, 9101, "Cross API Alpha").await;
    let beta = seed_track(&db, 9102, "Cross API Beta").await;
    let gamma = seed_track(&db, 9103, "Cross API Gamma").await;
    let user = import_user(&db).await;
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    };

    // Create through the native v1 API only.
    let (status, created) = native_call(
        native_state(&db),
        principal,
        Method::POST,
        "/api/v1/playlists",
        Some(serde_json::json!({
            "name": "Cross API Identity",
            "comment": "one row, three encodings",
            "isPublic": false,
            "songIds": [gamma, alpha, beta]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let native_id = created["id"].as_str().expect("native playlist id");
    let public_api_id = Uuid::parse_str(native_id).expect("native id is a dashed UUID");

    // Native: items in playlist-position order under the dashed UUID.
    let (status, songs) = native_call(
        native_state(&db),
        principal,
        Method::GET,
        &format!("/api/v1/playlists/{native_id}/songs?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        songs["data"]
            .as_array()
            .expect("native songs")
            .iter()
            .map(|song| song["id"].as_str().expect("song id").to_string())
            .collect::<Vec<_>>(),
        vec![gamma.to_string(), alpha.to_string(), beta.to_string()],
        "native serves the requested position order"
    );

    // OpenSubsonic: same row under `playlist_{uuid}`; captured `entry` order
    // follows Track creation order, not position order.
    let os_wire_id = format!("playlist_{public_api_id}");
    let (status, playlist) = os_get(
        os_state(&db),
        &format!("/rest/getPlaylist.view?f=json&id={os_wire_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(playlist["status"], "ok");
    assert_eq!(playlist["playlist"]["id"], os_wire_id);
    assert_eq!(playlist["playlist"]["name"], "Cross API Identity");
    assert_eq!(playlist["playlist"]["songCount"], 3);
    assert_eq!(
        playlist["playlist"]["entry"]
            .as_array()
            .expect("OS entries")
            .iter()
            .map(|entry| entry["id"].as_str().expect("entry id").to_string())
            .collect::<Vec<_>>(),
        vec![
            format!("song_{alpha}"),
            format!("song_{beta}"),
            format!("song_{gamma}")
        ],
        "captured OpenSubsonic contract orders entries by Track creation"
    );
    let (status, list) = os_get(os_state(&db), "/rest/getPlaylists.view?f=json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list["playlists"]["playlist"]
            .as_array()
            .expect("OS playlist list")
            .iter()
            .map(|playlist| playlist["id"].as_str().expect("listed id"))
            .collect::<Vec<_>>(),
        vec![os_wire_id.as_str()],
        "the native-created playlist is the same row in getPlaylists"
    );

    // Jellyfin: same row under the N-format UUID, items in position order.
    let jf_wire_id = public_api_id.simple().to_string();
    let jf_state = jf_state(&db);
    let token = jf_login(&jf_state).await;
    let (status, item) = jf_call(
        &jf_state,
        jf_get(&format!("/Playlists/{jf_wire_id}"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["Id"], jf_wire_id);
    assert_eq!(item["Name"], "Cross API Identity");
    let (status, items) = jf_call(
        &jf_state,
        jf_get(&format!("/Playlists/{jf_wire_id}/Items"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items["TotalRecordCount"], 3);
    assert_eq!(
        items["Items"]
            .as_array()
            .expect("JF items")
            .iter()
            .map(|item| item["Id"].as_str().expect("item id").to_string())
            .collect::<Vec<_>>(),
        vec![
            gamma.simple().to_string(),
            alpha.simple().to_string(),
            beta.simple().to_string()
        ],
        "Jellyfin serves the same position order as native"
    );

    // The designed relationship: one `native_playlists.public_api_id`, three
    // lossless string encodings — no per-adapter ID namespace.
    assert_eq!(os_wire_id, format!("playlist_{public_api_id}"));
    assert_eq!(jf_wire_id, public_api_id.simple().to_string());
    assert_eq!(native_id, public_api_id.to_string());
    let stored: String = sqlx::query_scalar(
        "SELECT public_api_id::text FROM native_playlists WHERE name = 'Cross API Identity'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("stored playlist row");
    assert_eq!(stored, public_api_id.to_string());
    db.drop().await;
}
