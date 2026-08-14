//! Phase 9 WP-03 native avatar routes (PRD-USER-001, ADR-0134 §2):
//! upload validation (magic bytes, declared type, size bounds), round-trip,
//! delete, and authentication enforcement. Also verifies the OpenSubsonic
//! getAvatar read path observes the new `native_user_avatars` store.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{io::Cursor, net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use image::{ImageFormat, ImageReader};
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport, UserAdminStore};
use melodee_server::native::{
    NativeFuture, NativeState, NativeStreamUrlSigner, PostgresNativeUserStateReader,
    auth::NativeAuthService,
};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-avatar-jwt-key-that-is-at-least-32";
const USERNAME: &str = "avatar-user";

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        _track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Err(()) })
    }
}

struct AvatarFixture {
    user_id: Uuid,
    token: String,
}

async fn seed(db: &common::TestDb) -> AvatarFixture {
    let now = OffsetDateTime::now_utc();
    let user_id = Uuid::now_v7();
    let public_api_id = Uuid::now_v7();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: user_id,
                public_api_id,
                username: USERNAME,
                normalized_username: "AVATAR-USER",
                email: "avatar-user@example.test",
                normalized_email: "AVATAR-USER@EXAMPLE.TEST",
                password_hash: &bcrypt::hash("Mel0deeR0cks!", 4).expect("fixture hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
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
        .expect("import user");
    let now_ts = OffsetDateTime::now_utc().unix_timestamp();
    let token = issue_access_token(
        &SecretString::new(JWT_KEY.to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email: "avatar-user@example.test".to_string(),
            exp: now_ts + 900,
            sid: public_api_id,
            iat: now_ts,
            iss: "MelodeeApi".to_string(),
            nbf: now_ts,
            role: vec!["HasStreamRole".to_string()],
            unique_name: USERNAME.to_string(),
            theme: String::new(),
        },
    )
    .expect("issue token");
    AvatarFixture { user_id, token }
}

fn state(db: &common::TestDb) -> NativeState {
    NativeState {
        pool: db.pool.clone(),
        search: None,
        public_origin: "http://melodee.test".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(NativeAuthService::new(JWT_KEY.to_string(), 90).expect("auth config")),
    }
}

async fn call(
    state: NativeState,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    bearer: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let mut request = builder.body(Body::from(body)).expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let response = melodee_server::native::configured_router(state)
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

/// Encodes a deterministic 64x64 RGBA PNG.
fn png_64x64() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        64,
        image::Rgba([17, 34, 51, 255]),
    ));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::Png).expect("png");
    cursor.into_inner()
}

fn decode_dimensions(bytes: &[u8]) -> (u32, u32) {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .expect("format")
        .into_dimensions()
        .expect("dimensions")
}

#[tokio::test]
async fn avatar_upload_get_delete_round_trip() {
    let db = common::TestDb::create("wp03_avatar_roundtrip").await;
    let fixture = seed(&db).await;
    let state = state(&db);

    // Nothing stored yet.
    let (status, _, _) = call(
        state.clone(),
        "GET",
        "/api/v1/user/me/avatar",
        None,
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Upload a valid PNG.
    let png = png_64x64();
    let (status, _, body) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/png"),
        png,
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json["updatedAt"].as_str().is_some());

    // The store row carries the re-encoded bytes and timestamp.
    let stored = UserAdminStore::new()
        .avatar_by_user_id(&db.pool, fixture.user_id)
        .await
        .expect("avatar lookup")
        .expect("avatar row");
    assert_eq!(stored.content_type, "image/png");
    assert!(stored.image.len() <= 1024 * 1024);

    // GET serves the stored bytes with the stored content type.
    let (status, headers, body) = call(
        state.clone(),
        "GET",
        "/api/v1/user/me/avatar",
        None,
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).expect("content type"),
        "image/png"
    );
    assert_eq!(decode_dimensions(&body), (64, 64));
    let etag = headers
        .get(header::ETAG)
        .expect("etag")
        .to_str()
        .expect("etag str")
        .to_string();

    // ETag revalidation short-circuits.
    let builder = Request::builder()
        .method("GET")
        .uri("/api/v1/user/me/avatar")
        .header(header::AUTHORIZATION, format!("Bearer {}", fixture.token))
        .header(header::IF_NONE_MATCH, &etag);
    let mut request = builder.body(Body::empty()).expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let response = melodee_server::native::configured_router(state.clone())
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

    // Delete, then the avatar is gone.
    let (status, _, _) = call(
        state.clone(),
        "DELETE",
        "/api/v1/user/me/avatar",
        None,
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        state.clone(),
        "GET",
        "/api/v1/user/me/avatar",
        None,
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(
        state.clone(),
        "DELETE",
        "/api/v1/user/me/avatar",
        None,
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn avatar_upload_rejects_invalid_and_mismatched_content() {
    let db = common::TestDb::create("wp03_avatar_validation").await;
    let fixture = seed(&db).await;
    let state = state(&db);

    // Non-image magic bytes.
    let (status, _, _) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/png"),
        b"<svg onload=alert(1)>".to_vec(),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // GIF magic is not in the store's content-type constraint.
    let (status, _, _) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/gif"),
        b"GIF89a....".to_vec(),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Declared content type must match the magic bytes.
    let (status, _, _) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/jpeg"),
        png_64x64(),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty body.
    let (status, _, _) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/png"),
        vec![],
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Corrupt bytes with image magic fail the decode gate.
    let (status, _, _) = call(
        state.clone(),
        "POST",
        "/api/v1/user/me/avatar",
        Some("image/png"),
        b"\x89PNG\r\n\x1a\ncorrupt".to_vec(),
        Some(&fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing was stored.
    assert!(
        UserAdminStore::new()
            .avatar_by_user_id(&db.pool, fixture.user_id)
            .await
            .expect("lookup")
            .is_none()
    );
    db.drop().await;
}

#[tokio::test]
async fn avatar_routes_require_authentication() {
    let db = common::TestDb::create("wp03_avatar_auth").await;
    let _fixture = seed(&db).await;
    let state = state(&db);
    for method in ["GET", "POST", "DELETE"] {
        let (status, _, _) = call(
            state.clone(),
            method,
            "/api/v1/user/me/avatar",
            Some("image/png"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
    }
    db.drop().await;
}

// The OpenSubsonic read path over the new store is covered at HTTP level in
// `opensubsonic_media_api.rs::get_avatar_reads_phase9_avatar_store` and
// `opensubsonic_conditional_api.rs::get_user_avatar_last_changed_reflects_avatar_store`.
