//! Phase 7 native-v1 request/share/preferences HTTP and authorization evidence.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use catalog_common::CatalogFixturePresentation;
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
                stream_signing_key: "community-test-stream-key",
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

fn principal(user: &StoredNativeUser) -> NativePrincipal {
    NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    }
}

fn state(db: &common::TestDb) -> NativeState {
    NativeState {
        pool: db.pool.clone(),
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
        "127.0.0.1:41001".parse::<SocketAddr>().expect("test peer"),
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

fn assert_same_shape(actual: &serde_json::Value, expected: &serde_json::Value) {
    if expected.as_str() == Some("<volatile>") {
        return;
    }
    match (actual, expected) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(expected)) => {
            let mut actual_keys = actual.keys().collect::<Vec<_>>();
            let mut expected_keys = expected.keys().collect::<Vec<_>>();
            actual_keys.sort_unstable();
            expected_keys.sort_unstable();
            assert_eq!(actual_keys, expected_keys, "object wire keys differ");
            for (key, expected_value) in expected {
                assert_same_shape(&actual[key], expected_value);
            }
        }
        (serde_json::Value::Array(actual), serde_json::Value::Array(expected)) => {
            if let (Some(actual), Some(expected)) = (actual.first(), expected.first()) {
                assert_same_shape(actual, expected);
            }
        }
        (serde_json::Value::Null, serde_json::Value::Null)
        | (serde_json::Value::Bool(_), serde_json::Value::Bool(_))
        | (serde_json::Value::Number(_), serde_json::Value::Number(_))
        | (serde_json::Value::String(_), serde_json::Value::String(_)) => {}
        _ => panic!("wire JSON type differs: actual={actual:?}, expected={expected:?}"),
    }
}

#[tokio::test]
async fn native_community_routes_are_owner_fenced_bounded_and_replay_safe() {
    let db = common::TestDb::create("server_native_community").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7301,
        CatalogFixturePresentation {
            release_title: "Community API Release",
            release_artist: "Community API Artist",
            track_title: "Community API Track",
            track_artist: "Community API Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let owner = import_user(&db, "community-owner-http").await;
    let other = import_user(&db, "community-other-http").await;
    let state = state(&db);
    let owner_principal = principal(&owner);
    let other_principal = principal(&other);
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Track public ID");
    let release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Release public ID");
    let artist_public_id: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM release_artists billing
         JOIN artists artist ON artist.id=billing.artist_id
         WHERE billing.release_id=$1 ORDER BY billing.presentation_order LIMIT 1",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("Artist public ID");

    let (_, default_settings) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        "/api/v1/playback/settings",
        None,
    )
    .await;
    let legacy_default_settings: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/melodee-api-v1/golden/native-community/community-lifecycle-playback-settings-default.json"
    ))
    .expect("legacy playback settings");
    assert_eq!(default_settings.unwrap(), legacy_default_settings);

    let (_, empty_presets) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        "/api/v1/equalizer/presets?page=1&limit=20",
        None,
    )
    .await;
    let legacy_empty_presets: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/melodee-api-v1/golden/native-community/community-lifecycle-equalizer-list-empty.json"
    ))
    .expect("legacy empty presets");
    assert_eq!(empty_presets.unwrap(), legacy_empty_presets);

    let (status, _) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/playback/settings",
        Some(serde_json::json!({
            "crossfadeDuration": 4.5,
            "gaplessPlayback": true,
            "volumeNormalization": true,
            "replayGain": "track",
            "audioQuality": "lossless",
            "equalizerPreset": null
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, settings) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        "/api/v1/playback/settings",
        None,
    )
    .await;
    assert_eq!(settings.unwrap()["crossfadeDuration"], 4.5);

    let preset_body = serde_json::json!({
        "name": "Headphones",
        "bands": [{"frequency": 60.0, "gain": 2.0}],
        "isDefault": true
    });
    let (_, first_preset) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/equalizer/presets",
        Some(preset_body.clone()),
    )
    .await;
    let (_, replayed_preset) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/equalizer/presets",
        Some(preset_body),
    )
    .await;
    assert_eq!(first_preset.unwrap()["id"], replayed_preset.unwrap()["id"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_equalizer_presets WHERE user_id = $1 AND name = 'Headphones'",
        )
        .bind(owner.id)
        .fetch_one(&db.pool)
        .await
        .expect("preset count"),
        1
    );

    let (status, created_request) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/requests",
        Some(serde_json::json!({"category": 5, "description": "Please add liner notes"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let created_request = created_request.unwrap();
    let legacy_request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/melodee-api-v1/golden/native-community/community-lifecycle-request-create.json"
    ))
    .expect("legacy request");
    assert_same_shape(&created_request, &legacy_request);
    assert_eq!(created_request["category"], legacy_request["category"]);
    assert_eq!(created_request["status"], legacy_request["status"]);
    assert_eq!(
        created_request["lastActivityType"],
        legacy_request["lastActivityType"]
    );
    let request_id = created_request["apiKey"]
        .as_str()
        .expect("request ID")
        .parse::<Uuid>()
        .expect("UUID");
    let (status, requests) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        "/api/v1/requests?page=1&pageSize=20&mine=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        requests.expect("request page")["data"][0]["apiKey"],
        request_id.to_string()
    );
    let (status, detail) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        &format!("/api/v1/requests/{request_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        detail.expect("request detail")["apiKey"],
        request_id.to_string()
    );
    let (status, _) = call(
        state.clone(),
        Some(other_principal),
        Method::PUT,
        &format!("/api/v1/requests/{request_id}"),
        Some(serde_json::json!({"description": "stolen"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, updated) = call(
        state.clone(),
        Some(owner_principal),
        Method::PUT,
        &format!("/api/v1/requests/{request_id}"),
        Some(serde_json::json!({"description": "Updated request"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated.expect("updated request")["description"],
        "Updated request"
    );
    let (status, _) = call(
        state.clone(),
        Some(other_principal),
        Method::POST,
        &format!("/api/v1/requests/{request_id}/comments"),
        Some(serde_json::json!({"body": "I can help"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, comments) = call(
        state.clone(),
        Some(owner_principal),
        Method::GET,
        &format!("/api/v1/requests/{request_id}/comments?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        comments.expect("comments")["data"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::GET,
            "/api/v1/requests/activity",
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::GET,
            "/api/v1/requests/activity/unread?page=1&pageSize=10",
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, _) = call(
        state.clone(),
        Some(other_principal),
        Method::POST,
        &format!("/api/v1/requests/{request_id}/seen"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::POST,
            &format!("/api/v1/requests/{request_id}/complete"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, delete_request) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/requests",
        Some(serde_json::json!({"category": 5, "description": "Delete me"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let delete_request = delete_request.expect("delete request");
    let delete_request_id = delete_request["apiKey"]
        .as_str()
        .expect("delete request ID");
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::DELETE,
            &format!("/api/v1/requests/{delete_request_id}"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );

    let (status, share) = call(
        state.clone(),
        Some(owner_principal),
        Method::POST,
        "/api/v1/shares",
        Some(serde_json::json!({
            "shareType": 1,
            "resourceId": track_public_id,
            "description": "Public sample",
            "isDownloadable": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let share = share.unwrap();
    let legacy_share: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/melodee-api-v1/golden/native-community/community-lifecycle-share-create.json"
    ))
    .expect("legacy share");
    assert_same_shape(&share, &legacy_share);
    assert_eq!(share["shareType"], legacy_share["shareType"]);
    let share_id = share["id"].as_str().expect("share ID");
    let token = share["shareUrl"]
        .as_str()
        .expect("share URL")
        .rsplit('/')
        .next()
        .expect("share token");
    assert_eq!(token.len(), 32);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::GET,
            "/api/v1/shares?page=1&pageSize=20",
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::GET,
            &format!("/api/v1/shares/{share_id}"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, updated_share) = call(
        state.clone(),
        Some(owner_principal),
        Method::PUT,
        &format!("/api/v1/shares/{share_id}"),
        Some(serde_json::json!({
            "description": "Updated public sample",
            "isDownloadable": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated_share.expect("updated share")["isDownloadable"],
        true
    );
    let (status, _) = call(
        state.clone(),
        Some(other_principal),
        Method::DELETE,
        &format!("/api/v1/shares/{share_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let public_path = share["shareUrl"].as_str().expect("share URL");
    let (status, public) = call(state.clone(), None, Method::GET, public_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public.unwrap()["song"]["id"], track_public_id.to_string());
    sqlx::query(
        "UPDATE native_shares SET expires_at=now()-interval '1 second'
         WHERE public_api_id=$1",
    )
    .bind(share_id.parse::<Uuid>().expect("share UUID"))
    .execute(&db.pool)
    .await
    .expect("expire share");
    let expired = call(state.clone(), None, Method::GET, public_path, None).await;
    let unknown = call(
        state.clone(),
        None,
        Method::GET,
        "/share/00000000000000000000000000000000",
        None,
    )
    .await;
    let malformed = call(
        state.clone(),
        None,
        Method::GET,
        "/share/not-a-valid-bearer",
        None,
    )
    .await;
    assert_eq!(expired.0, StatusCode::NOT_FOUND);
    for response in [&expired, &unknown, &malformed] {
        assert_eq!(response.0, StatusCode::NOT_FOUND);
        let body = response.1.as_ref().expect("uniform public-share error");
        assert_eq!(body["code"], "NOT_FOUND");
        assert_eq!(body["message"], "Resource not found");
    }
    assert_eq!(
        call(
            state.clone(),
            Some(owner_principal),
            Method::DELETE,
            &format!("/api/v1/shares/{share_id}"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    let revoked = call(state.clone(), None, Method::GET, public_path, None).await;
    assert_eq!(revoked.0, StatusCode::NOT_FOUND);
    assert_eq!(
        revoked.1.as_ref().map(|body| &body["code"]),
        Some(&serde_json::json!("NOT_FOUND"))
    );
    assert_eq!(
        revoked.1.as_ref().map(|body| &body["message"]),
        Some(&serde_json::json!("Resource not found")),
        "deleted bearers use the uniform public result without reusing correlation IDs"
    );

    let playlist_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_playlists (
             id, public_api_id, owner_user_id, name, description,
             is_public, created_at, updated_at
         ) VALUES ($1,$1,$2,'Private Shared Playlist','Shared only by opaque link',false,now(),now())",
    )
    .bind(playlist_id)
    .bind(owner.id)
    .execute(&db.pool)
    .await
    .expect("private shared playlist");
    sqlx::query(
        "INSERT INTO native_playlist_items (
             id,playlist_id,track_id,position,added_by_user_id,created_at
         ) VALUES ($1,$2,(SELECT id FROM tracks WHERE public_api_id=$3),0,$4,now())",
    )
    .bind(Uuid::now_v7())
    .bind(playlist_id)
    .bind(track_public_id)
    .bind(owner.id)
    .execute(&db.pool)
    .await
    .expect("private shared playlist item");
    let mut public_share_paths = std::collections::HashMap::new();
    for (share_type, resource_id, payload_key) in [
        (1_i16, track_public_id, "song"),
        (2_i16, release_public_id, "album"),
        (3_i16, playlist_id, "playlist"),
        (4_i16, artist_public_id, "artist"),
    ] {
        let (status, share) = call(
            state.clone(),
            Some(owner_principal),
            Method::POST,
            "/api/v1/shares",
            Some(serde_json::json!({
                "shareType": share_type,
                "resourceId": resource_id,
                "description": "Resolved public share",
                "isDownloadable": false
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "share type {share_type}");
        let share = share.unwrap();
        let public_path = share["shareUrl"].as_str().expect("share URL");
        public_share_paths.insert(share_type, public_path.to_owned());
        let (status, public) = call(state.clone(), None, Method::GET, public_path, None).await;
        assert_eq!(status, StatusCode::OK, "public share type {share_type}");
        assert_eq!(
            public.unwrap()[payload_key]["id"],
            resource_id.to_string(),
            "anonymous public resolution returns the preserved resource identity"
        );
    }

    let restricted_library = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET source_library_id=$2 WHERE id=$1")
        .bind(fixture.work_item_id())
        .bind(restricted_library)
        .execute(&db.pool)
        .await
        .expect("assign fixture library");
    let access_group = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_library_access_groups
         (id,public_api_id,name,created_at,updated_at) VALUES ($1,$1,'restricted',now(),now())",
    )
    .bind(access_group)
    .execute(&db.pool)
    .await
    .expect("restricted group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries (group_id,library_id,imported_at)
         VALUES ($1,$2,now())",
    )
    .bind(access_group)
    .bind(restricted_library)
    .execute(&db.pool)
    .await
    .expect("restricted library binding");
    for share_type in 1_i16..=4 {
        let unauthorized = call(
            state.clone(),
            None,
            Method::GET,
            public_share_paths
                .get(&share_type)
                .map(String::as_str)
                .expect("public share path"),
            None,
        )
        .await;
        assert_eq!(unauthorized.0, StatusCode::NOT_FOUND);
        assert_eq!(
            unauthorized.1.as_ref().map(|body| &body["code"]),
            Some(&serde_json::json!("NOT_FOUND")),
            "a valid bearer cannot reveal a resource its owner can no longer access"
        );
        assert_eq!(
            unauthorized.1.as_ref().map(|body| &body["message"]),
            Some(&serde_json::json!("Resource not found"))
        );
    }

    sqlx::query(
        "INSERT INTO native_user_library_groups (user_id,group_id,created_at)
         VALUES ($1,$2,now())",
    )
    .bind(owner.id)
    .bind(access_group)
    .execute(&db.pool)
    .await
    .expect("restore owner library access");
    for public_path in public_share_paths.values() {
        assert_eq!(
            call(state.clone(), None, Method::GET, public_path, None)
                .await
                .0,
            StatusCode::OK,
            "restoring owner access restores the opaque capability"
        );
    }

    // Hold resolution after its authorization and DTO reads, then prove a
    // concurrent lifecycle revocation cannot commit until that consistent
    // public-share transaction has finished. This deterministically exercises
    // the auth/load interleaving that previously produced a torn 200.
    const SHARE_BARRIER_KEY: i64 = 7_301_009;
    sqlx::query(
        "CREATE FUNCTION native_share_test_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock(7301009);
           RETURN NEW;
         END;
         $$",
    )
    .execute(&db.pool)
    .await
    .expect("install public-share concurrency barrier function");
    sqlx::query(
        "CREATE TRIGGER native_share_test_barrier
           BEFORE UPDATE OF visit_count ON native_shares
           FOR EACH ROW EXECUTE FUNCTION native_share_test_barrier()",
    )
    .execute(&db.pool)
    .await
    .expect("install public-share concurrency barrier trigger");
    let mut barrier_connection = db.pool.acquire().await.expect("barrier connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SHARE_BARRIER_KEY)
        .execute(&mut *barrier_connection)
        .await
        .expect("hold public-share concurrency barrier");
    let concurrent_state = state.clone();
    let concurrent_playlist_path = public_share_paths
        .get(&3)
        .expect("Playlist share path")
        .clone();
    let public_resolution = tokio::spawn(async move {
        call(
            concurrent_state,
            None,
            Method::GET,
            &concurrent_playlist_path,
            None,
        )
        .await
    });
    let mut reached_barrier = false;
    for _ in 0..100 {
        reached_barrier = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("inspect public-share concurrency barrier");
        if reached_barrier {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        reached_barrier,
        "public resolution must reach the post-DTO transaction barrier"
    );
    let mut revocation_connection = db.pool.acquire().await.expect("revocation connection");
    sqlx::query("BEGIN")
        .execute(&mut *revocation_connection)
        .await
        .expect("begin concurrent revocation transaction");
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *revocation_connection)
        .await
        .expect("bound concurrent revocation wait");
    let revoke_error =
        sqlx::query("DELETE FROM native_user_library_groups WHERE user_id=$1 AND group_id=$2")
            .bind(owner.id)
            .bind(access_group)
            .execute(&mut *revocation_connection)
            .await
            .expect_err("owner ACL revocation must wait for the public Playlist snapshot");
    assert_eq!(
        revoke_error
            .as_database_error()
            .and_then(|error| error.code()),
        Some(std::borrow::Cow::Borrowed("55P03"))
    );
    sqlx::query("ROLLBACK")
        .execute(&mut *revocation_connection)
        .await
        .expect("clear failed revocation transaction");
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(SHARE_BARRIER_KEY)
        .fetch_one(&mut *barrier_connection)
        .await
        .expect("release public-share concurrency barrier");
    assert!(unlocked);
    let resolved = public_resolution.await.expect("public resolution task");
    assert_eq!(resolved.0, StatusCode::OK);
    sqlx::query("DELETE FROM native_user_library_groups WHERE user_id=$1 AND group_id=$2")
        .bind(owner.id)
        .bind(access_group)
        .execute(&mut *revocation_connection)
        .await
        .expect("revoke owner ACL after public snapshot");
    drop(revocation_connection);
    drop(barrier_connection);
    assert_eq!(
        call(
            state.clone(),
            None,
            Method::GET,
            public_share_paths
                .get(&3)
                .map(String::as_str)
                .expect("Playlist share path"),
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "once ACL revocation commits, the same public URL cannot expose nested Tracks"
    );
    sqlx::query("DROP TRIGGER native_share_test_barrier ON native_shares")
        .execute(&db.pool)
        .await
        .expect("remove public-share concurrency barrier trigger");
    sqlx::query("DROP FUNCTION native_share_test_barrier()")
        .execute(&db.pool)
        .await
        .expect("remove public-share concurrency barrier function");
    sqlx::query(
        "INSERT INTO native_user_library_groups (user_id,group_id,created_at)
         VALUES ($1,$2,now())",
    )
    .bind(owner.id)
    .bind(access_group)
    .execute(&db.pool)
    .await
    .expect("restore owner library access after concurrency regression");

    sqlx::query("UPDATE releases SET status='deprecated',updated_at=now() WHERE id=$1")
        .bind(fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("deprecate shared Release");
    for share_type in [1_i16, 2, 3] {
        let deprecated = call(
            state.clone(),
            None,
            Method::GET,
            public_share_paths
                .get(&share_type)
                .map(String::as_str)
                .expect("public share path"),
            None,
        )
        .await;
        assert_eq!(deprecated.0, StatusCode::NOT_FOUND);
        assert_eq!(
            deprecated.1.as_ref().map(|body| &body["message"]),
            Some(&serde_json::json!("Resource not found")),
            "inactive direct and nested catalog targets use the uniform public result"
        );
    }
    assert_eq!(
        call(
            state.clone(),
            None,
            Method::GET,
            public_share_paths
                .get(&4)
                .map(String::as_str)
                .expect("Artist share path"),
            None,
        )
        .await
        .0,
        StatusCode::OK,
        "a still-active Artist remains independently shareable"
    );
    sqlx::query("UPDATE artists SET status='deprecated',updated_at=now() WHERE public_api_id=$1")
        .bind(artist_public_id)
        .execute(&db.pool)
        .await
        .expect("deprecate shared Artist");
    let deprecated_artist = call(
        state.clone(),
        None,
        Method::GET,
        public_share_paths
            .get(&4)
            .map(String::as_str)
            .expect("Artist share path"),
        None,
    )
    .await;
    assert_eq!(deprecated_artist.0, StatusCode::NOT_FOUND);
    assert_eq!(
        deprecated_artist.1.as_ref().map(|body| &body["message"]),
        Some(&serde_json::json!("Resource not found"))
    );

    let (status, _) = call(
        state,
        Some(owner_principal),
        Method::GET,
        "/api/v1/requests?page=1&pageSize=501",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    db.drop().await;
}
