//! Release-profile, in-process API measurements against the disposable fixture.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

// The shared fixture helpers need only a pool. This adapter borrows the
// invocation-owned benchmark DB and never owns or removes it.
mod common {
    use melodee_domain::{IngestionState, ReleaseWorkItem};
    use melodee_persistence::{WorkEngineConfig, WorkItemStore};
    use sqlx::PgPool;
    use time::OffsetDateTime;
    use uuid::Uuid;

    pub const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;
    pub struct TestDb {
        pub pool: PgPool,
    }
    pub fn test_config() -> WorkEngineConfig {
        WorkEngineConfig::default()
    }
    pub async fn seed_ready_item(store: &WorkItemStore, pool: &PgPool, path: &str) -> Uuid {
        let item = ReleaseWorkItem::discovered(path.to_string(), None, Uuid::now_v7(), T0);
        let id = item.id;
        store.create(pool, &item).await.unwrap();
        store
            .system_transition(pool, id, IngestionState::Settling, Some("test"), T0)
            .await
            .unwrap();
        store
            .system_transition(pool, id, IngestionState::Ready, Some("test"), T0)
            .await
            .unwrap();
        id
    }
}

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use melodee_auth::{NativeAccessClaims, SecretString, issue_access_token};
use melodee_server::{
    admin_access::AdminAccessState,
    admin_catalog::AdminCatalogState,
    admin_system::AdminSystemState,
    native::{
        NativeClientBinding, NativePrincipal, NativeState, PostgresNativeUserStateReader,
        stream::DatabaseStreamUrlSigner,
    },
};
use std::{net::SocketAddr, sync::Arc, time::Instant};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "performance-fixture-jwt-key-at-least-32-bytes";
const PEPPER: &[u8] = b"fixture-jellyfin-pepper-at-least-32-bytes";

fn admin_access(pool: sqlx::PgPool) -> AdminAccessState {
    AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool: pool.clone(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(KEY.into(), 90).unwrap(),
        ),
    }
}

fn admin_token(id: Uuid) -> String {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    issue_access_token(
        &SecretString::new(KEY.into()),
        &NativeAccessClaims {
            aud: "MelodeeClient".into(),
            email: "benchmark-1@example.test".into(),
            exp: now + 3600,
            sid: id,
            iat: now,
            iss: "MelodeeApi".into(),
            nbf: now,
            role: vec![],
            unique_name: "benchmark-1".into(),
            theme: String::new(),
        },
    )
    .unwrap()
}

async fn sample(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    principal: Option<NativePrincipal>,
    label: &str,
    counts: (i64, i64, i64),
) {
    for iteration in 0..4 {
        let mut builder = Request::builder().uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let mut request = builder.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "203.0.113.7:9000".parse::<SocketAddr>().unwrap(),
        ));
        if let Some(principal) = principal {
            request.extensions_mut().insert(principal);
            request
                .extensions_mut()
                .insert(NativeClientBinding("203.0.113.7".into()));
        }
        let start = Instant::now();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{label}: {value}");
        match label {
            "native_artists" | "native_releases" | "native_tracks" => {
                assert!(
                    value["data"]
                        .as_array()
                        .is_some_and(|rows| !rows.is_empty()),
                    "{label}: {value}"
                );
            }
            "native_suggest" => assert!(value["artists"].as_array().is_some()),
            "admin_stats" => {
                assert_eq!(value["library"]["artists"], counts.0);
                assert_eq!(value["library"]["releases"], counts.1);
                assert_eq!(value["library"]["tracks"], counts.2);
            }
            "admin_artist_search" => assert!(
                value["items"]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty())
            ),
            _ => panic!("unknown benchmark route"),
        }
        println!(
            "route={label} iteration={iteration} elapsed_ms={elapsed:.3} bytes={}",
            bytes.len()
        );
    }
}

#[tokio::test]
#[ignore = "requires the invocation-owned representative fixture and a release build"]
async fn measure_interactive_routes() {
    let url = std::env::var("MELODEE__DATABASE__URL").unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database
            .strip_prefix("melodee_template_")
            .is_some_and(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()))
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").unwrap(),
        database
    );
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)")
        .fetch_one(&pool).await.unwrap();
    let (user_id, user_public): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id,public_api_id FROM native_users WHERE normalized_username = 'BENCHMARK-2'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let admin_public: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM native_users WHERE normalized_username = 'BENCHMARK-1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let signer = Arc::new(DatabaseStreamUrlSigner::new(
        pool.clone(),
        "http://benchmark.invalid",
    ));
    let state = NativeState {
        lastfm_api_base_url: None,
        pool: pool.clone(),
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        search: None,
        public_origin: "http://benchmark.invalid".into(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .unwrap(),
        stream_url_signer: signer.clone(),
        user_state: Arc::new(PostgresNativeUserStateReader::new(pool.clone())),
        direct_play: None,
        client_addresses: Default::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(KEY.into(), 90).unwrap(),
        ),
    };
    let native = melodee_server::native::router().with_state(state);
    let principal = NativePrincipal {
        user_id,
        public_api_id: user_public,
        is_admin: false,
        is_editor: false,
    };
    for (label, path) in [
        ("native_artists", "/api/v1/artists?page=1&pageSize=25"),
        ("native_releases", "/api/v1/albums?page=1&pageSize=25"),
        ("native_tracks", "/api/v1/songs?page=1&pageSize=25"),
        ("native_suggest", "/api/v1/search/suggest?q=Artist&limit=10"),
    ] {
        sample(&native, path, None, Some(principal), label, counts).await;
    }

    let (rated_artist, expected_rating): (Uuid, i16) = sqlx::query_as(
        "SELECT a.public_api_id, s.rating FROM native_artist_user_states s
         JOIN artists a ON a.id = s.artist_id JOIN release_artists r ON r.artist_id = a.id
         WHERE s.user_id = $1 AND s.rating > 0 LIMIT 1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut request = Request::builder()
        .uri(format!("/api/v1/artists/{rated_artist}"))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("203.0.113.7".into()));
    let response = native.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let artist: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(artist["userRating"], expected_rating);
    println!("rating_check=passed");

    let token = admin_token(admin_public);
    let stats = melodee_server::admin_stats::router()
        .with_state(AdminSystemState::new(admin_access(pool.clone()), None));
    sample(
        &stats,
        "/api/admin/v1/stats",
        Some(&token),
        None,
        "admin_stats",
        counts,
    )
    .await;
    let catalog = melodee_server::admin_catalog::router().with_state(AdminCatalogState::new(
        admin_access(pool.clone()),
        signer,
        Default::default(),
    ));
    sample(
        &catalog,
        "/api/admin/v1/catalog/artists?query=Artist&limit=25",
        Some(&token),
        None,
        "admin_artist_search",
        counts,
    )
    .await;

    let jf = melodee_server::jellyfin::JfState {
        lastfm_api_base_url: None,
        pool: pool.clone(),
        server_name: "Melodee".into(),
        server_version: "performance-fixture".into(),
        server_id: "93cd3cc3a120127ccfda124620290cae".into(),
        local_address: "http://benchmark.invalid".into(),
        public_origin: "http://benchmark.invalid".into(),
        token_pepper: melodee_server::jellyfin::SecretPepper::new(PEPPER.to_vec()).unwrap(),
        token_expiry_hours: 1,
        max_active_tokens: 10,
        direct_play: None,
        transcoding: None,
        client_addresses: Default::default(),
        search: None,
        libraries: vec![],
    };
    let jf = melodee_server::jellyfin::configured_router(jf);
    let request = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"Username":"benchmark-1","Pw":"benchmark-only-password"}"#,
        ))
        .unwrap();
    let response = jf.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let auth: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let jf_token = auth["AccessToken"].as_str().unwrap();
    for path in [
        "/Artists?limit=25",
        "/Items?includeItemTypes=MusicAlbum&limit=25",
        "/Items?includeItemTypes=Audio&limit=25",
    ] {
        for iteration in 0..4 {
            let req = Request::builder()
                .uri(path)
                .header("X-Emby-Token", jf_token)
                .body(Body::empty())
                .unwrap();
            let start = Instant::now();
            let response = jf.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                value["Items"]
                    .as_array()
                    .is_some_and(|rows| !rows.is_empty()),
                "{path}: {value}"
            );
            println!(
                "route=jellyfin path={path} iteration={iteration} elapsed_ms={:.3} bytes={}",
                start.elapsed().as_secs_f64() * 1000.0,
                bytes.len()
            );
        }
    }

    // Use the same real manifest/publication/catalog path as integration
    // tests; the fixture harness supplies an invocation-owned asset root.
    assert!(std::env::var_os("MELODEE__STORAGE__ASSET_ROOT").is_some());
    let db = common::TestDb { pool: pool.clone() };
    let presentation = |title| catalog_common::CatalogFixturePresentation {
        release_title: title,
        release_artist: "ACL Fixture Artist",
        track_title: "ACL Fixture Track",
        track_artist: "ACL Fixture Artist",
        release_date: "2025-01-01",
    };
    let allowed =
        phase5_common::seed_cataloged_release(&db, 9801, presentation("ACL Allowed Release")).await;
    let hidden =
        phase5_common::seed_cataloged_release(&db, 9802, presentation("ACL Hidden Release")).await;
    let mut allowed_group = Uuid::nil();
    let mut release_ids = Vec::new();
    for (name, fixture) in [("allowed", &allowed), ("hidden", &hidden)] {
        let group = Uuid::now_v7();
        let library = Uuid::now_v7();
        if name == "allowed" {
            allowed_group = group;
        }
        sqlx::query("UPDATE release_work_items SET source_library_id=$1 WHERE id=$2")
            .bind(library)
            .bind(fixture.work_item_id())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO native_library_access_groups(id,public_api_id,name,created_at,updated_at) VALUES ($1,$2,$3,now(),now())")
            .bind(group).bind(Uuid::now_v7()).bind(name).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO native_library_group_libraries(group_id,library_id,imported_at) VALUES ($1,$2,now())")
            .bind(group).bind(library).execute(&pool).await.unwrap();
        let public_id: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
        release_ids.push(public_id);
    }
    sqlx::query(
        "INSERT INTO native_user_library_groups(user_id,group_id,created_at) VALUES ($1,$2,now())",
    )
    .bind(user_id)
    .bind(allowed_group)
    .execute(&pool)
    .await
    .unwrap();
    for (label, id, expected) in [
        ("allowed", release_ids[0], StatusCode::OK),
        ("denied", release_ids[1], StatusCode::NOT_FOUND),
    ] {
        for iteration in 0..4 {
            let mut request = Request::builder()
                .uri(format!("/api/v1/albums/{id}"))
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(principal);
            request
                .extensions_mut()
                .insert(NativeClientBinding("203.0.113.7".into()));
            let start = Instant::now();
            let response = native.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            if label == "allowed" {
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(value["name"], "ACL Allowed Release");
            }
            println!(
                "route=native_acl_{label} iteration={iteration} elapsed_ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    sqlx::query("DELETE FROM native_user_library_groups WHERE user_id=$1 AND group_id=$2")
        .bind(user_id)
        .bind(allowed_group)
        .execute(&pool)
        .await
        .unwrap();
    let mut request = Request::builder()
        .uri(format!("/api/v1/albums/{}", release_ids[0]))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("203.0.113.7".into()));
    assert_eq!(
        native.oneshot(request).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    println!("acl_revocation_check=passed");
}
