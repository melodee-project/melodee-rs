//! Phase 9 WP-08 Admin API integration tests: onboarding status, fail-closed
//! completion, and the migration-bypass heuristic (PRD-SETUP-001/002,
//! ADR-0134 §3).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport, SettingsStore};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_onboarding::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-onboarding-jwt-32-bytes!!";

fn app(pool: sqlx::PgPool) -> axum::Router {
    router().with_state(AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: std::sync::Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool,
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
    })
}

async fn seed_admin(db: &common::TestDb, username: &str, is_admin: bool) -> String {
    let now = OffsetDateTime::now_utc();
    let public_id = Uuid::now_v7();
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
    let issued = now.unix_timestamp();
    issue_access_token(
        &SecretString::new(JWT_KEY.to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email,
            exp: issued + 900,
            sid: public_id,
            iat: issued,
            iss: "MelodeeApi".to_string(),
            nbf: issued,
            role: vec![],
            unique_name: username.to_string(),
            theme: String::new(),
        },
    )
    .expect("issue token")
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("request"))
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
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, json)
}

fn check<'a>(status: &'a Value, name: &str) -> &'a Value {
    status["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("check {name} present"))
}

#[tokio::test]
async fn status_reports_six_named_checks_and_fail_closed_completion() {
    let db = common::TestDb::create("wp08_onboarding_status").await;
    // A non-admin cannot even read status; gate proof is below. Seed an
    // admin only for authentication — the fixture database still has no
    // *completed* onboarding and no other admin until seeded.
    let admin = seed_admin(&db, "wp08-onboard-admin", true).await;

    let (status, body) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/onboarding/status",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|check| check["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "database",
            "storage_paths",
            "media_tools",
            "search_index",
            "admin_user",
            "email"
        ]
    );
    assert_eq!(check(&body, "database")["passed"], true);
    assert_eq!(check(&body, "database")["required"], true);
    assert_eq!(check(&body, "admin_user")["passed"], true);
    assert_eq!(body["completed"], false);
    assert_eq!(body["bypassed"], false, "fresh database is not bypassed");
    assert_eq!(body["canComplete"], true);
    db.drop().await;
}

#[tokio::test]
async fn complete_fails_closed_without_an_admin_then_succeeds() {
    let db = common::TestDb::create("wp08_onboarding_complete").await;
    // Authenticate as a non-admin? No — completion needs the admin gate AND
    // the admin-exists check. Use a chicken-and-egg proof: the seeded admin
    // satisfies both; a separate fresh database proves the failing path via
    // direct evaluation.
    let report = melodee_server::onboarding::evaluate_onboarding(&db.pool).await;
    let admin_check = report
        .checks
        .iter()
        .find(|check| check.name == "admin_user")
        .expect("admin check");
    assert!(!admin_check.passed, "fresh database has no administrator");
    assert!(admin_check.remediation.is_some());
    assert!(!report.can_complete);

    // Direct completion fails closed while the required check fails.
    let outcome = melodee_server::onboarding::complete_onboarding(
        &db.pool,
        "0192abcd-0000-7000-8000-000000000001",
    )
    .await;
    assert!(
        matches!(
            outcome,
            Err(melodee_server::onboarding::OnboardingError::ChecksFailed(_))
        ),
        "completion must fail closed: {outcome:?}"
    );
    assert!(
        !melodee_persistence::OnboardingStore::new()
            .is_completed(&db.pool)
            .await
            .expect("state")
    );

    // Seed the admin (the bootstrap-admin path's effect) and complete via
    // the API.
    let admin = seed_admin(&db, "wp08-completer", true).await;
    let (status, body) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/onboarding/complete",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
    assert_eq!(body["bypassed"], true, "completion marks bypass evidence");
    assert!(body["completedBy"].as_str().is_some());

    // The durable marker carries check evidence (and no secret material).
    let stored = melodee_persistence::OnboardingStore::new()
        .get_state(&db.pool)
        .await
        .expect("state");
    assert!(stored.is_completed());
    let evidence = stored.evidence.expect("evidence");
    assert_eq!(evidence["checks"].as_array().expect("checks").len(), 6);

    // Completion is idempotent.
    let (status, _) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/onboarding/complete",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    db.drop().await;
}

#[tokio::test]
async fn migration_bypass_heuristic_catalog_or_imported_settings() {
    let db = common::TestDb::create("wp08_onboarding_bypass").await;
    let admin = seed_admin(&db, "wp08-bypass-admin", true).await;

    // Fresh database: no bypass.
    let (_, body) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/onboarding/status",
        Some(&admin),
    )
    .await;
    assert_eq!(body["bypassed"], false);

    // Catalog evidence (a migrated library has catalog rows): insert one
    // artist directly.
    sqlx::query(
        "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name, \
         artist_type, status, created_at, updated_at) \
         VALUES ($1, $2, 'Bypass Evidence', 'BYPASS EVIDENCE', 'Bypass Evidence', 'group', \
         'verified', now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("insert artist");
    let (_, body) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/onboarding/status",
        Some(&admin),
    )
    .await;
    assert_eq!(
        body["bypassed"], true,
        "non-empty catalog bypasses onboarding: {body}"
    );
    db.drop().await;

    // Legacy-settings-import evidence: a known registry row bypasses too.
    let db = common::TestDb::create("wp08_onboarding_bypass_settings").await;
    let admin = seed_admin(&db, "wp08-bypass-admin2", true).await;
    SettingsStore::new()
        .set(
            &db.pool,
            "system.siteName",
            &json!("Migrated Instance"),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("set known key");
    let (_, body) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/onboarding/status",
        Some(&admin),
    )
    .await;
    assert_eq!(
        body["bypassed"], true,
        "imported legacy settings bypass onboarding: {body}"
    );
    // Bypassed is not completed: completion remains a distinct marker.
    assert_eq!(body["completed"], false);
    db.drop().await;
}

#[tokio::test]
async fn administrator_gate_rejects_anonymous_and_non_admin() {
    let db = common::TestDb::create("wp08_onboarding_gate").await;
    let admin = seed_admin(&db, "wp08-gate-admin", true).await;
    let plain = seed_admin(&db, "wp08-gate-plain", false).await;
    let _ = admin;
    for (bearer, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(plain.as_str()), StatusCode::FORBIDDEN),
    ] {
        let (status, _) = call(
            app(db.pool.clone()),
            "GET",
            "/api/admin/v1/onboarding/status",
            bearer,
        )
        .await;
        assert_eq!(status, expected, "status");
        let (status, _) = call(
            app(db.pool.clone()),
            "POST",
            "/api/admin/v1/onboarding/complete",
            bearer,
        )
        .await;
        assert_eq!(status, expected, "complete");
    }
    db.drop().await;
}
