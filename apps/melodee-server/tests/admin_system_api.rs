//! Phase 10 WP-17 system/workers/search admin API integration tests: the
//! administrator gate, the worker-pool table with durable queue metrics,
//! lease-coordinated pause/resume (the same control plane the CLI uses), the
//! bounded failures listing, and search status/rebuild coordination.

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
    http::{Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use common::{TestDb, seed_ready_item, test_config};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{AuthStore, NativeUserImport, WorkItemStore, WorkerId, is_pool_paused};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_system::AdminSystemState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-system-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp17-admin", true, admin_public),
        ("wp17-plain", false, plain_public),
    ] {
        let email = format!("{username}@example.test");
        AuthStore::new()
            .import_user(
                &db.pool,
                &NativeUserImport {
                    id: Uuid::now_v7(),
                    public_api_id: public_id,
                    username,
                    normalized_username: &username.to_uppercase(),
                    email: &email,
                    normalized_email: &email.to_uppercase(),
                    password_hash: &bcrypt::hash("Mel0deeR0cks!", 4).expect("fixture hash"),
                    password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                    stream_signing_key: "test-stream-signing-key",
                    is_admin,
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
    }
    Fixture {
        admin_public,
        plain_public,
    }
}

fn token_for(public_api_id: Uuid, username: &str) -> String {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    issue_access_token(
        &SecretString::new(JWT_KEY.to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email: format!("{username}@example.test"),
            exp: now + 900,
            sid: public_api_id,
            iat: now,
            iss: "MelodeeApi".to_string(),
            nbf: now,
            role: vec![],
            unique_name: username.to_string(),
            theme: String::new(),
        },
    )
    .expect("issue token")
}

fn build_app(pool: sqlx::PgPool, search: Option<melodee_search::SearchConfig>) -> axum::Router {
    let admin = AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool,
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
    };
    melodee_server::admin_system::router().with_state(AdminSystemState::new(admin, search))
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize"))
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).expect("request"))
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
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Seeds one work item parked in `failed` through the legal worker path.
async fn seed_failed_item(db: &TestDb, source_path: &str) -> Uuid {
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, source_path).await;
    let worker = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim failure fixture");
    assert!(claimed.iter().any(|item| item.id == id));
    store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("start inspection");
    store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "system fixture failure",
            FailureClass::Permanent,
            &config.retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("record permanent failure");
    id
}

#[tokio::test]
async fn system_routes_require_administrator() {
    let db = TestDb::create("wp17_system_gate").await;
    let fixture = seed_users(&db).await;
    let app = build_app(db.pool.clone(), None);

    for path in [
        "/api/admin/v1/workers/pools",
        "/api/admin/v1/workers/failures",
        "/api/admin/v1/search/status",
        "/api/admin/v1/system/health",
        "/api/admin/v1/dashboard",
    ] {
        let (status, _) = call(app.clone(), "GET", path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        let plain = token_for(fixture.plain_public, "wp17-plain");
        let (status, _) = call(app.clone(), "GET", path, None, Some(&plain)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }

    db.drop().await;
}

#[tokio::test]
async fn worker_pool_table_reports_durable_state_and_queue_metrics() {
    let db = TestDb::create("wp17_pool_table").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp17-admin");
    let app = build_app(db.pool.clone(), None);

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/workers/pools",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("pools");
    assert_eq!(items.len(), 3, "the migration seeds the three wired pools");
    let by_name = |name: &str| {
        items
            .iter()
            .find(|pool| pool["name"] == name)
            .unwrap_or_else(|| panic!("pool {name} must be listed"))
    };
    let discovery = by_name("discovery");
    assert_eq!(discovery["metricsAvailable"], false);
    assert!(discovery["queued"].is_null());
    let policy = by_name("policy");
    assert_eq!(policy["metricsAvailable"], true);
    assert_eq!(policy["queued"], 0);
    assert_eq!(policy["running"], 0);
    assert_eq!(policy["paused"], false);
    assert_eq!(by_name("podcast")["metricsAvailable"], true);

    // A cataloged work item is the policy pool's durable queue.
    let _release = phase5_common::seed_cataloged_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Pool Table Fixture",
            release_artist: "Pool Artist",
            track_title: "Pool Track",
            track_artist: "Pool Artist",
            release_date: "2006-07-08",
        },
    )
    .await;
    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/workers/pools",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("pools");
    let policy = items
        .iter()
        .find(|pool| pool["name"] == "policy")
        .expect("policy pool");
    assert_eq!(policy["queued"], 1);
    assert!(policy["oldestQueuedAgeSeconds"].as_i64().expect("age") >= 0);
    assert_eq!(policy["completedLastHour"], 0);

    db.drop().await;
}

#[tokio::test]
async fn pool_pause_resume_is_lease_coordinated_and_audited() {
    let db = TestDb::create("wp17_pool_pause").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp17-admin");
    let app = build_app(db.pool.clone(), None);

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/workers/pools/policy/pause",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "policy");
    assert_eq!(body["paused"], true);
    assert_eq!(
        body["pausedBy"].as_str().expect("pausedBy"),
        fixture.admin_public.to_string(),
        "the acting administrator is the durable pause audit"
    );
    assert!(
        is_pool_paused(&db.pool, "policy")
            .await
            .expect("pause check"),
        "the durable gate the policy worker checks between batches is set"
    );

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/workers/pools/policy/resume",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["paused"], false);
    assert!(
        !body["resumeRequestedAt"].is_null(),
        "resume requests are stamped"
    );
    assert!(
        !is_pool_paused(&db.pool, "policy")
            .await
            .expect("pause check")
    );

    // Unknown-but-valid names upsert (a pool wired in a later binary is
    // paused the moment it first checks its row) — same semantics as the CLI.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/workers/pools/ingest/pause",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["paused"], true);

    // Invalid pool names are rejected.
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/workers/pools/NotAPool/pause",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_pool_name");

    db.drop().await;
}

#[tokio::test]
async fn worker_failures_list_recent_failed_work_items() {
    let db = TestDb::create("wp17_failures").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp17-admin");
    let failed_id = seed_failed_item(&db, "wp17/failures/failed").await;
    let _ready = seed_ready_item(&WorkItemStore, &db.pool, "wp17/failures/ready").await;
    let app = build_app(db.pool.clone(), None);

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/workers/failures",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1);
    let items = body["items"].as_array().expect("failures");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().expect("id"), failed_id.to_string());
    assert_eq!(items[0]["state"], "failed");
    assert!(items[0]["lastErrorCode"].is_string());

    // Pagination envelope clamps and bad sort keys are rejected.
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/workers/failures?limit=100000",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["limit"], 500);
    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/workers/failures?sort=bogus:desc",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "unknown_sort");

    db.drop().await;
}

#[tokio::test]
async fn search_status_and_rebuild_honor_disabled_and_conflict_states() {
    let db = TestDb::create("wp17_search_admin").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp17-admin");

    // Disabled search: status reports honestly; rebuild conflicts.
    let app = build_app(db.pool.clone(), None);
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/search/status",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert!(body["projection"].is_null());
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/search/rebuild",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "search_disabled");

    // Enabled search against a fresh root: no active generation yet, and the
    // first rebuild request is accepted while a second conflicts (the running
    // search worker owns execution; this surface only records the durable
    // request).
    let search_root = tempfile::tempdir().expect("search root");
    let search = melodee_search::SearchConfig {
        search_root: search_root.path().to_path_buf(),
        enabled: true,
        ..melodee_search::SearchConfig::default()
    };
    let app = build_app(db.pool.clone(), Some(search));
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/search/status",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    assert!(body["projection"].is_object());
    assert!(body["generation"].is_null());
    assert_eq!(body["generationErrorCode"], "search.no_active_generation");

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/search/rebuild",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body["jobId"].is_string());
    assert_eq!(body["status"], "requested");
    assert_eq!(
        body["requestedBy"].as_str().expect("requestedBy"),
        fixture.admin_public.to_string()
    );

    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/search/rebuild",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rebuild_in_progress");

    db.drop().await;
}

#[tokio::test]
async fn cookie_session_authenticates_admin_routes() {
    let db = TestDb::create("admin_system_cookie_auth").await;
    let fixture = seed_users(&db).await;
    let app = build_app(db.pool.clone(), None);

    let admin_cookie = mint_cookie(JWT_KEY, fixture.admin_public, 3600);
    let (status, body) = call_with_cookie(
        app.clone(),
        "GET",
        "/api/admin/v1/system/health",
        None,
        Some(&admin_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");

    let non_admin_cookie = mint_cookie(JWT_KEY, fixture.plain_public, 3600);
    let (status, body) = call_with_cookie(
        app.clone(),
        "GET",
        "/api/admin/v1/system/health",
        None,
        Some(&non_admin_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "forbidden");

    let expired_cookie = mint_cookie(JWT_KEY, fixture.admin_public, -3600);
    let (status, body) = call_with_cookie(
        app.clone(),
        "GET",
        "/api/admin/v1/system/health",
        None,
        Some(&expired_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "unauthenticated");

    let (status, body) = call_with_cookie(
        app,
        "GET",
        "/api/admin/v1/system/health",
        None,
        Some("not-a-valid-cookie-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "unauthenticated");

    db.drop().await;
}

fn mint_cookie(key: &str, user_public_id: Uuid, offset_seconds: i64) -> String {
    let secret = melodee_auth::SecretString::new(key.to_string());
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let (iat, exp) = if offset_seconds < 0 {
        (now + offset_seconds - 3600, now + offset_seconds)
    } else {
        (now - 60, now + offset_seconds)
    };
    melodee_auth::issue_cookie_session(
        &secret,
        &melodee_auth::NativeCookieClaims {
            public_api_id: user_public_id,
            iat,
            exp,
        },
    )
    .expect("mint cookie")
}

async fn call_with_cookie(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie_val) = cookie {
        builder = builder.header(header::COOKIE, format!("melodee_auth={cookie_val}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize"))
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).expect("request"))
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
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}
