//! Phase 7 compiled PartySessionEndpointRegistry route integration.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
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
        _track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Ok(String::new()) })
    }
}

async fn user(db: &common::TestDb, name: &str) -> NativePrincipal {
    let now = OffsetDateTime::now_utc();
    let normalized_username = name.to_ascii_uppercase();
    let email = format!("{name}@example.test");
    let normalized_email = format!("{}@EXAMPLE.TEST", name.to_ascii_uppercase());
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: name,
                normalized_username: &normalized_username,
                email: &email,
                normalized_email: &normalized_email,
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "endpoint-test-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &[],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("user");
    NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: false,
        is_editor: false,
    }
}

fn state(db: &common::TestDb) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://melodee.test".to_owned(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast client"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_owned(),
                90,
            )
            .expect("auth"),
        ),
    }
}

async fn call(
    state: NativeState,
    principal: NativePrincipal,
    method: &str,
    path: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body)
        .expect("request");
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("127.0.0.1".to_owned()));
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
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn endpoint_registry_enforces_visibility_session_roles_ownership_and_atomic_attachment() {
    let db = common::TestDb::create("server_native_endpoints").await;
    let owner = user(&db, "endpoint-owner").await;
    let listener = user(&db, "endpoint-listener").await;
    let own_id = Uuid::now_v7();
    let own_public = Uuid::now_v7();
    let listener_id = Uuid::now_v7();
    let system_id = Uuid::now_v7();
    let system_public = Uuid::now_v7();
    for (id, public, owner_id, name, shared) in [
        (
            own_id,
            own_public,
            Some(owner.user_id),
            "Owner Browser",
            false,
        ),
        (
            listener_id,
            Uuid::now_v7(),
            Some(listener.user_id),
            "Listener Browser",
            false,
        ),
        (system_id, system_public, None, "MPV Backend", true),
    ] {
        sqlx::query(
            "INSERT INTO native_playback_endpoints(\
             id,public_api_id,owner_user_id,name,endpoint_type,is_shared,last_seen_at,created_at,updated_at) \
             VALUES($1,$2,$3,$4,'WebPlayer',$5,now(),now(),now())",
        )
        .bind(id)
        .bind(public)
        .bind(owner_id)
        .bind(name)
        .bind(shared)
        .execute(&db.pool)
        .await
        .expect("endpoint");
    }
    let session_id = Uuid::now_v7();
    let mut tx = db.pool.begin().await.expect("session tx");
    sqlx::query(
        "INSERT INTO native_endpoint_sessions(\
         session_public_api_id,owner_user_id,is_active,created_at,updated_at) \
         VALUES($1,$2,true,now(),now())",
    )
    .bind(session_id)
    .bind(owner.user_id)
    .execute(&mut *tx)
    .await
    .expect("session");
    for (user_id, role) in [(owner.user_id, "owner"), (listener.user_id, "listener")] {
        sqlx::query(
            "INSERT INTO native_endpoint_session_members(session_public_api_id,user_id,role,joined_at) \
             VALUES($1,$2,$3,now())",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(role)
        .execute(&mut *tx)
        .await
        .expect("membership");
    }
    tx.commit().await.expect("session commit");
    let app_state = state(&db);

    let (status, bytes) = call(
        app_state.clone(),
        owner,
        "GET",
        "/api/v1/endpoints",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["apiKey"], own_public.to_string());

    let (status, bytes) = call(
        app_state.clone(),
        owner,
        "GET",
        &format!("/api/v1/endpoints/for-session/{session_id}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let attach_body = Body::from(format!(r#"{{"sessionApiKey":"{session_id}"}}"#));
    let (status, _) = call(
        app_state.clone(),
        listener,
        "POST",
        &format!("/api/v1/endpoints/{system_public}/attach"),
        attach_body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let attach_body = Body::from(format!(r#"{{"sessionApiKey":"{session_id}"}}"#));
    let (status, bytes) = call(
        app_state.clone(),
        owner,
        "POST",
        &format!("/api/v1/endpoints/{system_public}/attach"),
        attach_body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT active_endpoint_id FROM native_endpoint_sessions WHERE session_public_api_id=$1",
    )
    .bind(session_id)
    .fetch_one(&db.pool)
    .await
    .expect("active endpoint");
    assert_eq!(active, Some(system_id));

    let (status, _) = call(
        app_state,
        owner,
        "POST",
        &format!("/api/v1/endpoints/{system_public}/detach"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    db.drop().await;
}
