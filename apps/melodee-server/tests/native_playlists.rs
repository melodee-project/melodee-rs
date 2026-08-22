//! native-v1 playlist and smart-playlist route evidence.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode, header::CONTENT_TYPE},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport, StoredNativeUser};
use melodee_search::{SearchConfig, request_rebuild, run_rebuild_once};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use tempfile::TempDir;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

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

async fn insert_user(db: &common::TestDb, username: &str) -> StoredNativeUser {
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
                stream_signing_key: "playlist-test-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasPlaylistRole".to_string()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("insert native user")
}

fn principal(user: &StoredNativeUser) -> NativePrincipal {
    NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: user.is_admin,
        is_editor: user.is_editor,
    }
}

fn state(db: &common::TestDb, search: Option<SearchConfig>) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search,
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

async fn call(
    state: NativeState,
    principal: NativePrincipal,
    method: Method,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    let mut request = builder.body(Body::from(body)).expect("request");
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
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, body)
}

async fn json_call(
    state: NativeState,
    principal: NativePrincipal,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Option<serde_json::Value>) {
    let bytes = body
        .map(|body| serde_json::to_vec(&body).expect("serialize request"))
        .unwrap_or_default();
    let (status, response) = call(
        state.clone(),
        principal,
        method,
        uri,
        Some("application/json"),
        bytes,
    )
    .await;
    let parsed =
        (!response.is_empty()).then(|| serde_json::from_slice(&response).expect("JSON response"));
    (status, parsed)
}

async fn seed_tracks(db: &common::TestDb) -> [Uuid; 2] {
    let first = phase5_common::seed_cataloged_release(
        db,
        7301,
        CatalogFixturePresentation {
            release_title: "Playlist Release One",
            release_artist: "Playlist Artist One",
            track_title: "Playlist Track One",
            track_artist: "Playlist Artist One",
            release_date: "2026-08-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        db,
        7302,
        CatalogFixturePresentation {
            release_title: "Playlist Release Two",
            release_artist: "Playlist Artist Two",
            track_title: "Playlist Track Two",
            track_artist: "Playlist Artist Two",
            release_date: "2026-08-02",
        },
    )
    .await;
    let first_id = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
        .bind(first.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("first Track public ID");
    let second_id = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
        .bind(second.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("second Track public ID");
    [first_id, second_id]
}

#[tokio::test]
async fn playlist_crud_routes_preserve_order_dual_add_shapes_and_ownership() {
    let db = common::TestDb::create("server_native_playlists_crud").await;
    let [first, second] = seed_tracks(&db).await;
    let owner = insert_user(&db, "playlist-owner").await;
    let outsider = insert_user(&db, "playlist-outsider").await;
    let source_library_id = Uuid::now_v7();
    sqlx::query(
        "UPDATE release_work_items item SET source_library_id=$2 \
         FROM catalog_imports import JOIN tracks track ON track.release_id=import.release_id \
         WHERE item.id=import.work_item_id AND track.public_api_id=$1",
    )
    .bind(first)
    .bind(source_library_id)
    .execute(&db.pool)
    .await
    .expect("restrict first Track library");
    let group_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_library_access_groups(id,public_api_id,name,created_at,updated_at) \
         VALUES($1,$1,'playlist owner library',now(),now())",
    )
    .bind(group_id)
    .execute(&db.pool)
    .await
    .expect("access group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at) VALUES($1,$2,now())",
    )
    .bind(group_id)
    .bind(source_library_id)
    .execute(&db.pool)
    .await
    .expect("access group library");
    sqlx::query(
        "INSERT INTO native_user_library_groups(user_id,group_id,created_at) VALUES($1,$2,now())",
    )
    .bind(owner.id)
    .bind(group_id)
    .execute(&db.pool)
    .await
    .expect("owner access membership");
    let state = state(&db, None);

    let (status, _) = json_call(
        state.clone(),
        principal(&outsider),
        Method::POST,
        "/api/v1/playlists",
        Some(serde_json::json!({"name":"ACL probe","songIds":[first]})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_playlists WHERE owner_user_id=$1",
        )
        .bind(outsider.id)
        .fetch_one(&db.pool)
        .await
        .expect("outsider playlist count"),
        0,
        "an inaccessible Track must reject the entire playlist mutation"
    );

    let (status, created) = json_call(
        state.clone(),
        principal(&owner),
        Method::POST,
        "/api/v1/playlists",
        Some(serde_json::json!({
            "name": "Native CRUD",
            "comment": "ordered",
            "isPublic": false,
            "songIds": [first, second]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let created = created.expect("create body");
    let playlist_id = Uuid::parse_str(created["id"].as_str().unwrap()).expect("playlist ID");
    assert_eq!(created["songCount"], 2);
    assert_eq!(created["description"], "");
    assert_eq!(created["owner"]["id"], owner.public_api_id.to_string());
    let (status, listed) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        "/api/v1/playlists?page=1&pageSize=50",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.unwrap()["meta"]["totalCount"], 1);
    let (status, updated) = json_call(
        state.clone(),
        principal(&owner),
        Method::PUT,
        &format!("/api/v1/playlists/{playlist_id}"),
        Some(serde_json::json!({
            "name":"Native CRUD Updated","comment":"ordered","isPublic":false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let updated = updated.expect("update body");
    assert_eq!(updated["name"], "Native CRUD Updated");
    assert_eq!(updated["description"], "");

    for body in [
        serde_json::json!([first]),
        serde_json::json!({"songIds": [second]}),
    ] {
        let (status, _) = json_call(
            state.clone(),
            principal(&owner),
            Method::POST,
            &format!("/api/v1/playlists/{playlist_id}/songs"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::PUT,
        &format!("/api/v1/playlists/{playlist_id}/songs/reorder"),
        Some(serde_json::json!({"songIds": [second, first, second, first]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, songs) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}/songs?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        songs.unwrap()["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|song| song["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![second.to_string(), first.to_string()]
    );

    sqlx::query(
        "UPDATE tracks SET duration_ms=CASE public_api_id \
           WHEN $1 THEN 111000 WHEN $2 THEN 222000 END \
         WHERE public_api_id=ANY($3)",
    )
    .bind(first)
    .bind(second)
    .bind(vec![first, second])
    .execute(&db.pool)
    .await
    .expect("set distinct playlist durations");

    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::PUT,
        &format!("/api/v1/playlists/{playlist_id}"),
        Some(serde_json::json!({"isPublic":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, public_playlist) = json_call(
        state.clone(),
        principal(&outsider),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let public_playlist = public_playlist.expect("public playlist body");
    assert_eq!(public_playlist["songCount"], 1);
    assert_eq!(public_playlist["durationMs"], 222_000);
    let (status, public_songs) = json_call(
        state.clone(),
        principal(&outsider),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}/songs?page=1&pageSize=1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let public_songs = public_songs.expect("public playlist songs");
    assert_eq!(public_songs["meta"]["totalCount"], 1);
    assert_eq!(public_songs["meta"]["totalPages"], 1);
    assert_eq!(public_songs["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(public_songs["data"][0]["id"], second.to_string());

    sqlx::query(
        "UPDATE releases SET status='deprecated' \
         WHERE id=(SELECT release_id FROM tracks WHERE public_api_id=$1)",
    )
    .bind(second)
    .execute(&db.pool)
    .await
    .expect("retire second Release");
    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::POST,
        &format!("/api/v1/playlists/{playlist_id}/songs"),
        Some(serde_json::json!([second])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, retired_public_playlist) = json_call(
        state.clone(),
        principal(&outsider),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retired_public_playlist = retired_public_playlist.expect("retired playlist body");
    assert_eq!(retired_public_playlist["songCount"], 0);
    assert_eq!(retired_public_playlist["durationMs"], 0);
    let (status, retired_public_songs) = json_call(
        state.clone(),
        principal(&outsider),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}/songs?page=1&pageSize=1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retired_public_songs = retired_public_songs.expect("retired playlist songs");
    assert_eq!(retired_public_songs["meta"]["totalCount"], 0);
    assert_eq!(retired_public_songs["meta"]["totalPages"], 0);
    assert_eq!(
        retired_public_songs["data"].as_array().map(Vec::len),
        Some(0)
    );

    let (status, _) = json_call(
        state.clone(),
        principal(&outsider),
        Method::PUT,
        &format!("/api/v1/playlists/{playlist_id}"),
        Some(serde_json::json!({"name": "stolen", "comment": null, "isPublic": null})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::DELETE,
        &format!("/api/v1/playlists/{playlist_id}/songs"),
        Some(serde_json::json!([second])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, listed) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        "/api/v1/user/playlists?page=1&limit=50",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.unwrap()["meta"]["totalCount"], 1);
    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::DELETE,
        &format!("/api/v1/playlists/{playlist_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json_call(
        state,
        principal(&owner),
        Method::GET,
        &format!("/api/v1/playlists/{playlist_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.expect("not-found body")["message"],
        "Playlist not found"
    );
    db.drop().await;
}

#[tokio::test]
async fn playlist_import_and_image_routes_are_bounded_and_content_safe() {
    let db = common::TestDb::create("server_native_playlists_uploads").await;
    let [track_id, _] = seed_tracks(&db).await;
    let owner = insert_user(&db, "playlist-upload-owner").await;
    let state = state(&db, None);
    let (status, created) = json_call(
        state.clone(),
        principal(&owner),
        Method::POST,
        "/api/v1/playlists",
        Some(serde_json::json!({"name":"Images","comment":null,"songIds":[]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let playlist_id = Uuid::parse_str(
        created.unwrap()["id"]
            .as_str()
            .expect("created playlist ID"),
    )
    .expect("UUID");

    for uri in [
        format!("/api/v1/playlists/{playlist_id}/image"),
        "/api/v1/playlists/import".to_string(),
    ] {
        let response = melodee_server::native::router()
            .with_state(state.clone())
            .oneshot({
                let mut request = Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("empty multipart request");
                request.extensions_mut().insert(principal(&owner));
                request
                    .extensions_mut()
                    .insert(NativeClientBinding("127.0.0.1".to_owned()));
                request
            })
            .await
            .expect("empty multipart response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                "application/problem+json; charset=utf-8"
            ))
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("empty multipart body")
            .to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).expect("validation body");
        assert_eq!(
            body["type"],
            "https://tools.ietf.org/html/rfc9110#section-15.5.1"
        );
        assert_eq!(body["title"], "One or more validation errors occurred.");
        assert_eq!(body["status"], 400);
        assert_eq!(
            body["errors"]["file"],
            serde_json::json!(["The file field is required."])
        );
        assert!(
            body["traceId"]
                .as_str()
                .is_some_and(|value| value.starts_with("00-"))
        );
    }

    let boundary = "playlist-image-test";
    let svg = multipart_body(boundary, "cover.png", b"<svg onload=alert(1)>");
    let (status, _) = call(
        state.clone(),
        principal(&owner),
        Method::POST,
        &format!("/api/v1/playlists/{playlist_id}/image"),
        Some(&format!("multipart/form-data; boundary={boundary}")),
        svg,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let truncated_png = multipart_body(
        boundary,
        "cover.png",
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0],
    );
    let (status, _) = call(
        state.clone(),
        principal(&owner),
        Method::POST,
        &format!("/api/v1/playlists/{playlist_id}/image"),
        Some(&format!("multipart/form-data; boundary={boundary}")),
        truncated_png,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let png = multipart_body(
        boundary,
        "cover.png",
        include_bytes!(
            "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin"
        ),
    );
    let (status, _) = call(
        state.clone(),
        principal(&owner),
        Method::POST,
        &format!("/api/v1/playlists/{playlist_id}/image"),
        Some(&format!("multipart/form-data; boundary={boundary}")),
        png,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stored_mime: Option<String> =
        sqlx::query_scalar("SELECT image_mime_type FROM native_playlists WHERE public_api_id=$1")
            .bind(playlist_id)
            .fetch_one(&db.pool)
            .await
            .expect("stored image MIME");
    assert_eq!(stored_mime.as_deref(), Some("image/png"));
    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::DELETE,
        &format!("/api/v1/playlists/{playlist_id}/image"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let source: String = sqlx::query_scalar(
        "SELECT COALESCE(track.source_path, media.canonical_relative_path)
         FROM tracks track JOIN media_assets media ON media.id=track.playable_media_asset_id
         WHERE track.public_api_id=$1",
    )
    .bind(track_id)
    .fetch_one(&db.pool)
    .await
    .expect("Track source reference");
    let file_name = source.rsplit('/').next().expect("source basename");
    let m3u = format!("#EXTM3U\n#EXTINF:1,Known\n{file_name}\nmissing.flac\n");
    let upload = multipart_body(boundary, "portable.m3u8", m3u.as_bytes());
    let (status, response) = call(
        state.clone(),
        principal(&owner),
        Method::POST,
        "/api/v1/playlists/import?playlistName=Portable",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        upload,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response: serde_json::Value = serde_json::from_slice(&response).expect("import JSON");
    assert_eq!(response["totalEntries"], 2);
    assert_eq!(response["matchedCount"], 1);
    assert_eq!(
        response["missingReferences"],
        serde_json::json!(["missing.flac"])
    );
    let matching_plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON, COSTS OFF)
         SELECT file_name.track_id FROM native_track_file_names file_name
         WHERE file_name.normalized_file_name=$1 ORDER BY file_name.track_id",
    )
    .bind(file_name.to_ascii_lowercase())
    .fetch_one(&db.pool)
    .await
    .expect("indexed M3U lookup plan");
    assert!(
        matching_plan
            .to_string()
            .contains("native_track_file_names_lookup_idx"),
        "M3U basename lookup must remain index-backed: {matching_plan}"
    );
    sqlx::query(
        "UPDATE releases SET status='deprecated' \
         WHERE id=(SELECT release_id FROM tracks WHERE public_api_id=$1)",
    )
    .bind(track_id)
    .execute(&db.pool)
    .await
    .expect("retire imported Track Release");
    let retired_m3u = format!("#EXTM3U\n{file_name}\n");
    let retired_upload = multipart_body(boundary, "retired.m3u8", retired_m3u.as_bytes());
    let (status, retired_response) = call(
        state,
        principal(&owner),
        Method::POST,
        "/api/v1/playlists/import?playlistName=Retired",
        Some(&format!("multipart/form-data; boundary={boundary}")),
        retired_upload,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retired_response: serde_json::Value =
        serde_json::from_slice(&retired_response).expect("retired import JSON");
    assert_eq!(retired_response["matchedCount"], 0);
    assert_eq!(
        retired_response["missingReferences"],
        serde_json::json!([file_name])
    );
    db.drop().await;
}

#[tokio::test]
async fn smart_playlist_routes_validate_and_evaluate_bounded_mql() {
    let db = common::TestDb::create("server_native_smart_playlists").await;
    let [track_id, _] = seed_tracks(&db).await;
    let owner = insert_user(&db, "smart-owner").await;
    let reader = insert_user(&db, "smart-reader").await;
    let search_root = TempDir::new().expect("search root");
    let config = SearchConfig {
        search_root: search_root.path().join("search"),
        enabled: true,
        ..SearchConfig::default()
    };
    request_rebuild(&config, &db.pool, "native-smart-playlist-test")
        .await
        .expect("request rebuild");
    assert!(
        run_rebuild_once(&config, &db.pool)
            .await
            .expect("run rebuild")
            .is_some()
    );
    let state = state(&db, Some(config));

    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::POST,
        "/api/v1/playlists/smart",
        Some(serde_json::json!({
            "name":"Invalid","mqlQuery":"(".repeat(4097),"entityType":"songs"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, created) = json_call(
        state.clone(),
        principal(&owner),
        Method::POST,
        "/api/v1/playlists/smart",
        Some(serde_json::json!({
            "name":"Track Smart","mqlQuery":"track:Playlist","entityType":"songs","isPublic":false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let smart_id = Uuid::parse_str(
        created.unwrap()["apiKey"]
            .as_str()
            .expect("smart playlist ID"),
    )
    .expect("UUID");
    let (status, fetched) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        &format!("/api/v1/playlists/smart/{smart_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched.unwrap()["apiKey"], smart_id.to_string());
    let (status, evaluated) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        &format!("/api/v1/playlists/smart/{smart_id}/evaluate?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let evaluated = evaluated.expect("evaluate body");
    assert!(evaluated["meta"]["totalCount"].as_i64().unwrap() >= 1);
    assert!(evaluated["data"].as_array().unwrap().iter().any(|song| {
        song["id"] == track_id.to_string() && song["streamUrl"].as_str().is_some()
    }));
    assert!(
        !evaluated["playlist"]["lastEvaluatedAt"]
            .as_str()
            .unwrap()
            .is_empty()
    );

    let (status, _) = json_call(
        state.clone(),
        principal(&owner),
        Method::PUT,
        &format!("/api/v1/playlists/smart/{smart_id}"),
        Some(serde_json::json!({"name":"Updated Smart","isPublic":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_call(
        state.clone(),
        principal(&reader),
        Method::GET,
        &format!("/api/v1/playlists/smart/{smart_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "public smart playlist is readable");
    let before_reader_evaluation: (i64, i32, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT revision,last_result_count,last_evaluated_at
         FROM native_smart_playlists WHERE public_api_id=$1",
    )
    .bind(smart_id)
    .fetch_one(&db.pool)
    .await
    .expect("smart metadata before public-reader evaluation");
    let (status, reader_result) = json_call(
        state.clone(),
        principal(&reader),
        Method::GET,
        &format!("/api/v1/playlists/smart/{smart_id}/evaluate?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader_result.expect("public evaluation")["data"].is_array());
    let after_reader_evaluation: (i64, i32, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT revision,last_result_count,last_evaluated_at
         FROM native_smart_playlists WHERE public_api_id=$1",
    )
    .bind(smart_id)
    .fetch_one(&db.pool)
    .await
    .expect("smart metadata after public-reader evaluation");
    assert_eq!(after_reader_evaluation, before_reader_evaluation);

    let restricted_library_id = Uuid::now_v7();
    let restricted_work_item_id: Uuid = sqlx::query_scalar(
        "SELECT catalog_import.work_item_id
         FROM tracks track
         JOIN catalog_imports catalog_import ON catalog_import.release_id=track.release_id
         WHERE track.public_api_id=$1",
    )
    .bind(track_id)
    .fetch_one(&db.pool)
    .await
    .expect("smart result source work item");
    sqlx::query("UPDATE release_work_items SET source_library_id=$1 WHERE id=$2")
        .bind(restricted_library_id)
        .bind(restricted_work_item_id)
        .execute(&db.pool)
        .await
        .expect("restrict smart result source");
    let restricted_group_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_library_access_groups
           (id,public_api_id,name,created_at,updated_at)
         VALUES($1,$2,'Restricted smart library',now(),now())",
    )
    .bind(restricted_group_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("smart library group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at)
         VALUES($1,$2,now())",
    )
    .bind(restricted_group_id)
    .bind(restricted_library_id)
    .execute(&db.pool)
    .await
    .expect("smart library binding");
    let (status, _) = json_call(
        state.clone(),
        principal(&reader),
        Method::GET,
        &format!("/api/v1/playlists/smart/{smart_id}/evaluate?page=1&pageSize=50"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_call(
        state.clone(),
        principal(&reader),
        Method::DELETE,
        &format!("/api/v1/playlists/smart/{smart_id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "only its owner may delete it"
    );
    let (status, listed) = json_call(
        state.clone(),
        principal(&owner),
        Method::GET,
        "/api/v1/playlists/smart?page=1&pageSize=50&orderBy=CreatedAt&orderDirection=desc",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.unwrap()["data"][0]["name"], "Updated Smart");
    let (status, _) = json_call(
        state,
        principal(&owner),
        Method::DELETE,
        &format!("/api/v1/playlists/smart/{smart_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    db.drop().await;
}

fn multipart_body(boundary: &str, file_name: &str, content: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}
