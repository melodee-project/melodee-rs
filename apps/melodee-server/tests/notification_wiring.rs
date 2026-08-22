//! event-wiring evidence (PRD-NOTIF-001/002): the native
//! password-reset, share-creation, and request-fulfillment flows dispatch a
//! fire-and-record notification. With the fixture's disabled (noop)
//! transport each dispatch is honestly recorded as `skipped`, and the
//! triggering API operation still succeeds — delivery never rolls back the
//! operation that triggered it.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport, StoredNativeUser};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async move { Ok(format!("http://melodee.test/stream/{track_public_id}")) })
    }
}

async fn import_user(db: &common::TestDb, username: &str) -> StoredNativeUser {
    let now = OffsetDateTime::now_utc();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username,
                normalized_username: &username.to_ascii_uppercase(),
                email: &format!("{username}@example.test"),
                normalized_email: &format!("{}@EXAMPLE.TEST", username.to_ascii_uppercase()),
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "notification-wiring-test-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["user".to_owned()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import user")
}

fn state(db: &common::TestDb) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        // Disabled transport: dispatches record an honest `skipped` row.
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://melodee.test".to_owned(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_owned(),
                90,
            )
            .expect("test auth config"),
        ),
    }
}

async fn call(
    state: NativeState,
    principal: Option<NativePrincipal>,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Option<serde_json::Value>) {
    let bytes = body.map_or_else(Vec::new, |value| serde_json::to_vec(&value).expect("JSON"));
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(bytes))
        .expect("request");
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:41009".parse::<SocketAddr>().expect("test peer"),
    ));
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
        request
            .extensions_mut()
            .insert(NativeClientBinding("127.0.0.1".to_owned()));
    }
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
    let json = if bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice(&bytes).expect("JSON response"))
    };
    (status, json)
}

/// Delivery dispatch is spawned off the request path; poll the audit table
/// until the spawned task lands its row (or the test times out loudly).
async fn wait_for_delivery(
    db: &common::TestDb,
    template_key: &str,
) -> Option<(String, String, String)> {
    for _ in 0..100 {
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT template_key, recipient, status FROM notification_deliveries \
             WHERE template_key = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(template_key)
        .fetch_optional(&db.pool)
        .await
        .expect("delivery lookup");
        if row.is_some() {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

async fn delivery_count(db: &common::TestDb, template_key: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notification_deliveries WHERE template_key = $1")
        .bind(template_key)
        .fetch_one(&db.pool)
        .await
        .expect("delivery count")
}

#[tokio::test]
async fn password_reset_request_dispatches_a_fail_degraded_delivery() {
    let db = common::TestDb::create("wp09_wiring_reset").await;
    let user = import_user(&db, "wp09-reset-user").await;

    // Unknown account: 200 (non-enumerating) and no dispatch.
    let (status, _) = call(
        state(&db),
        None,
        Method::POST,
        "/api/v1/auth/password-reset/request",
        Some(serde_json::json!({"email": "ghost@example.test"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(delivery_count(&db, "user.password_reset").await, 0);

    // Known account: the reset request succeeds and the delivery is
    // honestly recorded as skipped (transport disabled) — never propagated.
    let (status, _) = call(
        state(&db),
        None,
        Method::POST,
        "/api/v1/auth/password-reset/request",
        Some(serde_json::json!({"email": user.email})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let delivery = wait_for_delivery(&db, "user.password_reset")
        .await
        .expect("password reset delivery recorded");
    assert_eq!(delivery.1, user.email);
    assert_eq!(delivery.2, "skipped");
    db.drop().await;
}

#[tokio::test]
async fn share_creation_and_request_fulfillment_dispatch_notifications() {
    let db = common::TestDb::create("wp09_wiring_share").await;
    let user = import_user(&db, "wp09-share-user").await;
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    };

    // Create a playlist to have a shareable, owned resource.
    let (status, playlist) = call(
        state(&db),
        Some(principal),
        Method::POST,
        "/api/v1/playlists",
        Some(serde_json::json!({"name": "wp09 mix"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{playlist:?}");
    let playlist_id = playlist.expect("playlist body")["id"]
        .as_str()
        .expect("playlist id")
        .to_string();

    let (status, share) = call(
        state(&db),
        Some(principal),
        Method::POST,
        "/api/v1/shares",
        Some(serde_json::json!({"shareType": 3, "resourceId": playlist_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{share:?}");
    let delivery = wait_for_delivery(&db, "share.created")
        .await
        .expect("share delivery recorded");
    assert_eq!(delivery.1, user.email);
    assert_eq!(delivery.2, "skipped");

    // Request fulfillment: the creator completes their own request and is
    // notified.
    let (status, created) = call(
        state(&db),
        Some(principal),
        Method::POST,
        "/api/v1/requests",
        Some(serde_json::json!({"category": 5, "description": "Add more jazz"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let request_id = created.expect("request body")["apiKey"]
        .as_str()
        .expect("request api key")
        .to_string();

    let (status, _) = call(
        state(&db),
        Some(principal),
        Method::POST,
        &format!("/api/v1/requests/{request_id}/complete"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let delivery = wait_for_delivery(&db, "request.fulfilled")
        .await
        .expect("request fulfillment delivery recorded");
    assert_eq!(delivery.1, user.email);
    assert_eq!(delivery.2, "skipped");
    db.drop().await;
}
