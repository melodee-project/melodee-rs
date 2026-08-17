//! Phase 7 native discovery/audio/chart/backend/theme route integration.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{io::Write, net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    http::{Request, StatusCode},
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
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        track_public_id: Uuid,
        user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async move {
            Ok(format!(
                "/song/stream/{track_public_id}/{user_public_id}/test"
            ))
        })
    }
}

async fn request(
    state: NativeState,
    principal: Option<NativePrincipal>,
    method: &str,
    path: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    request_with_content_type(state, principal, method, path, "application/json", body).await
}

async fn request_with_content_type(
    state: NativeState,
    principal: Option<NativePrincipal>,
    method: &str,
    path: &str,
    content_type: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", content_type)
        .body(body)
        .expect("request");
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
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn theme_zip() -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    writer.start_file("theme.json", options).unwrap();
    writer
        .write_all(br#"{"id":"imported","name":"Imported","author":"Melodee","version":"1"}"#)
        .unwrap();
    writer.start_file("theme.css", options).unwrap();
    writer.write_all(b"body { color: #123; }").unwrap();
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn native_discovery_routes_use_durable_bounded_facts_and_preserve_wire_shapes() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let db = common::TestDb::create("server_native_discovery").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7501,
        CatalogFixturePresentation {
            release_title: "Discovery Release",
            release_artist: "Discovery Artist",
            track_title: "Discovery Track",
            track_artist: "Discovery Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "discovery-admin",
                normalized_username: "DISCOVERYADMIN",
                email: "discovery@example.test",
                normalized_email: "DISCOVERY@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-key",
                is_admin: true,
                is_editor: true,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["admin".to_owned()],
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
    let track_id: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("Track");
    let track_public_id: Uuid = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE id=$1")
        .bind(track_id)
        .fetch_one(&db.pool)
        .await
        .expect("Track public ID");
    let release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("Release public ID");
    sqlx::query(
        "INSERT INTO native_track_audio_features(track_id,tempo,musical_key,mode,time_signature,updated_at) \
         VALUES($1,128,'C','major',4,now())",
    )
    .bind(track_id)
    .execute(&db.pool)
    .await
    .expect("feature");
    let chart_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_charts(id,public_api_id,slug,title,is_generated_playlist_enabled,created_at,updated_at) \
         VALUES($1,$2,'discovery','Discovery Chart',true,now(),now())",
    )
    .bind(chart_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("chart");
    let artist_id: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 AND role='primary' \
         ORDER BY presentation_order LIMIT 1",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("artist");
    sqlx::query(
        "INSERT INTO native_chart_items(chart_id,rank,artist_name,release_title,\
         linked_artist_id,linked_release_id,created_at) \
         VALUES($1,1,'Discovery Artist','Discovery Release',$2,$3,now())",
    )
    .bind(chart_id)
    .bind(artist_id)
    .bind(fixture.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("chart item");
    sqlx::query(
        "INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,\
         submitted_at,duration_seconds,completed) \
         VALUES($1,$2,$3,'discovery-play',now()-interval '1 minute',now(),123,true)",
    )
    .bind(Uuid::now_v7())
    .bind(user.id)
    .bind(track_id)
    .execute(&db.pool)
    .await
    .expect("play");
    let session_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_playback_backend_state(singleton,enabled,initialized,session_public_api_id,updated_at) \
         VALUES(true,true,false,$1,now())",
    )
    .bind(session_id)
    .execute(&db.pool)
    .await
    .expect("backend");
    sqlx::query(
        "INSERT INTO native_themes(id,name,author,version,is_built_in,created_at,updated_at) \
         VALUES('fixture','Fixture','Melodee','1',false,now(),now())",
    )
    .execute(&db.pool)
    .await
    .expect("theme");
    sqlx::query(
        "INSERT INTO native_theme_assets(theme_id,relative_path,mime_type,bytes) \
         VALUES('fixture','theme.css','text/css',$1)",
    )
    .bind(b"body{}".as_slice())
    .execute(&db.pool)
    .await
    .expect("theme asset");
    sqlx::query(
        "INSERT INTO artist_external_identities(
           id,artist_id,provider,external_id,confidence,evidence_source,created_at)
         VALUES($1,$2,'itunes','1825315582',9000,'phase7-test',now())",
    )
    .bind(Uuid::now_v7())
    .bind(artist_id)
    .execute(&db.pool)
    .await
    .expect("artist identity");

    let state = NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
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
            .expect("auth config"),
        ),
    };

    for path in [
        format!("/api/v1/audio/features/{track_public_id}"),
        "/api/v1/audio/bpm?min=120&max=130&page=1&limit=10".to_owned(),
        "/api/v1/charts?page=1&pageSize=10".to_owned(),
        "/api/v1/charts/discovery".to_owned(),
        "/api/v1/charts/discovery/playlist".to_owned(),
        "/api/v1/recommendations?limit=10&type=song".to_owned(),
        "/api/v1/system/stats".to_owned(),
        "/api/v1/analytics/listening?period=week".to_owned(),
    ] {
        let (status, bytes) =
            request(state.clone(), Some(principal), "GET", &path, Body::empty()).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("JSON");
    }

    let (_, bytes) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/analytics/listening?period=week",
        Body::empty(),
    )
    .await;
    let listening: serde_json::Value = serde_json::from_slice(&bytes).expect("analytics");
    assert_eq!(listening["totalTracksPlayed"], 1);
    assert_eq!(listening["totalPlayTime"], 123.0);
    assert_eq!(listening["topArtists"][0]["name"], "Discovery Artist");
    assert_eq!(listening["topAlbums"][0]["name"], "Discovery Release");
    assert_eq!(listening["topGenres"][0]["name"], "Fixture");
    assert!(
        listening["listeningByTimeOfDay"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["playTime"] == 123.0)
    );

    let (status, bytes) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/artist-lookup/providers",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let providers: serde_json::Value = serde_json::from_slice(&bytes).expect("providers");
    assert_eq!(providers.as_array().unwrap().len(), 7);
    assert_eq!(providers[3]["displayName"], "ITunes Search Engine");
    assert_eq!(providers[3]["isEnabled"], true);

    let (status, bytes) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/artist-lookup",
        Body::from(r#"{"artistName":"Discovery Artist","limit":5}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let lookup: serde_json::Value = serde_json::from_slice(&bytes).expect("lookup");
    assert_eq!(lookup["candidates"][0]["name"], "Discovery Artist");
    assert_eq!(
        lookup["candidates"][0]["providerId"],
        "36E4DB7A-18BB-4A7D-86F7-A3C653A21611"
    );
    assert_eq!(lookup["candidates"][0]["itunesId"], "1825315582");

    let (status, bytes) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/system/stats",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stats: serde_json::Value = serde_json::from_slice(&bytes).expect("stats");
    assert_eq!(stats[0]["type"], 4);
    assert_eq!(stats[0]["title"], "Albums");
    assert_eq!(stats[1]["title"], "Artists");
    assert_eq!(stats[2]["title"], "Playlists");
    assert_eq!(stats[3]["title"], "Songs");
    assert_eq!(stats[3]["sortOrder"], 9);

    for content_type in ["album", "artist"] {
        let (status, bytes) = request(
            state.clone(),
            Some(principal),
            "GET",
            &format!("/api/v1/analytics/top/week?type={content_type}&limit=10"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let top: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(top["items"].as_array().unwrap().len(), 1);
        assert_eq!(top["items"][0]["playCount"], 1);
    }

    let (_, bytes) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/discovery",
        Body::empty(),
    )
    .await;
    let chart_detail: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        chart_detail["items"][0]["albumImageUrl"],
        format!("http://melodee.test/images/album_{release_public_id}/80")
    );

    let (_, bytes) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/discovery/playlist",
        Body::empty(),
    )
    .await;
    let playlist: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(playlist["trackCount"], 1);
    assert_eq!(
        playlist["tracks"][0]["songApiKey"],
        track_public_id.to_string()
    );
    assert_eq!(
        playlist["tracks"][0]["coverUrl"],
        format!("http://melodee.test/images/album_{release_public_id}/80")
    );

    let oversized_chart_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_charts
           (id,public_api_id,slug,title,is_generated_playlist_enabled,created_at,updated_at)
         VALUES($1,$2,'oversized','Oversized Chart',true,now(),now())",
    )
    .bind(oversized_chart_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("oversized chart");
    sqlx::query(
        "INSERT INTO native_chart_items
           (chart_id,rank,artist_name,release_title,linked_artist_id,linked_release_id,created_at)
         SELECT $1,rank,'Discovery Artist','Discovery Release',$2,$3,now()
         FROM generate_series(1,5001) rank",
    )
    .bind(oversized_chart_id)
    .bind(artist_id)
    .bind(fixture.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("oversized chart rows");
    let (status, oversized) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/oversized/playlist",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(oversized.len() < 1_024, "bounded chart error envelope");

    for content_type in ["song", "album", "artist"] {
        let (status, bytes) = request(
            state.clone(),
            Some(principal),
            "GET",
            &format!(
                "/api/v1/recommendations?limit=10&type={content_type}&category=based_on_recent"
            ),
            Body::empty(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{content_type}: {}",
            String::from_utf8_lossy(&bytes)
        );
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["category"], "based_on_recent");
        assert_eq!(
            body["recommendations"][0]["type"], content_type,
            "{content_type}: {body}"
        );
        assert_eq!(
            body["recommendations"][0]["reason"],
            "Based on your recent listening"
        );
    }

    let (status, _) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/playback-backend/initialize",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE native_playback_backend_state SET enabled=false WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("disable backend");
    for (method, path) in [
        ("GET", "/api/v1/playback-backend/status"),
        ("GET", "/api/v1/playback-backend/capabilities"),
        ("POST", "/api/v1/playback-backend/initialize"),
        ("POST", "/api/v1/playback-backend/register-endpoint"),
    ] {
        let (status, bytes) =
            request(state.clone(), Some(principal), method, path, Body::empty()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["message"], "Jukebox is not enabled");
    }
    let (status, bytes) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/playback-backend/shutdown",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"true");
    sqlx::query(
        "UPDATE native_playback_backend_state SET enabled=true,initialized=true WHERE singleton",
    )
    .execute(&db.pool)
    .await
    .expect("restore backend for heartbeat");
    let (_, first_endpoint) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/playback-backend/register-endpoint",
        Body::empty(),
    )
    .await;
    let (_, second_endpoint) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/playback-backend/register-endpoint",
        Body::empty(),
    )
    .await;
    let first_endpoint: serde_json::Value = serde_json::from_slice(&first_endpoint).unwrap();
    let second_endpoint: serde_json::Value = serde_json::from_slice(&second_endpoint).unwrap();
    assert_eq!(first_endpoint["apiKey"], second_endpoint["apiKey"]);
    let (status, _) = request(
        state.clone(),
        Some(principal),
        "POST",
        &format!("/api/v1/playback-backend/heartbeat/{session_id}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/themes/me",
        Body::from(r#"{"themeId":"fixture"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, bytes) = request(
        state.clone(),
        None,
        "GET",
        "/api/v1/themes/fixture/file/theme.css",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"body{}");

    let boundary = "phase7-theme-boundary";
    let archive = theme_zip();
    let mut multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"imported.zip\"\r\nContent-Type: application/zip\r\n\r\n"
    )
    .into_bytes();
    multipart.extend_from_slice(&archive);
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let (status, bytes) = request_with_content_type(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/themes/import",
        &format!("multipart/form-data; boundary={boundary}"),
        Body::from(multipart),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (status, exported) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/themes/imported/export",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut exported = ZipArchive::new(std::io::Cursor::new(exported)).expect("exported ZIP");
    assert!(exported.by_name("theme.json").is_ok());
    assert!(exported.by_name("theme.css").is_ok());
    let (status, _) = request(
        state.clone(),
        Some(principal),
        "POST",
        "/api/v1/themes/rescan",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(
        state.clone(),
        Some(principal),
        "DELETE",
        "/api/v1/themes/imported",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(
        state.clone(),
        None,
        "GET",
        "/api/v1/themes/imported/file/theme.css",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM native_track_audio_features WHERE track_id=$1")
        .bind(track_id)
        .execute(&db.pool)
        .await
        .expect("remove audio features");
    let (status, _) = request(
        state.clone(),
        Some(principal),
        "GET",
        &format!("/api/v1/audio/features/{track_public_id}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let configured = melodee_server::native::configured_router(state.clone());
    let mut bearer_theme_request = Request::builder()
        .method("POST")
        .uri("/api/v1/themes/me")
        .header("authorization", "Bearer deliberately-not-a-cookie")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"themeId":"fixture"}"#))
        .expect("cookie boundary request");
    bearer_theme_request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
        ));
    let cookie_response = configured
        .oneshot(bearer_theme_request)
        .await
        .expect("cookie challenge");
    assert_eq!(cookie_response.status(), StatusCode::UNAUTHORIZED);
    assert!(cookie_response.headers().get("location").is_some());
    let (status, _) = request(
        state.clone(),
        None,
        "GET",
        "/api/v1/system/throw",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let projected_popularity: i64 = sqlx::query_scalar(
        "SELECT popularity FROM native_recommendation_candidates
         WHERE entity_type='track' AND entity_id=$1",
    )
    .bind(track_id)
    .fetch_one(&db.pool)
    .await
    .expect("materialized recommendation popularity");
    assert_eq!(projected_popularity, 1);
    sqlx::query(
        "INSERT INTO native_track_audio_features(track_id,tempo,musical_key,mode,time_signature,updated_at)
         VALUES($1,128,'C','major',4,now())",
    )
    .bind(track_id)
    .execute(&db.pool)
    .await
    .expect("restore feature for ACL regression");

    // Revoking library access must also revoke derived audio and historical
    // discovery facts. Durable plays remain authoritative but become
    // invisible to this principal immediately.
    let restricted_library_id = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET source_library_id=$1 WHERE id=$2")
        .bind(restricted_library_id)
        .bind(fixture.work_item_id())
        .execute(&db.pool)
        .await
        .expect("restricted discovery source library");
    let restricted_group_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_library_access_groups
           (id,public_api_id,name,created_at,updated_at)
         VALUES($1,$2,'Restricted discovery',now(),now())",
    )
    .bind(restricted_group_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("restricted discovery group");
    sqlx::query(
        "INSERT INTO native_library_group_libraries(group_id,library_id,imported_at)
         VALUES($1,$2,now())",
    )
    .bind(restricted_group_id)
    .bind(restricted_library_id)
    .execute(&db.pool)
    .await
    .expect("restricted discovery binding");

    let visible_fixture = phase5_common::seed_cataloged_release(
        &db,
        7502,
        CatalogFixturePresentation {
            release_title: "Visible Chart Release",
            release_artist: "Visible Chart Artist",
            track_title: "Visible Chart Track",
            track_artist: "Visible Chart Artist",
            release_date: "2026-08-13",
        },
    )
    .await;
    let visible_artist_id: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 AND role='primary'
         ORDER BY presentation_order LIMIT 1",
    )
    .bind(visible_fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("visible chart Artist");
    let visible_track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
            .bind(visible_fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("visible chart Track public ID");
    let visible_release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
            .bind(visible_fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("visible recommendation Release public ID");
    sqlx::query(
        "INSERT INTO native_chart_items(chart_id,rank,artist_name,release_title,
         linked_artist_id,linked_release_id,created_at)
         VALUES($1,2,'Visible Chart Artist','Visible Chart Release',$2,$3,now())",
    )
    .bind(chart_id)
    .bind(visible_artist_id)
    .bind(visible_fixture.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("visible chart item");

    let ineligible_release_ids = (0..2_001).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let ineligible_public_ids = (0..2_001).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO releases(
           id,public_api_id,title,normalized_title,sort_title,status,created_at,updated_at)
         SELECT id,public_id,'Played recommendation '||ordinal,
                'PLAYED RECOMMENDATION '||ordinal,'Played recommendation '||ordinal,
                'active',now(),now()
         FROM unnest($1::uuid[],$2::uuid[]) WITH ORDINALITY input(id,public_id,ordinal)",
    )
    .bind(&ineligible_release_ids)
    .bind(&ineligible_public_ids)
    .execute(&db.pool)
    .await
    .expect("ineligible recommendation prefix");
    sqlx::query(
        "INSERT INTO native_release_user_states(
           user_id,release_id,play_count,created_at,updated_at)
         SELECT $1,release_id,1,now(),now() FROM unnest($2::uuid[]) release_id",
    )
    .bind(principal.user_id)
    .bind(&ineligible_release_ids)
    .execute(&db.pool)
    .await
    .expect("category-ineligible recommendation prefix");
    sqlx::query(
        "UPDATE native_recommendation_candidates SET popularity=100
         WHERE entity_type='release' AND entity_id=ANY($1)",
    )
    .bind(&ineligible_release_ids)
    .execute(&db.pool)
    .await
    .expect("rank ineligible recommendation prefix first");
    let recommendation_plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON, COSTS OFF)
         WITH candidate AS MATERIALIZED (
           SELECT entity_id,popularity FROM native_recommendation_candidates
           WHERE entity_type='release'
           ORDER BY popularity DESC,entity_id LIMIT 4096)
         SELECT count(*) FROM candidate",
    )
    .fetch_one(&db.pool)
    .await
    .expect("recommendation rank plan");
    assert!(
        recommendation_plan
            .to_string()
            .contains("native_recommendation_candidates_rank_idx"),
        "recommendation ranking must remain index-backed: {recommendation_plan}"
    );

    let (status, _) = request(
        state.clone(),
        Some(principal),
        "GET",
        &format!("/api/v1/audio/features/{track_public_id}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, bpm) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/audio/bpm?min=120&max=130&page=1&limit=10",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bpm: serde_json::Value = serde_json::from_slice(&bpm).expect("restricted BPM response");
    assert_eq!(bpm["meta"]["totalCount"], 0);
    assert_eq!(bpm["tracks"].as_array().map(Vec::len), Some(0));
    let (_, listening) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/analytics/listening?period=week",
        Body::empty(),
    )
    .await;
    let listening: serde_json::Value =
        serde_json::from_slice(&listening).expect("restricted analytics response");
    assert_eq!(listening["totalTracksPlayed"], 0);
    assert_eq!(listening["topArtists"].as_array().map(Vec::len), Some(0));
    let (_, top) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/analytics/top/week?type=song&limit=10",
        Body::empty(),
    )
    .await;
    let top: serde_json::Value = serde_json::from_slice(&top).expect("restricted top response");
    assert_eq!(top["items"].as_array().map(Vec::len), Some(0));

    let (status, detail) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/discovery",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: serde_json::Value = serde_json::from_slice(&detail).expect("ACL chart detail");
    assert_eq!(detail["items"][0]["linkStatus"], "Unlinked");
    assert!(detail["items"][0]["linkedArtistApiKey"].is_null());
    assert!(detail["items"][0]["linkedAlbumApiKey"].is_null());
    assert_eq!(detail["items"][1]["linkStatus"], "Linked");

    let (status, playlist) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/discovery/playlist",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let playlist: serde_json::Value =
        serde_json::from_slice(&playlist).expect("ACL chart playlist");
    assert_eq!(playlist["trackCount"], 1);
    assert_eq!(
        playlist["tracks"][0]["songApiKey"],
        visible_track_public_id.to_string()
    );

    let bounded_started = std::time::Instant::now();
    let (status, recommendations) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/recommendations?limit=1&type=album&category=discover",
        Body::empty(),
    )
    .await;
    assert!(
        bounded_started.elapsed() < std::time::Duration::from_secs(2),
        "a 2,001-row category-ineligible prefix must remain within the route budget"
    );
    assert_eq!(status, StatusCode::OK);
    let recommendations: serde_json::Value =
        serde_json::from_slice(&recommendations).expect("scoped recommendations");
    assert_eq!(
        recommendations["recommendations"][0]["id"],
        visible_release_public_id.to_string(),
        "eligibility is applied before the response bound, so an inaccessible/inactive prefix cannot starve visible results"
    );

    let overflow_release_ids = (0..2_096).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let overflow_public_ids = (0..2_096).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO releases(
           id,public_api_id,title,normalized_title,sort_title,status,created_at,updated_at)
         SELECT id,public_id,'Bounded recommendation '||ordinal,
                'BOUNDED RECOMMENDATION '||ordinal,'Bounded recommendation '||ordinal,
                'active',now(),now()
         FROM unnest($1::uuid[],$2::uuid[]) WITH ORDINALITY input(id,public_id,ordinal)",
    )
    .bind(&overflow_release_ids)
    .bind(&overflow_public_ids)
    .execute(&db.pool)
    .await
    .expect("overflow recommendation prefix");
    sqlx::query(
        "INSERT INTO native_release_user_states(
           user_id,release_id,play_count,created_at,updated_at)
         SELECT $1,release_id,1,now(),now() FROM unnest($2::uuid[]) release_id",
    )
    .bind(principal.user_id)
    .bind(&overflow_release_ids)
    .execute(&db.pool)
    .await
    .expect("overflow category-ineligible prefix");
    sqlx::query(
        "UPDATE native_recommendation_candidates SET popularity=100
         WHERE entity_type='release' AND entity_id=ANY($1)",
    )
    .bind(&overflow_release_ids)
    .execute(&db.pool)
    .await
    .expect("rank overflow recommendation prefix first");
    let continuation_plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON, COSTS OFF)
         SELECT EXISTS(
           SELECT candidate.entity_id FROM native_recommendation_candidates candidate
           WHERE candidate.entity_type='release'
           ORDER BY candidate.popularity DESC,candidate.entity_id
           OFFSET 4096 LIMIT 1)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("recommendation continuation plan");
    assert!(
        continuation_plan
            .to_string()
            .contains("native_recommendation_candidates_rank_idx"),
        "recommendation continuation probe must remain index-backed: {continuation_plan}"
    );
    let visible_track_id: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE public_api_id=$1")
        .bind(visible_track_public_id)
        .fetch_one(&db.pool)
        .await
        .expect("visible Track internal ID");
    let mut plan_tx = db
        .pool
        .begin()
        .await
        .expect("recommendation plan transaction");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *plan_tx)
        .await
        .expect("prefer indexes for deterministic plan assertion");
    let recent_probe_plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON, COSTS OFF)
         SELECT 1 FROM native_track_plays play
         WHERE play.user_id=$1 AND play.track_id=$2
           AND play.played_at>=now()-interval '30 days' LIMIT 1",
    )
    .bind(principal.user_id)
    .bind(visible_track_id)
    .fetch_one(&mut *plan_tx)
    .await
    .expect("recent recommendation probe plan");
    plan_tx
        .rollback()
        .await
        .expect("release recommendation plan transaction");
    assert!(
        recent_probe_plan
            .to_string()
            .contains("native_track_plays_user_track_time_idx"),
        "recent/similar probes must use the principal-and-Track index: {recent_probe_plan}"
    );
    let exhausted_started = std::time::Instant::now();
    let (status, exhausted) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/recommendations?limit=1&type=album&category=discover",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        exhausted_started.elapsed() < std::time::Duration::from_secs(2),
        "recommendation exhaustion must fail within the hard candidate budget"
    );
    let exhausted: serde_json::Value =
        serde_json::from_slice(&exhausted).expect("bounded exhaustion error");
    assert_eq!(exhausted["code"], "SERVICE_UNAVAILABLE");

    sqlx::query(
        "UPDATE native_recommendation_candidates SET popularity=101
         WHERE entity_type='release' AND entity_id=$1",
    )
    .bind(visible_fixture.release_id().into_uuid())
    .execute(&db.pool)
    .await
    .expect("rank one eligible Release inside the bounded window");

    let (status, first_family_complete) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/recommendations?limit=1&category=discover",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first_family_complete: serde_json::Value = serde_json::from_slice(&first_family_complete)
        .expect("first-family complete recommendations");
    assert_eq!(first_family_complete["recommendations"][0]["type"], "song");
    assert_eq!(
        first_family_complete["recommendations"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "a complete higher-precedence family must skip an exhausted lower family"
    );
    let (status, aggregate_complete) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/recommendations?limit=2&category=discover",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let aggregate_complete: serde_json::Value =
        serde_json::from_slice(&aggregate_complete).expect("aggregate complete recommendations");
    assert_eq!(aggregate_complete["recommendations"][0]["type"], "song");
    assert_eq!(aggregate_complete["recommendations"][1]["type"], "album");
    assert_eq!(
        aggregate_complete["recommendations"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "the lower family is complete once it fills only the aggregate remainder"
    );

    sqlx::query("UPDATE releases SET status='deprecated' WHERE id=$1")
        .bind(visible_fixture.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("retire visible chart Release");
    let (status, retired_detail) = request(
        state.clone(),
        Some(principal),
        "GET",
        "/api/v1/charts/discovery",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retired_detail: serde_json::Value =
        serde_json::from_slice(&retired_detail).expect("retired chart detail");
    assert_eq!(retired_detail["items"][1]["linkStatus"], "Unlinked");
    assert!(retired_detail["items"][1]["linkedAlbumApiKey"].is_null());
    let (status, retired_playlist) = request(
        state,
        Some(principal),
        "GET",
        "/api/v1/charts/discovery/playlist",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retired_playlist: serde_json::Value =
        serde_json::from_slice(&retired_playlist).expect("retired chart playlist");
    assert_eq!(retired_playlist["trackCount"], 0);
    assert_eq!(retired_playlist["tracks"].as_array().map(Vec::len), Some(0));

    db.drop().await;
}
