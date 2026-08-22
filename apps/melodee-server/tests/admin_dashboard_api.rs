//! dashboard aggregation API integration tests: the
//! administrator gate, KPI/attention counts over durable state, the
//! aggregated subsystem health block, and the 24h activity series.

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
use common::{TestDb, test_config};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_domain::PolicyRuleSetId;
use melodee_persistence::{AuthStore, NativeUserImport, PolicyStore, WorkItemStore, WorkerId};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_system::AdminSystemState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-dashboard-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp14-admin", true, admin_public),
        ("wp14-plain", false, plain_public),
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

fn build_app(pool: sqlx::PgPool) -> axum::Router {
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
    melodee_server::admin_system::router().with_state(AdminSystemState::new(admin, None))
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

/// Seeds one cataloged Release and commits a pending Review decision through
/// the legal policy path (same shape as the fixtures).
async fn seed_pending_review(db: &TestDb, ordinal: u32) -> phase5_common::CatalogedPolicyFixture {
    let fixture = phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: "Dashboard Fixture",
            release_artist: "Dashboard Artist",
            track_title: "Dashboard Track",
            track_artist: "Dashboard Artist",
            release_date: "2005-06-07",
        },
    )
    .await;
    let store = PolicyStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim dashboard fixture");
    let claim = claims.pop().expect("one claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load context");
    let rule_set = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist rules");
    store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("commit pending review decision");
    fixture
}

#[tokio::test]
async fn dashboard_requires_administrator() {
    let db = TestDb::create("wp14_dashboard_gate").await;
    let fixture = seed_users(&db).await;
    let app = build_app(db.pool.clone());

    let (status, _) = call(app.clone(), "GET", "/api/admin/v1/dashboard", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let plain = token_for(fixture.plain_public, "wp14-plain");
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/dashboard",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = token_for(fixture.admin_public, "wp14-admin");
    let (status, body) = call(app, "GET", "/api/admin/v1/dashboard", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    db.drop().await;
}

#[tokio::test]
async fn dashboard_aggregates_counts_attention_health_and_activity() {
    let db = TestDb::create("wp14_dashboard").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp14-admin");
    let app = build_app(db.pool.clone());

    // Empty database: zeroed KPIs, honest health, empty activity.
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/dashboard",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["catalog"]["releases"], 0);
    assert_eq!(body["catalog"]["artists"], 0);
    assert_eq!(body["workItems"]["failedLast24h"], 0);
    assert_eq!(body["workItems"]["completedLast24h"], 0);
    assert_eq!(body["attention"]["unresolvedIdentities"], 0);
    assert_eq!(body["attention"]["pendingPolicyDecisions"], 0);
    assert_eq!(body["attention"]["failedProcessing"], 0);
    assert_eq!(body["attention"]["quarantined"], 0);
    assert_eq!(body["health"]["postgresql"]["status"], "ok");
    assert_eq!(body["health"]["tantivy"]["status"], "disabled");
    assert!(
        body["health"]["apiSurfaces"]
            .as_array()
            .expect("api surfaces")
            .iter()
            .any(|surface| surface["surface"] == "admin" && surface["status"] == "serving")
    );
    assert!(
        body["activity"]["completedPerHour"]
            .as_array()
            .expect("activity")
            .is_empty()
    );

    // Seed: one cataloged Release with a pending Review decision plus one
    // failed work item.
    let review = seed_pending_review(&db, 1).await;
    let failed_id =
        common::seed_ready_item(&WorkItemStore, &db.pool, "wp14/dashboard/failed").await;
    let worker = WorkerId::new();
    let config = test_config();
    let claimed = WorkItemStore
        .claim(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim failure fixture");
    assert!(claimed.iter().any(|item| item.id == failed_id));
    WorkItemStore
        .transition(
            &db.pool,
            failed_id,
            Some(&worker),
            melodee_domain::TransitionKind::Worker,
            melodee_domain::IngestionState::Inspecting,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("start inspection");
    WorkItemStore
        .record_failure(
            &db.pool,
            failed_id,
            &worker,
            melodee_domain::WorkErrorCode::new(melodee_domain::WorkErrorCode::STAGE_FAILED),
            "dashboard fixture failure",
            melodee_domain::FailureClass::Permanent,
            &config.retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("record failure");
    // One item completed inside the window (test-only backdate; the schema
    // requires a terminal state with completed_at).
    sqlx::query(
        "UPDATE release_work_items SET state='indexed', completed_at=now(), updated_at=now() WHERE id=$1",
    )
    .bind(review.work_item_id())
    .execute(&db.pool)
    .await
    .expect("backdate completion");

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/dashboard",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["catalog"]["releases"], 1);
    assert_eq!(body["catalog"]["tracks"], 1);
    assert!(body["catalog"]["artists"].as_i64().expect("artists") >= 1);
    assert_eq!(body["workItems"]["failedLast24h"], 1);
    assert_eq!(body["workItems"]["completedLast24h"], 1);
    assert_eq!(body["attention"]["pendingPolicyDecisions"], 1);
    assert_eq!(body["attention"]["failedProcessing"], 1);
    let counts = body["workItems"]["countsByState"]
        .as_array()
        .expect("counts by state");
    assert!(
        counts
            .iter()
            .any(|count| count["state"] == "failed" && count["count"] == 1)
    );
    let activity = body["activity"]["completedPerHour"]
        .as_array()
        .expect("activity");
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0]["count"], 1);

    db.drop().await;
}

#[tokio::test]
async fn system_health_reports_aggregated_subsystems() {
    let db = TestDb::create("wp14_system_health").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp14-admin");
    let app = build_app(db.pool.clone());

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/system/health",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["postgresql"]["status"], "ok");
    assert_eq!(body["tantivy"]["status"], "disabled");
    assert!(body["storage"]["status"].is_string());
    assert!(body["ffmpeg"]["status"].is_string());
    let surfaces = body["apiSurfaces"].as_array().expect("api surfaces");
    assert_eq!(surfaces.len(), 4);
    assert!(
        surfaces
            .iter()
            .all(|surface| surface["status"] == "serving")
    );

    db.drop().await;
}
