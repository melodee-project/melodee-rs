//! Policy Rule API integration tests: the administrator gate,
//! versioned rule CRUD (immutable history, never in-place), safety-rule
//! protection, enable/disable as version membership, and the audited
//! test-against-catalog dry run.

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
use common::TestDb;
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport, PolicyStore};
use melodee_policy::{PolicyConfig, default_rule_set};
use melodee_server::admin_access::AdminAccessState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-policies-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp16-admin", true, admin_public),
        ("wp16-plain", false, plain_public),
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

/// Persists the compiled-in default rule set exactly as the policy worker
/// does at startup (`ensure_rule_set` of `default_rule_set`).
async fn seed_default_rule_set(db: &TestDb) {
    let rule_set = default_rule_set(&PolicyConfig::default()).expect("default rule set");
    PolicyStore
        .ensure_rule_set(&db.pool, &rule_set, OffsetDateTime::now_utc())
        .await
        .expect("persist default rule set");
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
    melodee_server::admin_policies::router().with_state(admin)
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

fn rule_ids(body: &Value) -> Vec<&str> {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|rule| rule["id"].as_str().expect("rule id"))
        .collect()
}

fn rule_by_id<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|rule| rule["id"] == id)
        .unwrap_or_else(|| panic!("rule {id} must be listed"))
}

#[tokio::test]
async fn policy_routes_require_administrator() {
    let db = TestDb::create("wp16_policies_gate").await;
    let fixture = seed_users(&db).await;
    seed_default_rule_set(&db).await;
    let app = build_app(db.pool.clone());

    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/policies/rules",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let plain = token_for(fixture.plain_public, "wp16-plain");
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/policies/rules",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = token_for(fixture.admin_public, "wp16-admin");
    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/policies/rules",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    db.drop().await;
}

#[tokio::test]
async fn rule_listing_reports_active_version_with_priority_and_match_counts() {
    let db = TestDb::create("wp16_policies_list").await;
    let fixture = seed_users(&db).await;
    seed_default_rule_set(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");
    let app = build_app(db.pool.clone());

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/policies/rules",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["ruleSet"]["name"],
        melodee_policy::DEFAULT_RULE_SET_NAME
    );
    assert_eq!(body["ruleSet"]["version"], 2);
    let ids = rule_ids(&body);
    assert_eq!(
        ids,
        [
            "POL-ADMIN-OVERRIDE",
            "POL-CORRUPT",
            "POL-GH-UNCERTAIN",
            "POL-GH-NOVEL",
            "POL-GH-REDUNDANT"
        ],
        "active rules in evaluation (priority) order"
    );
    let first = rule_by_id(&body, "POL-ADMIN-OVERRIDE");
    assert_eq!(first["priority"], 0);
    assert_eq!(first["ruleType"], "administrator_override");
    assert_eq!(first["enabled"], true);
    assert_eq!(first["matchCount"], 0);
    assert_eq!(body["total"], 5);

    db.drop().await;
}

#[tokio::test]
async fn rule_crud_creates_immutable_versions_and_protects_safety_rules() {
    let db = TestDb::create("wp16_policies_crud").await;
    let fixture = seed_users(&db).await;
    seed_default_rule_set(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");
    let app = build_app(db.pool.clone());

    // Create: appended at the lowest priority of a new immutable version.
    let new_rule = serde_json::json!({
        "id": "POL-HOLIDAY",
        "ruleType": "trait_disposition",
        "disposition": "review",
        "predicate": { "required_traits": ["greatest_hits"] }
    });
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules",
        Some(new_rule.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["ruleSet"]["version"], 3);
    assert_eq!(body["rule"]["id"], "POL-HOLIDAY");
    assert_eq!(body["rule"]["priority"], 5);
    assert_eq!(body["changed"], true);

    // Duplicate id conflicts.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules",
        Some(new_rule),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_exists");

    // Update: same position, new version.
    let (status, body) = call(
        app.clone(),
        "PUT",
        "/api/admin/v1/policies/rules/POL-HOLIDAY",
        Some(serde_json::json!({
            "id": "POL-HOLIDAY",
            "ruleType": "trait_disposition",
            "disposition": "ignore",
            "predicate": { "required_traits": ["greatest_hits"] }
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ruleSet"]["version"], 4);
    assert_eq!(body["rule"]["disposition"], "ignore");
    assert_eq!(body["rule"]["priority"], 5);

    // A destructive disposition without a confidence floor violates the
    // domain invariant and never persists.
    let (status, body) = call(
        app.clone(),
        "PUT",
        "/api/admin/v1/policies/rules/POL-HOLIDAY",
        Some(serde_json::json!({
            "id": "POL-HOLIDAY",
            "ruleType": "trait_disposition",
            "disposition": "delete",
            "predicate": { "required_traits": ["greatest_hits"] }
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_rule");

    // Path/body id mismatch is rejected.
    let (status, body) = call(
        app.clone(),
        "PUT",
        "/api/admin/v1/policies/rules/POL-HOLIDAY",
        Some(serde_json::json!({
            "id": "POL-OTHER",
            "ruleType": "trait_disposition",
            "disposition": "ignore",
            "predicate": { "required_traits": ["greatest_hits"] }
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "rule_id_mismatch");

    // Disable: new version without the rule; the listing retains it from
    // immutable history with enabled:false.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/POL-HOLIDAY/disable",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ruleSet"]["version"], 5);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["changed"], true);

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/policies/rules",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ruleSet"]["version"], 5);
    assert_eq!(
        body["total"], 6,
        "disabled rules remain listed from history"
    );
    let holiday = rule_by_id(&body, "POL-HOLIDAY");
    assert_eq!(holiday["enabled"], false);
    assert!(holiday["priority"].is_null());
    assert_eq!(
        holiday["disposition"], "ignore",
        "the latest historical definition"
    );

    // Disabling again is an idempotent no-op.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/POL-HOLIDAY/disable",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], false);

    // Enable restores the historical definition at the lowest priority.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/POL-HOLIDAY/enable",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], true);
    assert_eq!(body["rule"]["enabled"], true);
    assert_eq!(body["rule"]["disposition"], "ignore");
    assert_eq!(body["rule"]["priority"], 5);
    assert_eq!(body["ruleSet"]["version"], 6);

    // Enabling again is an idempotent no-op.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/POL-HOLIDAY/enable",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], false);
    assert_eq!(body["ruleSet"]["version"], 6);

    // Delete removes the rule in a new version; history keeps it listed.
    let (status, body) = call(
        app.clone(),
        "DELETE",
        "/api/admin/v1/policies/rules/POL-HOLIDAY",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], true);
    assert_eq!(body["ruleSet"]["version"], 7);
    let (status, body) = call(
        app.clone(),
        "DELETE",
        "/api/admin/v1/policies/rules/POL-HOLIDAY",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The safety rules can be neither disabled nor deleted.
    for (method, suffix) in [("POST", "/disable"), ("DELETE", "")] {
        for protected in ["POL-ADMIN-OVERRIDE", "POL-CORRUPT"] {
            let (status, body) = call(
                app.clone(),
                method,
                &format!("/api/admin/v1/policies/rules/{protected}{suffix}"),
                None,
                Some(&admin),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "{method} {protected}{suffix}: {body}"
            );
            assert_eq!(body["code"], "protected_rule");
        }
    }

    // Enabling a rule that never existed is a 404.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/POL-NEVER/enable",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // History is immutable: version 2 still holds the original five rules.
    let versions = PolicyStore
        .list_rule_set_versions(&db.pool, melodee_policy::DEFAULT_RULE_SET_NAME)
        .await
        .expect("version history");
    assert_eq!(versions.len(), 6);
    assert_eq!(versions[0].version, 2);
    assert_eq!(versions[0].rules.len(), 5);
    assert_eq!(versions[1].rules.len(), 6);

    db.drop().await;
}

#[tokio::test]
async fn test_run_evaluates_catalog_and_persists_audit_for_stored_rules() {
    let db = TestDb::create("wp16_policies_test").await;
    let fixture = seed_users(&db).await;
    seed_default_rule_set(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");
    let release = phase5_common::seed_cataloged_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Policy Test Greatest Hits",
            release_artist: "Policy Artist",
            track_title: "Policy Track",
            track_artist: "Policy Artist",
            release_date: "2004-05-06",
        },
    )
    .await;
    let app = build_app(db.pool.clone());

    // Test a stored rule: the title classifies this as Greatest Hits and its
    // unconfirmed Track novelty matches the uncertain safety rule.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({ "ruleId": "POL-GH-UNCERTAIN" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["persisted"], true);
    assert!(body["testRunId"].is_string());
    assert_eq!(body["evaluated"], 1);
    assert_eq!(body["matchedCount"], 1);
    assert_eq!(body["matchedRuleCounts"]["POL-GH-UNCERTAIN"], 1);
    assert_eq!(body["proposedDispositionCounts"]["review"], 1);
    assert_eq!(body["destructiveImpact"], 0);
    assert_eq!(body["destructive"], false);
    let sample = body["sample"].as_array().expect("sample");
    assert_eq!(sample.len(), 1);
    assert_eq!(
        sample[0]["releaseId"].as_str().expect("releaseId"),
        release.release_id().into_uuid().to_string()
    );
    assert_eq!(sample[0]["title"], "Policy Test Greatest Hits");
    assert_eq!(sample[0]["artist"], "Policy Artist");

    let stored_runs: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM policy_test_runs")
        .fetch_one(&db.pool)
        .await
        .expect("count test runs");
    assert_eq!(stored_runs, 1, "the ruleId test run is persisted");

    // A non-matching stored rule reports zero matches honestly.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({ "ruleId": "POL-GH-REDUNDANT" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["matchedCount"], 0);
    assert_eq!(body["evaluated"], 1);

    // An ad-hoc rule definition is evaluated but never persisted (the
    // policy_test_runs foreign key requires a durably stored rule set).
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({
            "rule": {
                "id": "POL-ADHOC-DELETE",
                "ruleType": "greatest_hits_redundancy",
                "disposition": "delete",
                "confidenceThreshold": 9000,
                "predicate": {}
            }
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["persisted"], false);
    assert!(body["testRunId"].is_null());
    assert_eq!(body["destructive"], true);
    assert_eq!(body["matchedCount"], 0);
    let stored_runs: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM policy_test_runs")
        .fetch_one(&db.pool)
        .await
        .expect("count test runs");
    assert_eq!(stored_runs, 2, "ad-hoc test runs are not persisted");
    // The ad-hoc candidate never became a rule-set version.
    let versions = PolicyStore
        .list_rule_set_versions(&db.pool, melodee_policy::DEFAULT_RULE_SET_NAME)
        .await
        .expect("version history");
    assert_eq!(versions.len(), 1, "testing never mutates the rule set");

    // An invalid ad-hoc rule is rejected before any evaluation.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({
            "rule": {
                "id": "POL-BAD",
                "ruleType": "corrupt_media",
                "disposition": "accept",
                "predicate": {}
            }
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_rule");

    // Unknown ruleId, and ruleId+rule together, are rejected.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({ "ruleId": "POL-NOPE" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/policies/rules/test",
        Some(serde_json::json!({ "ruleId": "POL-CORRUPT", "rule": {
            "id": "POL-X", "ruleType": "unknown_classification",
            "disposition": "review", "predicate": {}
        } })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_test_request");

    db.drop().await;
}
