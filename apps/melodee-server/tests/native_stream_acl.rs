//! Native stream library-access authorization regression (ADR-0125).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{fs, net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::{
    native::{
        NativeFuture, NativeState, NativeStreamUrlSigner, PostgresNativeUserStateReader,
        podcasts::PodcastDiscoveryClient, stream::DatabaseStreamUrlSigner,
    },
    native_stream::{
        CanonicalAssetRoot, ClientAddressResolver, DirectPlayService, StreamTokenCodec,
    },
};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug)]
struct UnusedSigner;
impl NativeStreamUrlSigner for UnusedSigner {
    fn stream_url(
        &self,
        _track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Err(()) })
    }
}

#[tokio::test]
async fn valid_hmac_for_a_restricted_library_returns_uniform_unauthorized_without_media_bytes() {
    let db = common::TestDb::create("server_native_stream_acl").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7901,
        CatalogFixturePresentation {
            release_title: "Restricted Release",
            release_artist: "Restricted Artist",
            track_title: "Restricted Track",
            track_artist: "Restricted Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track");
    let source_library_id = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET source_library_id=$2 WHERE id=$1")
        .bind(fixture.work_item_id())
        .bind(source_library_id)
        .execute(&db.pool)
        .await
        .expect("source library");

    let now = OffsetDateTime::now_utc();
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "restricted-stream-user",
                normalized_username: "RESTRICTEDSTREAMUSER",
                email: "restricted-stream@example.test",
                normalized_email: "RESTRICTED-STREAM@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "restricted-stream-key",
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
    let group_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_library_access_groups(id,public_api_id,name,created_at,updated_at) \
         VALUES($1,$2,'restricted-stream-library',now(),now())",
    )
    .bind(group_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at) \
         VALUES($1,$2,now())",
    )
    .bind(group_id)
    .bind(source_library_id)
    .execute(&db.pool)
    .await
    .expect("binding");

    let token = StreamTokenCodec
        .issue(
            "restricted-stream-key",
            user.public_api_id,
            track_public_id,
            "127.0.0.1",
            now.unix_timestamp(),
        )
        .expect("valid HMAC");
    let temp = tempfile::tempdir().expect("stream root");
    let media_path = temp.path().join(&fixture.published.canonical_asset_path);
    fs::create_dir_all(media_path.parent().expect("media parent")).expect("media directory");
    fs::write(&media_path, [0x5a; 42]).expect("restricted media bytes");
    let direct_play = DirectPlayService::new(
        CanonicalAssetRoot::open(temp.path()).expect("canonical root"),
        1,
    );
    let state = NativeState {
        pool: db.pool.clone(),
        search: None,
        public_origin: "http://melodee.test".to_owned(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: PodcastDiscoveryClient::itunes().expect("podcast client"),
        stream_url_signer: Arc::new(UnusedSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play,
        client_addresses: ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_owned(),
                90,
            )
            .expect("auth"),
        ),
    };
    let path = format!(
        "/song/stream/{track_public_id}/{}/{token}",
        user.public_api_id
    );
    let mut request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41000".parse::<SocketAddr>().unwrap(),
    ));
    let response = melodee_server::native::router()
        .with_state(state)
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!body.windows(42).any(|window| window == [0x5a; 42]));

    let signer = DatabaseStreamUrlSigner::new(db.pool.clone(), "http://melodee.test");
    assert!(
        signer
            .stream_url(track_public_id, user.public_api_id, "127.0.0.1")
            .await
            .is_err(),
        "catalog DTO assembly must not issue unusable URLs for restricted Tracks"
    );
    db.drop().await;
}
