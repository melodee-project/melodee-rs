//! native-v1 router and catalog route integration evidence.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{collections::HashSet, sync::Arc};

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
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

fn assert_json_contract_shape(actual: &serde_json::Value, golden: &serde_json::Value) {
    match (actual, golden) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(golden)) => {
            let actual_keys = actual.keys().collect::<std::collections::BTreeSet<_>>();
            let golden_keys = golden.keys().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual_keys, golden_keys, "wire property set changed");
            for (key, golden_value) in golden {
                assert_json_contract_shape(&actual[key], golden_value);
            }
        }
        (serde_json::Value::Array(actual), serde_json::Value::Array(golden)) => {
            if let (Some(actual), Some(golden)) = (actual.first(), golden.first()) {
                assert_json_contract_shape(actual, golden);
            }
        }
        (serde_json::Value::String(_), serde_json::Value::String(_))
        | (serde_json::Value::Number(_), serde_json::Value::Number(_))
        | (serde_json::Value::Bool(_), serde_json::Value::Bool(_))
        | (serde_json::Value::Null, serde_json::Value::Null) => {}
        (actual, golden) => panic!("wire JSON type/nullability changed: {actual:?} vs {golden:?}"),
    }
}

fn golden(value: &str) -> serde_json::Value {
    serde_json::from_str(value).expect("frozen legacy golden JSON")
}

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        let client_binding = client_binding.to_string();
        Box::pin(async move {
            Ok(format!(
                "http://melodee.test/song/stream/{track_public_id}/{user_public_id}/signed-{client_binding}"
            ))
        })
    }
}

async fn native_test_state(db: &common::TestDb, username: &str) -> (NativeState, NativePrincipal) {
    let now = OffsetDateTime::now_utc();
    let normalized_username = username.to_ascii_uppercase();
    let email = format!("{username}@example.test");
    let normalized_email = email.to_ascii_uppercase();
    let roles = vec!["user".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username,
                normalized_username: &normalized_username,
                email: &email,
                normalized_email: &normalized_email,
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("test user");
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: false,
        is_editor: false,
    };
    let state = NativeState {
        lastfm_api_base_url: None,
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
    };
    (state, principal)
}

async fn response_json(
    state: NativeState,
    principal: NativePrincipal,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .uri(path)
        .body(Body::empty())
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
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON body")
    };
    (status, body)
}

async fn response_status(
    state: NativeState,
    principal: NativePrincipal,
    method: &str,
    path: &str,
) -> StatusCode {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("127.0.0.1".to_string()));
    melodee_server::native::router()
        .with_state(state)
        .oneshot(request)
        .await
        .expect("response")
        .status()
}

async fn response_json_body(
    state: NativeState,
    principal: NativePrincipal,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).expect("request JSON")))
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
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON body")
    };
    (status, body)
}

async fn response_bytes(
    state: NativeState,
    path: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().uri(path);
    if let Some(etag) = if_none_match {
        builder = builder.header(header::IF_NONE_MATCH, etag);
    }
    let response = melodee_server::native::router()
        .with_state(state)
        .oneshot(builder.body(Body::empty()).expect("request"))
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

async fn response_without_consuming_body(
    state: NativeState,
    path: &str,
) -> axum::response::Response {
    melodee_server::native::router()
        .with_state(state)
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

#[tokio::test]
async fn native_catalog_browse_detail_and_search_preserve_public_ids_and_user_state() {
    let db = common::TestDb::create("server_native_catalog").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7201,
        CatalogFixturePresentation {
            release_title: "Native Aurora",
            release_artist: "Phase Seven Artist",
            track_title: "Compatibility Track",
            track_artist: "Phase Seven Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let roles = vec!["user".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "native-catalog-user",
                normalized_username: "NATIVECATALOGUSER",
                email: "native@example.test",
                normalized_email: "NATIVE@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("user");
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: true,
        is_editor: true,
    };
    let state = NativeState {
        lastfm_api_base_url: None,
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
    };

    let artist_public_id: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM artists artist
         JOIN release_artists billing ON billing.artist_id = artist.id
         WHERE billing.release_id = $1 AND billing.role = 'primary'",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("Artist public ID");
    let release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Release public ID");
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Track public ID");

    let playlist_public_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_playlists
           (id, public_api_id, owner_user_id, name, is_public, created_at, updated_at)
         VALUES ($1,$2,$3,'Image route fixture',true,$4,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(playlist_public_id)
    .bind(user.id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("playlist image fixture");
    for (path, expected_type, expected_bytes) in [
        (
            format!("/images/album_{release_public_id}/1024"),
            "image/jpeg",
            include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin").as_slice(),
        ),
        (
            format!("/images/artist_{artist_public_id}/80"),
            "image/jpeg",
            include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin").as_slice(),
        ),
        (
            format!("/images/song_{track_public_id}/80"),
            "image/jpeg",
            include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-song-thumb.bin").as_slice(),
        ),
        (
            format!("/images/playlist_{playlist_public_id}/80"),
            "image/gif",
            include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-playlist-thumb.bin").as_slice(),
        ),
        (
            format!("/images/user_{}/80", user.public_api_id),
            "image/jpeg",
            include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin").as_slice(),
        ),
    ] {
        let (status, headers, bytes) = response_bytes(state.clone(), &path, None).await;
        assert_eq!(status, StatusCode::OK, "GET {path}");
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some(expected_type)
        );
        assert_eq!(bytes, expected_bytes, "frozen image bytes changed for {path}");
        assert!(
            headers.get(header::CACHE_CONTROL).is_none(),
            "native-v1 image contract must not add Cache-Control for {path}"
        );
        let etag = headers
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .expect("image etag");
        let (conditional, _, conditional_body) =
            response_bytes(state.clone(), &path, Some(etag)).await;
        assert_eq!(conditional, StatusCode::NOT_MODIFIED);
        assert!(conditional_body.is_empty());
    }
    sqlx::query(
        "UPDATE native_playlists SET image_mime_type = 'image/png', asset_key = $2
         WHERE public_api_id = $1",
    )
    .bind(playlist_public_id)
    .bind(melodee_persistence::asset_files::put(&db.pool, include_bytes!("../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin")).await.expect("store fixture"))
    .execute(&db.pool)
    .await
    .expect("custom playlist image");
    let (status, headers, resized) = response_bytes(
        state.clone(),
        &format!("/images/playlist_{playlist_public_id}/80"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/gif")
    );
    assert_eq!(
        image::ImageReader::new(std::io::Cursor::new(&resized))
            .with_guessed_format()
            .expect("custom image format")
            .into_dimensions()
            .expect("custom image dimensions"),
        (80, 80)
    );
    sqlx::query("UPDATE native_playlists SET image_mime_type = 'image/png', asset_key = $2 WHERE public_api_id = $1")
        .bind(playlist_public_id)
        .bind(melodee_persistence::asset_files::put(&db.pool, b"bad").await.expect("store fixture"))
        .execute(&db.pool)
        .await
        .expect("malformed image fixture");
    assert_eq!(
        response_bytes(
            state.clone(),
            &format!("/images/playlist_{playlist_public_id}/80"),
            None,
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (status, headers, body) = response_bytes(
        state.clone(),
        "/images/song_00000000-0000-0000-0000-000000000000/80",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_TYPE).is_none());
    assert!(body.is_empty());

    // ADR-0143 §2 precedence beyond the override: a stored operator override
    // wins over provider artwork, and provider artwork fills the gap left by a
    // Release whose publication plan carries none. `original` must serve those
    // source bytes untouched (the Admin UI addresses artwork that way, and the
    // token used to fall through to the empty-200 path).
    let original_path = format!("/images/album_{release_public_id}/original");
    let provider_bytes = include_bytes!(
        "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin"
    )
    .to_vec();
    sqlx::query(
        "INSERT INTO catalog_provider_images
           (id, entity_type, entity_id, provider, source_url, image_mime_type, asset_key,
            confidence, observed_at)
         VALUES ($1,'release',$2,'musicbrainz','https://example.invalid/cover.jpg','image/jpeg',$3,90,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(fixture.release_id().into_uuid())
    .bind(melodee_persistence::asset_files::put(&db.pool, &provider_bytes).await.expect("store fixture"))
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("provider artwork fixture");
    let (status, headers, bytes) = response_bytes(state.clone(), &original_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/jpeg")
    );
    assert_eq!(
        bytes, provider_bytes,
        "provider artwork must be served byte-for-byte for `original`"
    );

    let override_bytes = include_bytes!(
        "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin"
    )
    .to_vec();
    sqlx::query(
        "INSERT INTO catalog_entity_images
           (entity_type, release_id, image_mime_type, asset_key, uploaded_by, created_at, updated_at)
         VALUES ('release',$1,'image/jpeg',$2,'test-admin',$3,$3)",
    )
    .bind(fixture.release_id().into_uuid())
    .bind(melodee_persistence::asset_files::put(&db.pool, &override_bytes).await.expect("store fixture"))
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("override artwork fixture");
    let (status, _, bytes) = response_bytes(state.clone(), &original_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        bytes, override_bytes,
        "an operator override must outrank provider artwork"
    );
    sqlx::query("DELETE FROM catalog_entity_images WHERE release_id = $1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("restore provider precedence fixture");
    sqlx::query("DELETE FROM catalog_provider_images WHERE entity_id = $1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("restore placeholder fallback fixture");

    let fallback_path = format!("/images/album_{release_public_id}/80");
    let mut admitted = Vec::new();
    for _ in 0..4 {
        let response = response_without_consuming_body(state.clone(), &fallback_path).await;
        assert_eq!(response.status(), StatusCode::OK);
        admitted.push(response);
    }
    assert_eq!(
        response_without_consuming_body(state.clone(), &fallback_path)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "canonical-artwork fallback bodies retain admission until consumed or dropped"
    );
    drop(admitted.pop());
    let replacement = response_without_consuming_body(state.clone(), &fallback_path).await;
    assert_eq!(replacement.status(), StatusCode::OK);
    drop(replacement);
    drop(admitted);

    sqlx::query("UPDATE releases SET status='deprecated' WHERE id=$1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("retire image fixture Release");
    for path in [
        format!("/images/album_{release_public_id}/80"),
        format!("/images/song_{track_public_id}/80"),
    ] {
        // A withheld Release still names a real entity, so the frozen endpoint
        // answers with the captured default image rather than an empty body —
        // never with the Release's own artwork.
        let (status, headers, bytes) = response_bytes(state.clone(), &path, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/jpeg")
        );
        assert_eq!(
            bytes.as_slice(),
            include_bytes!(
                "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin"
            )
            .as_slice(),
            "a withheld Release must serve the captured default, not its artwork: {path}"
        );
    }

    // A known Artist with no artwork is not an unknown id: the frozen endpoint
    // serves the captured default image, which is what keeps client artwork
    // slots rendering instead of showing a broken image.
    let bare_artist_public_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists
           (id, public_api_id, canonical_name, sort_name, normalized_name, artist_type, status,
            created_at, updated_at)
         VALUES ($1,$2,'Bare Artist','Bare Artist','bare artist','person','active',$3,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(bare_artist_public_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("bare Artist fixture");
    let (status, headers, bytes) = response_bytes(
        state.clone(),
        &format!("/images/artist_{bare_artist_public_id}/80"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    assert_eq!(
        bytes.as_slice(),
        include_bytes!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin"
        )
        .as_slice(),
        "a known Artist without artwork must serve the captured default image"
    );
    let (status, headers, bytes) = response_bytes(
        state.clone(),
        &format!("/images/artist_{}/80", Uuid::now_v7()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::CONTENT_TYPE).is_none());
    assert!(bytes.is_empty(), "unknown ids keep the captured empty 200");
    sqlx::query("UPDATE releases SET status='active' WHERE id=$1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("restore image fixture Release");
    let (status, artists) = response_json(
        state.clone(),
        principal,
        "/api/v1/artists?page=1&pageSize=5&orderBy=Name&orderDirection=asc",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(artists["meta"]["totalCount"].as_i64().unwrap() >= 1);
    assert!(artists["data"].as_array().unwrap().iter().any(|artist| {
        artist["id"] == artist_public_id.to_string() && artist["name"] == "Phase Seven Artist"
    }));
    assert_json_contract_shape(
        &artists,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-artists-list.json"
        )),
    );

    for path in [
        "/api/v1/artists/recent?limit=5".to_string(),
        format!("/api/v1/artists/{artist_public_id}"),
        format!("/api/v1/artists/{artist_public_id}/albums?page=1&pageSize=5"),
        format!("/api/v1/artists/{artist_public_id}/songs?page=1&pageSize=5"),
        "/api/v1/albums?page=1&pageSize=5".to_string(),
        "/api/v1/albums/recent?limit=5".to_string(),
        format!("/api/v1/albums/{release_public_id}/songs?page=1&pageSize=5"),
        "/api/v1/songs?page=1&pageSize=5".to_string(),
    ] {
        let (status, body) = response_json(state.clone(), principal, &path).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    }

    let (status, album) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/albums/{release_public_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(album["id"], release_public_id.to_string());
    assert_eq!(album["name"], "Native Aurora");
    assert_eq!(album["artist"]["id"], artist_public_id.to_string());
    assert_json_contract_shape(
        &album,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-album-detail.json"
        )),
    );

    let (status, song) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/songs/{track_public_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(song["id"], track_public_id.to_string());
    assert_eq!(song["title"], "Compatibility Track");
    assert!(
        song["streamUrl"]
            .as_str()
            .unwrap()
            .contains("signed-127.0.0.1")
    );
    assert_json_contract_shape(
        &song,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-song-detail.json"
        )),
    );

    let (status, found) = response_json(
        state.clone(),
        principal,
        "/api/v1/search/songs?q=Compatibility&page=1&pageSize=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["meta"]["totalCount"], 1);
    assert_eq!(found["data"][0]["id"], track_public_id.to_string());
    assert_json_contract_shape(
        &found,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-search-songs-neon.json"
        )),
    );

    let (status, combined) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/search",
        serde_json::json!({"query": "Compatibility", "pageSize": 10}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(combined["data"]["totalSongs"], 1);
    assert_json_contract_shape(
        &combined,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-search-post-dune.json"
        )),
    );

    let (status, suggested) = response_json(
        state.clone(),
        principal,
        "/api/v1/search/suggest?q=Ph&limit=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(suggested["artists"][0]["id"], artist_public_id.to_string());
    let (status, playlist_suggestions) = response_json(
        state.clone(),
        principal,
        "/api/v1/search/suggest?q=Image&limit=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        playlist_suggestions["playlists"],
        serde_json::json!([]),
        "legacy SearchService does not populate playlist suggestions"
    );

    let (status, advanced) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/search/advanced",
        serde_json::json!({
            "query": "Compatibility",
            "types": ["song"],
            "page": 1,
            "limit": 10
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        advanced["results"]["songs"][0]["id"],
        track_public_id.to_string()
    );

    let (status, similar) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/search/similar/{track_public_id}/song?limit=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(similar["similar"].as_array().unwrap().is_empty());

    let (status, lyrics) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/songs/{track_public_id}/lyrics"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(lyrics["code"], "NOT_FOUND");

    let (status, genres) =
        response_json(state.clone(), principal, "/api/v1/genres?page=1&limit=50").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(genres["genres"][0]["id"], "Rml4dHVyZQ");
    assert_json_contract_shape(
        &genres,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-genres-list.json"
        )),
    );
    let (status, genre_songs) = response_json(
        state.clone(),
        principal,
        "/api/v1/genres/Rml4dHVyZQ/songs?page=1&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(genre_songs["songs"][0]["id"], track_public_id.to_string());
    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            "/api/v1/genres/starred/Rml4dHVyZQ/true",
        )
        .await,
        StatusCode::OK
    );
    for path in [
        format!("/api/v1/artists/hated/{artist_public_id}/true"),
        format!("/api/v1/artists/setrating/{artist_public_id}/4"),
        format!("/api/v1/albums/starred/{release_public_id}/true"),
        format!("/api/v1/albums/hated/{release_public_id}/true"),
        format!("/api/v1/songs/starred/{track_public_id}/true"),
        format!("/api/v1/songs/setrating/{track_public_id}/5"),
    ] {
        assert_eq!(
            response_status(state.clone(), principal, "POST", &path).await,
            StatusCode::OK,
            "POST {path}"
        );
    }
    let (status, starred_genres) =
        response_json(state.clone(), principal, "/api/v1/genres/starred").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(starred_genres, serde_json::json!(["Fixture"]));
    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            "/api/v1/genres/hated/Rml4dHVyZQ/true",
        )
        .await,
        StatusCode::OK
    );
    let (status, hated_genres) =
        response_json(state.clone(), principal, "/api/v1/genres/hated").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hated_genres, serde_json::json!(["Fixture"]));

    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/artists/starred/{artist_public_id}/true"),
        )
        .await,
        StatusCode::OK
    );
    let (status, liked) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/artists/liked?page=1&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(liked["data"].as_array().unwrap().iter().any(|artist| {
        artist["id"] == artist_public_id.to_string() && artist["userStarred"] == true
    }));

    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/albums/setrating/{release_public_id}/5"),
        )
        .await,
        StatusCode::OK
    );
    let (status, rated) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/albums/top-rated?page=1&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rated["data"][0]["id"], release_public_id.to_string());
    assert_eq!(rated["data"][0]["userRating"], 5);

    for path in [
        "/api/v1/user/artists/disliked?page=1&limit=50",
        "/api/v1/user/artists/rated?page=1&limit=50",
        "/api/v1/user/artists/top-rated?page=1&limit=50",
        "/api/v1/user/artists/recently-played?page=1&limit=50",
        "/api/v1/user/albums/liked?page=1&limit=50",
        "/api/v1/user/albums/disliked?page=1&limit=50",
        "/api/v1/user/albums/rated?page=1&limit=50",
        "/api/v1/user/albums/recently-played?page=1&limit=50",
        "/api/v1/user/songs/liked?page=1&limit=50",
        "/api/v1/user/songs/rated?page=1&limit=50",
        "/api/v1/user/songs/top-rated?page=1&limit=50",
        "/api/v1/user/songs/recently-played?page=1&limit=50",
        "/api/v1/user/last-played?page=1&limit=50",
    ] {
        let (status, body) = response_json(state.clone(), principal, path).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    }

    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/songs/hated/{track_public_id}/true"),
        )
        .await,
        StatusCode::OK
    );
    let (status, disliked) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/songs/disliked?page=1&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disliked["data"][0]["id"], track_public_id.to_string());

    let (status, _) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/scrobble",
        serde_json::json!({
            "songId": track_public_id,
            "playerName": "phase7-http-test",
            "scrobbleType": "played",
            "timestamp": now.unix_timestamp(),
            "playedDuration": 30
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, stats) =
        response_json(state.clone(), principal, "/api/v1/user/stats?days=30").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["totalPlays"], 1);

    // The last-played route is a history feed: the play it counted is the play it
    // returns, with the Song DTO the client needs to play it again (ADR-0188).
    let (status, last_played) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/last-played?page=1&pageSize=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last_played["meta"]["totalCount"], 1);
    let item = &last_played["data"][0];
    assert_eq!(item["song"]["id"], track_public_id.to_string());
    assert_eq!(item["song"]["title"], "Compatibility Track");
    assert!(item["song"]["streamUrl"].is_string());
    assert!(item["playedAt"].is_string());
    assert_eq!(item["duration"], 30);

    let (status, lastfm_auth) = response_json(
        state.clone(),
        principal,
        "/api/v1/scrobble/lastfm/auth-url?callback=https%3A%2F%2Fclient.example%2Fcallback",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(lastfm_auth["code"], "BAD_REQUEST");
    assert_eq!(lastfm_auth["message"], "Last.fm not configured");
    let (status, lastfm_session) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/scrobble/lastfm/session",
        serde_json::json!({"token": "synthetic-provider-token"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        lastfm_session["message"],
        "Unable to exchange Last.fm session key"
    );
    let (status, lastfm_disconnect) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/scrobble/lastfm/disconnect",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lastfm_disconnect["message"], "Last.fm disconnected");
    let (status, top) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/stats/top-songs?days=30&limit=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(top["data"][0]["songId"], track_public_id.to_string());
    let (status, top_genres) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/stats/top-genres?days=30&limit=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(top_genres["data"][0]["name"], "Fixture");
    let (status, per_day) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/stats/plays-per-day?days=30",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let per_day_rows = per_day["data"].as_array().expect("daily stats data");
    assert_eq!(per_day_rows.len(), 31);
    assert!(per_day_rows.iter().any(|row| row["plays"] == 1));
    let (status, history) = response_json(
        state.clone(),
        principal,
        "/api/v1/user/stats/history?limit=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["data"][0]["songId"], track_public_id.to_string());

    let (status, users) = response_json(state.clone(), principal, "/api/v1/admin/users").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(users[0]["id"], user.public_api_id.to_string());
    assert_eq!(users[0]["username"], "native-catalog-user");
    assert_eq!(users[0]["isEnabled"], true);
    let mut non_admin = principal;
    non_admin.is_admin = false;
    let (status, forbidden) = response_json(state.clone(), non_admin, "/api/v1/admin/users").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden["code"], "FORBIDDEN");

    let (status, recent) =
        response_json(state.clone(), principal, "/api/v1/songs/recent?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent["data"].as_array().unwrap().len(), 1);
    let (status, _) = response_json(
        state.clone(),
        principal,
        "/api/v1/artists?page=2147483647&pageSize=500",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let overlong_query = "x".repeat(257);
    let (status, _) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/search/songs?q={overlong_query}&page=1&pageSize=25"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, literal_wildcard) = response_json(
        state.clone(),
        principal,
        "/api/v1/search/songs?q=%25&page=1&pageSize=25",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(literal_wildcard["meta"]["totalCount"], 0);
    let (status, random) = response_json(state, principal, "/api/v1/songs/random?count=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(random["count"], 1);
    assert_eq!(random["data"].as_array().unwrap().len(), 1);
    db.drop().await;
}

#[tokio::test]
async fn native_search_suggest_preserves_legacy_contains_matching_and_order() {
    let db = common::TestDb::create("server_native_search_suggest").await;
    phase5_common::seed_cataloged_release(
        &db,
        7_301,
        CatalogFixturePresentation {
            release_title: "Desert Rock Radio",
            release_artist: "Dune",
            track_title: "Sandstone",
            track_artist: "Session Band A",
            release_date: "2026-08-12",
        },
    )
    .await;
    phase5_common::seed_cataloged_release(
        &db,
        7_302,
        CatalogFixturePresentation {
            release_title: "Unicode Release",
            release_artist: "Sæglópur 日本",
            track_title: "Neon 東京",
            track_artist: "Session Band B",
            release_date: "2026-08-12",
        },
    )
    .await;
    phase5_common::seed_cataloged_release(
        &db,
        7_303,
        CatalogFixturePresentation {
            release_title: "Phase Zero Compilation",
            release_artist: "Various Artists",
            track_title: "Saxophone in the Rain",
            track_artist: "Session Band C",
            release_date: "2026-08-12",
        },
    )
    .await;
    phase5_common::seed_cataloged_release(
        &db,
        7_304,
        CatalogFixturePresentation {
            release_title: "Dusk",
            release_artist: "Düné",
            track_title: "Coda",
            track_artist: "Session Band D",
            release_date: "2026-08-12",
        },
    )
    .await;

    // The frozen legacy database stores both Dune spellings under the same
    // accent-folded NameNormalized value. Mirror that imported fact so this
    // fixture exercises the compatibility query rather than the new-catalog
    // identity normalizer.
    sqlx::query("UPDATE artists SET normalized_name = 'dune' WHERE canonical_name = 'Düné'")
        .execute(&db.pool)
        .await
        .expect("accent-folded imported Artist search value");

    let (state, principal) = native_test_state(&db, "native-search-suggest-user").await;
    let (status, suggestions) =
        response_json(state, principal, "/api/v1/search/suggest?q=Ne&limit=5").await;
    assert_eq!(status, StatusCode::OK, "{suggestions}");
    let artist_names = suggestions["artists"]
        .as_array()
        .expect("artist suggestions")
        .iter()
        .map(|value| value["name"].as_str().expect("artist name"))
        .collect::<Vec<_>>();
    assert_eq!(artist_names, ["Dune", "Düné"]);
    assert_eq!(suggestions["albums"], serde_json::json!([]));
    let song_names = suggestions["songs"]
        .as_array()
        .expect("song suggestions")
        .iter()
        .map(|value| value["name"].as_str().expect("song name"))
        .collect::<Vec<_>>();
    assert_eq!(
        song_names,
        [
            "Sandstone - Dune",
            "Neon 東京 - Sæglópur 日本",
            "Saxophone in the Rain - Various Artists",
        ]
    );
    assert_eq!(suggestions["playlists"], serde_json::json!([]));
    db.drop().await;
}

#[tokio::test]
async fn deprecated_releases_disappear_from_every_native_track_read_surface() {
    let db = common::TestDb::create("server_native_deprecated_release_tracks").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7_250,
        CatalogFixturePresentation {
            release_title: "Lifecycle Release",
            release_artist: "Lifecycle Artist",
            track_title: "Lifecycle Track",
            track_artist: "Lifecycle Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let (state, principal) = native_test_state(&db, "deprecated-release-user").await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Track public ID");

    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/songs/starred/{track_public_id}/true"),
        )
        .await,
        StatusCode::OK
    );
    sqlx::query("UPDATE releases SET status='deprecated',updated_at=now() WHERE id=$1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("deprecate Release");

    for path in [
        format!("/api/v1/songs/{track_public_id}"),
        format!("/api/v1/songs/{track_public_id}/lyrics"),
        format!("/api/v1/search/similar/{track_public_id}/song"),
    ] {
        assert_eq!(
            response_json(state.clone(), principal, &path).await.0,
            StatusCode::NOT_FOUND,
            "deprecated Track detail leaked through {path}"
        );
    }

    let release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Release public ID");
    assert_eq!(
        response_json(
            state.clone(),
            principal,
            &format!("/api/v1/albums/{release_public_id}"),
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, releases) = response_json(
        state.clone(),
        principal,
        "/api/v1/albums?page=1&pageSize=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(releases["data"], serde_json::json!([]));
    assert_eq!(releases["meta"]["totalCount"], 0);

    for path in [
        "/api/v1/songs?page=1&pageSize=50",
        "/api/v1/songs/recent?limit=50",
        "/api/v1/search/songs?q=Lifecycle&page=1&pageSize=50",
        "/api/v1/user/songs/liked?page=1&limit=50",
    ] {
        let (status, body) = response_json(state.clone(), principal, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body["data"], serde_json::json!([]), "{path}");
        assert_eq!(body["meta"]["totalCount"], 0, "{path}");
    }

    let (status, random) =
        response_json(state.clone(), principal, "/api/v1/songs/random?count=50").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(random, serde_json::json!({"count": 0, "data": []}));

    let (status, suggestions) = response_json(
        state.clone(),
        principal,
        "/api/v1/search/suggest?q=Lifecycle&limit=10",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(suggestions["songs"], serde_json::json!([]));

    let (status, genres) =
        response_json(state.clone(), principal, "/api/v1/genres?page=1&limit=50").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(genres["genres"], serde_json::json!([]));
    assert_eq!(genres["totalCount"], 0);
    assert_eq!(
        response_json(
            state.clone(),
            principal,
            "/api/v1/genres/Rml4dHVyZQ/songs?page=1&limit=50",
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (status, stats) = response_json(state, principal, "/api/v1/user/stats?days=30").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["favoriteSongs"], 0);
    db.drop().await;
}

#[tokio::test]
async fn native_random_songs_samples_beyond_the_first_ten_thousand_tracks() {
    let db = common::TestDb::create("server_native_random_domain").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7_301,
        CatalogFixturePresentation {
            release_title: "Random Domain",
            release_artist: "Random Domain Artist",
            track_title: "Random Seed Track",
            track_artist: "Random Domain Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let (state, principal) = native_test_state(&db, "random-domain-user").await;
    let (audio_identity_id, media_asset_id, artist_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT track.audio_identity_id,track.playable_media_asset_id,billing.artist_id
         FROM tracks track
         JOIN track_artists billing ON billing.track_id=track.id AND billing.role='primary'
         WHERE track.release_id=$1 LIMIT 1",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("random fixture facts");
    let now = OffsetDateTime::now_utc();
    let mut tx = db.pool.begin().await.expect("random seed transaction");
    sqlx::query(
        "CREATE TEMP TABLE native_random_tail ON COMMIT DROP AS
           SELECT ordinal,uuidv7() AS id,uuidv7() AS public_api_id
           FROM generate_series(2,12001) ordinal",
    )
    .execute(&mut *tx)
    .await
    .expect("random Track identity map");
    sqlx::query(
        "INSERT INTO tracks
           (id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,
            normalized_title,sort_title,display_artist,disc_number,track_number,duration_ms,
            revision,created_at,updated_at)
         SELECT id,public_api_id,$1,$2,$3,ordinal,format('Random Track %s',ordinal),
                format('random track %s',ordinal),format('random track %s',ordinal),
                'Random Domain Artist',1,ordinal,180000,1,$4,$4
         FROM native_random_tail",
    )
    .bind(fixture.release_id().into_uuid())
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("random Tracks");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,ordinal,created_at)
         SELECT id,$1,'playable',1,$2 FROM native_random_tail",
    )
    .bind(media_asset_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("random playable links");
    sqlx::query(
        "INSERT INTO track_artists
           (id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         SELECT uuidv7(),id,$1,'primary',1,'Random Domain Artist','',$2
         FROM native_random_tail",
    )
    .bind(artist_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("random Track artists");
    sqlx::query(
        "INSERT INTO artist_track_associations
           (artist_id,track_id,association_type,participation_kind,source_key,confidence,
            is_performance,created_at)
         SELECT $1,id,'primary_artist','direct',id,10000,true,$2 FROM native_random_tail",
    )
    .bind(artist_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("random Artist associations");
    sqlx::query(
        "UPDATE release_catalog_aggregates SET track_count=12001,duration_ms=2160180000,
           revision=revision+1,updated_at=$2 WHERE release_id=$1",
    )
    .bind(fixture.release_id().into_uuid())
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("random Release aggregate");
    tx.commit().await.expect("random seed commit");
    sqlx::query("ANALYZE tracks,track_artists,artist_track_associations")
        .execute(&db.pool)
        .await
        .expect("random sampling statistics");

    let old_fixed_window =
        sqlx::query_scalar::<_, Uuid>("SELECT public_api_id FROM tracks ORDER BY id LIMIT 10000")
            .fetch_all(&db.pool)
            .await
            .expect("old fixed random window")
            .into_iter()
            .collect::<HashSet<_>>();
    let (status, body) = response_json(state, principal, "/api/v1/songs/random?count=500").await;
    assert_eq!(status, StatusCode::OK);
    let sampled = body["data"].as_array().expect("random songs");
    assert_eq!(sampled.len(), 500);
    assert!(
        sampled.iter().any(|song| {
            song["id"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_some_and(|id| !old_fixed_window.contains(&id))
        }),
        "random selection remained trapped in the former first-10k window"
    );
    db.drop().await;
}

#[tokio::test]
async fn native_library_acl_prevents_cross_library_browse_search_mutation_and_scrobble_leaks() {
    let db = common::TestDb::create("server_native_catalog_acl").await;
    let allowed = phase5_common::seed_cataloged_release(
        &db,
        7301,
        CatalogFixturePresentation {
            release_title: "Allowed Library Release",
            release_artist: "Allowed Library Artist",
            track_title: "Allowed Library Track",
            track_artist: "Allowed Library Artist",
            release_date: "2025-01-01",
        },
    )
    .await;
    let hidden = phase5_common::seed_cataloged_release(
        &db,
        7302,
        CatalogFixturePresentation {
            release_title: "Hidden Library Release",
            release_artist: "Hidden Library Artist",
            track_title: "Hidden Library Track",
            track_artist: "Hidden Library Artist",
            release_date: "2026-01-01",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let roles = vec!["user".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "library-a-user",
                normalized_username: "LIBRARYAUSER",
                email: "library-a@example.test",
                normalized_email: "LIBRARY-A@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "library-a-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("user");
    let principal = NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: false,
        is_editor: false,
    };
    let state = NativeState {
        lastfm_api_base_url: None,
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
    };
    let hidden_release_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(hidden.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("hidden release");
    let hidden_track_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(hidden.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("hidden track");
    let hidden_artist_id: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM artists artist
         JOIN release_artists billing ON billing.artist_id = artist.id
         WHERE billing.release_id = $1 AND billing.role = 'primary'",
    )
    .bind(hidden.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("hidden artist");
    let (allowed_artist_internal_id, allowed_artist_public_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT artist.id,artist.public_api_id FROM artists artist
             JOIN release_artists billing ON billing.artist_id=artist.id
             WHERE billing.release_id=$1 AND billing.role='primary'",
    )
    .bind(allowed.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("allowed artist");

    // Establish user state while both source libraries are unrestricted, then
    // restrict one of them. The subsequent reads prove revocation applies to
    // already-durable annotations and play history, not only new writes.
    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/albums/starred/{hidden_release_id}/true"),
        )
        .await,
        StatusCode::OK
    );
    let (status, _) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/scrobble",
        serde_json::json!({
            "songId": hidden_track_id,
            "playerName": "acl-regression",
            "scrobbleType": "played",
            "timestamp": now.unix_timestamp(),
            "playedDuration": 20
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let allowed_library_id = Uuid::now_v7();
    let hidden_library_id = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET source_library_id = $1 WHERE id = $2")
        .bind(allowed_library_id)
        .bind(allowed.work_item_id())
        .execute(&db.pool)
        .await
        .expect("allowed source library");
    sqlx::query("UPDATE release_work_items SET source_library_id = $1 WHERE id = $2")
        .bind(hidden_library_id)
        .bind(hidden.work_item_id())
        .execute(&db.pool)
        .await
        .expect("hidden source library");
    let allowed_group = Uuid::now_v7();
    let hidden_group = Uuid::now_v7();
    for (group, name, library) in [
        (allowed_group, "Allowed group", allowed_library_id),
        (hidden_group, "Hidden group", hidden_library_id),
    ] {
        sqlx::query(
            "INSERT INTO native_library_access_groups
               (id, public_api_id, name, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$4)",
        )
        .bind(group)
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("library group");
        sqlx::query(
            "INSERT INTO native_library_group_libraries (group_id, library_id, imported_at)
             VALUES ($1,$2,$3)",
        )
        .bind(group)
        .bind(library)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("group library binding");
    }

    // A user with no group memberships must not bypass globally configured
    // library restrictions.
    for path in [
        "/api/v1/artists?page=1&pageSize=50",
        "/api/v1/albums?page=1&pageSize=50",
        "/api/v1/songs?page=1&pageSize=50",
    ] {
        let (status, body) = response_json(state.clone(), principal, path).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
        assert_eq!(body["meta"]["totalCount"], 0, "GET {path}: {body}");
        assert!(body["data"].as_array().is_some_and(Vec::is_empty));
    }
    let (status, random) =
        response_json(state.clone(), principal, "/api/v1/songs/random?count=50").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(random["count"], 0);
    assert!(random["data"].as_array().is_some_and(Vec::is_empty));

    // One Artist spans both libraries. The durable catalog aggregate is
    // deliberately global; the native DTO must recompute counts within the
    // requesting user's ACL rather than reveal the hidden Release/Track.
    sqlx::query("UPDATE release_artists SET artist_id=$1 WHERE release_id=$2")
        .bind(allowed_artist_internal_id)
        .bind(hidden.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("shared Release Artist");
    sqlx::query(
        "UPDATE track_artists SET artist_id=$1 WHERE track_id IN (
           SELECT id FROM tracks WHERE release_id=$2)",
    )
    .bind(allowed_artist_internal_id)
    .bind(hidden.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("shared Track Artist");
    sqlx::query(
        "UPDATE artist_track_associations SET artist_id=$1 WHERE track_id IN (
           SELECT id FROM tracks WHERE release_id=$2)",
    )
    .bind(allowed_artist_internal_id)
    .bind(hidden.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("shared Artist associations");
    sqlx::query(
        "UPDATE artist_catalog_aggregates SET release_count=2,billed_track_count=2
         WHERE artist_id=$1",
    )
    .bind(allowed_artist_internal_id)
    .execute(&db.pool)
    .await
    .expect("global Artist aggregate");

    sqlx::query(
        "INSERT INTO native_user_library_groups (user_id, group_id, created_at)
         VALUES ($1,$2,$3)",
    )
    .bind(user.id)
    .bind(allowed_group)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("allowed membership");

    let (status, visible_artist) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/artists/{allowed_artist_public_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(visible_artist["albumCount"], 1);
    assert_eq!(visible_artist["songCount"], 1);

    for path in [
        "/api/v1/artists?page=1&pageSize=50",
        "/api/v1/albums?page=1&pageSize=50",
        "/api/v1/songs?page=1&pageSize=50",
        "/api/v1/songs/recent?limit=50",
        "/api/v1/genres?page=1&limit=50",
        "/api/v1/user/albums/liked?page=1&limit=50",
        "/api/v1/user/stats/history?limit=50",
    ] {
        let (status, body) = response_json(state.clone(), principal, path).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
        assert!(
            !body.to_string().contains("Hidden Library"),
            "restricted facts leaked from GET {path}: {body}"
        );
    }
    for path in [
        format!("/api/v1/artists/{hidden_artist_id}"),
        format!("/api/v1/albums/{hidden_release_id}"),
        format!("/api/v1/songs/{hidden_track_id}"),
        format!("/api/v1/songs/{hidden_track_id}/lyrics"),
        format!("/api/v1/search/similar/{hidden_track_id}/song?limit=10"),
    ] {
        assert_eq!(
            response_json(state.clone(), principal, &path).await.0,
            StatusCode::NOT_FOUND,
            "GET {path}"
        );
    }
    let (status, hidden_search) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/search",
        serde_json::json!({"query":"Hidden Library", "pageSize":50}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden_search["data"]["totalCount"], 0);
    assert_eq!(
        response_status(
            state.clone(),
            principal,
            "POST",
            &format!("/api/v1/songs/setrating/{hidden_track_id}/5"),
        )
        .await,
        StatusCode::NOT_FOUND
    );
    let (status, _) = response_json_body(
        state.clone(),
        principal,
        "POST",
        "/api/v1/scrobble",
        serde_json::json!({
            "songId": hidden_track_id,
            "playerName": "acl-regression",
            "scrobbleType": "played"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, stats) = response_json(state, principal, "/api/v1/user/stats?days=30").await;
    assert_eq!(stats["totalPlays"], 0);
    assert_eq!(stats["favoriteAlbums"], 0);
    db.drop().await;
}

#[tokio::test]
async fn native_song_play_count_reads_global_play_ledger_without_user_state_row() {
    let db = common::TestDb::create("server_native_play_count").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7_301,
        CatalogFixturePresentation {
            release_title: "Global Play Count Release",
            release_artist: "Global Play Count Artist",
            track_title: "Ledger Count Track",
            track_artist: "Global Play Count Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let (state, principal) = native_test_state(&db, "global-play-count-user").await;
    let (track_id, track_public_id, release_public_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT track.id, track.public_api_id, release.public_api_id
         FROM tracks track JOIN releases release ON release.id = track.release_id
         WHERE track.release_id = $1",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("catalog Track and Release IDs");
    let played_at = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO native_track_plays
           (id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
         SELECT uuidv7(),$1,$2,format('native-play-count-%s', ordinal),$3,$3,true
         FROM generate_series(1,3) AS ordinal",
    )
    .bind(principal.user_id)
    .bind(track_id)
    .bind(played_at)
    .execute(&db.pool)
    .await
    .expect("global play facts");

    let (status, song) = response_json(
        state.clone(),
        principal,
        &format!("/api/v1/songs/{track_public_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(song["playCount"], 3);
    assert_eq!(song["userStarred"], false);
    assert_eq!(song["userRating"], 0);

    let (status, album_songs) = response_json(
        state,
        principal,
        &format!("/api/v1/albums/{release_public_id}/songs?page=1&pageSize=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(album_songs["data"][0]["id"], track_public_id.to_string());
    assert_eq!(album_songs["data"][0]["playCount"], 3);
    db.drop().await;
}

#[tokio::test]
async fn native_similar_song_preserves_captured_genre_bpm_order_scores_and_acl() {
    let db = common::TestDb::create("server_native_similar").await;
    let source = phase5_common::seed_cataloged_release(
        &db,
        7_401,
        CatalogFixturePresentation {
            release_title: "Similarity Source",
            release_artist: "Similarity Source Artist",
            track_title: "Polar Veil",
            track_artist: "Similarity Source Artist",
            release_date: "2021-01-01",
        },
    )
    .await;
    let mut candidates = Vec::new();
    for (ordinal, title, tempo) in [
        (7_402, "Solar Wind", 92.0),
        (7_403, "Midnight Sun", 88.0),
        (7_404, "Falling (feat. Guest Vocalist)", 97.0),
        (7_405, "Out of Range", 150.0),
        (7_406, "Hidden Similar", 100.0),
    ] {
        let release_title = format!("Similarity Candidate {ordinal}");
        let artist_name = format!("Similarity Artist {ordinal}");
        let fixture = phase5_common::seed_cataloged_release(
            &db,
            ordinal,
            CatalogFixturePresentation {
                release_title: &release_title,
                release_artist: &artist_name,
                track_title: title,
                track_artist: &artist_name,
                release_date: "2021-01-01",
            },
        )
        .await;
        let (track_id, public_id): (Uuid, Uuid) =
            sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
                .bind(fixture.release_id().into_uuid())
                .fetch_one(&db.pool)
                .await
                .expect("candidate Track");
        candidates.push((
            title,
            tempo,
            track_id,
            public_id,
            fixture.release_id().into_uuid(),
        ));
    }
    let (state, principal) = native_test_state(&db, "similar-user").await;
    let (source_track_id, source_public_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(source.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("source Track");
    for (track_id, tempo) in std::iter::once((source_track_id, 100.0)).chain(
        candidates
            .iter()
            .map(|(_, tempo, track_id, _, _)| (*track_id, *tempo)),
    ) {
        sqlx::query(
            "INSERT INTO native_track_audio_features (track_id,tempo,updated_at)
             VALUES ($1,$2,$3)",
        )
        .bind(track_id)
        .bind(tempo)
        .bind(OffsetDateTime::now_utc())
        .execute(&db.pool)
        .await
        .expect("Track audio feature");
    }
    for (candidate_index, play_count) in [(0_usize, 5_i32), (1, 3)] {
        sqlx::query(
            "INSERT INTO native_track_plays
               (id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
             SELECT uuidv7(),$1,$2,format('similar-%s-%s',$3,ordinal),$4,$4,true
             FROM generate_series(1,$5) ordinal",
        )
        .bind(principal.user_id)
        .bind(candidates[candidate_index].2)
        .bind(candidate_index.to_string())
        .bind(OffsetDateTime::now_utc())
        .bind(play_count)
        .execute(&db.pool)
        .await
        .expect("candidate play facts");
    }
    let hidden_library_id = Uuid::now_v7();
    let hidden_group_id = Uuid::now_v7();
    sqlx::query(
        "UPDATE release_work_items SET source_library_id=$1 WHERE id=(
           SELECT work_item_id FROM catalog_imports WHERE release_id=$2)",
    )
    .bind(hidden_library_id)
    .bind(candidates[4].4)
    .execute(&db.pool)
    .await
    .expect("hidden similarity library");
    sqlx::query(
        "INSERT INTO native_library_access_groups
           (id,public_api_id,name,created_at,updated_at) VALUES ($1,$2,$3,$4,$4)",
    )
    .bind(hidden_group_id)
    .bind(Uuid::now_v7())
    .bind("Hidden similarity group")
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("hidden similarity group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries (group_id,library_id,imported_at)
         VALUES ($1,$2,$3)",
    )
    .bind(hidden_group_id)
    .bind(hidden_library_id)
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("hidden similarity binding");

    let (status, body) = response_json(
        state,
        principal,
        &format!("/api/v1/search/similar/{source_public_id}/song?limit=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_json_contract_shape(
        &body,
        &golden(include_str!(
            "../../../compatibility/melodee-api-v1/golden/native-user-extra/native-user-extra-search-similar.json"
        )),
    );
    let similar = body["similar"].as_array().expect("similar array");
    assert_eq!(similar.len(), 3);
    assert_eq!(similar[0]["name"], "Solar Wind");
    assert_eq!(similar[1]["name"], "Midnight Sun");
    assert_eq!(similar[2]["name"], "Falling (feat. Guest Vocalist)");
    assert_eq!(similar[0]["similarityScore"], 1.0);
    assert_eq!(similar[1]["similarityScore"], 0.95);
    assert_eq!(similar[2]["similarityScore"], 0.9);
    assert!(!body.to_string().contains("Out of Range"));
    assert!(!body.to_string().contains("Hidden Similar"));
    db.drop().await;
}
