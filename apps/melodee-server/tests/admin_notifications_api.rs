//! Admin API integration tests: notification template CRUD
//! over the (key, locale) identity, the administrator-only gate, the
//! append-only delivery audit listing, and honest test-send recording with
//! a disabled transport (PRD-NOTIF-001..003, ADR-0134 §4).

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
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_notifications::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-notifications-jwt-32b";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("wp09-admin", true), ("wp09-plain", false)] {
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
        tokens.push(
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
            .expect("issue token"),
        );
    }
    Fixture {
        admin_token: tokens.remove(0),
        plain_token: tokens.remove(0),
    }
}

/// The fixture transport is disabled (noop), so test-sends record an honest
/// `skipped` delivery instead of touching a network.
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
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, json)
}

#[tokio::test]
async fn template_lifecycle_list_upsert_get_delete() {
    let db = common::TestDb::create("wp09_templates").await;
    let fixture = seed(&db).await;

    // The migration seeds the three wired `en` templates.
    let (status, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/notifications/templates",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let keys: Vec<&str> = listed
        .as_array()
        .expect("array")
        .iter()
        .map(|row| row["templateKey"].as_str().expect("key"))
        .collect();
    assert_eq!(
        keys,
        vec!["request.fulfilled", "share.created", "user.password_reset"]
    );
    let reset = &listed.as_array().expect("array")[2];
    assert_eq!(reset["subject"], "Reset your Melodee password");
    assert_eq!(reset["version"], 1);

    // Upsert creates a new (key, locale) row.
    let (status, created) = call(
        app(db.pool.clone()),
        "PUT",
        "/api/admin/v1/notifications/templates/custom.event/en-US",
        Some(json!({"subject": "Hi {{name}}", "bodyText": "Hello {{name}}"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["templateKey"], "custom.event");
    assert_eq!(created["locale"], "en-US");
    assert_eq!(created["version"], 1);
    assert_eq!(created["bodyHtml"], Value::Null);

    // A second upsert replaces and bumps the version; null clears a field.
    let (status, replaced) = call(
        app(db.pool.clone()),
        "PUT",
        "/api/admin/v1/notifications/templates/custom.event/en-US",
        Some(json!({"subject": null, "bodyText": "Only text"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["version"], 2);
    assert_eq!(replaced["subject"], Value::Null);
    assert_eq!(replaced["bodyText"], "Only text");

    let (status, fetched) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/notifications/templates/custom.event/en-US",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["version"], 2);

    // Deleting a seeded key is allowed (it disables the rendered body for
    // that event); the row is honestly gone afterwards.
    let (status, _) = call(
        app(db.pool.clone()),
        "DELETE",
        "/api/admin/v1/notifications/templates/share.created/en",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, gone) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/notifications/templates/share.created/en",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    assert_eq!(gone["code"], "not_found");
    db.drop().await;
}

#[tokio::test]
async fn validation_bounds_are_enforced_over_the_api() {
    let db = common::TestDb::create("wp09_validation").await;
    let fixture = seed(&db).await;

    let oversized_subject = "s".repeat(501);
    let oversized_body = "b".repeat(64 * 1024 + 1);
    for (name, path, body) in [
        (
            "oversized-subject",
            "/api/admin/v1/notifications/templates/k/en".to_string(),
            json!({"subject": oversized_subject}),
        ),
        (
            "oversized-body",
            "/api/admin/v1/notifications/templates/k/en".to_string(),
            json!({"bodyText": oversized_body}),
        ),
        (
            "invalid-locale",
            "/api/admin/v1/notifications/templates/k/english".to_string(),
            json!({"subject": "x"}),
        ),
    ] {
        let (status, body_out) = call(
            app(db.pool.clone()),
            "PUT",
            &path,
            Some(body),
            Some(&fixture.admin_token),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body_out}");
        assert_eq!(body_out["code"], "invalid_template", "{name}");
    }
    db.drop().await;
}

#[tokio::test]
async fn administrator_gate_rejects_anonymous_and_non_admin() {
    let db = common::TestDb::create("wp09_gate").await;
    let fixture = seed(&db).await;

    for (bearer, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(fixture.plain_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        for (method, path) in [
            ("GET", "/api/admin/v1/notifications/templates".to_string()),
            ("GET", "/api/admin/v1/notifications/deliveries".to_string()),
            ("POST", "/api/admin/v1/notifications/test-send".to_string()),
            (
                "DELETE",
                "/api/admin/v1/notifications/templates/share.created/en".to_string(),
            ),
        ] {
            let body = (method == "POST").then(
                || json!({"templateKey": "user.password_reset", "recipient": "ops@example.test"}),
            );
            let (status, _) = call(app(db.pool.clone()), method, &path, body, bearer).await;
            assert_eq!(status, expected, "{method} {path}");
        }
    }
    db.drop().await;
}

#[tokio::test]
async fn test_send_with_disabled_transport_records_an_honest_skip() {
    let db = common::TestDb::create("wp09_test_send").await;
    let fixture = seed(&db).await;

    let (status, empty) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/notifications/deliveries",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty.as_array().expect("array").len(), 0);

    let (status, delivery) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/notifications/test-send",
        Some(json!({"templateKey": "user.password_reset", "recipient": "ops@example.test"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delivery}");
    assert_eq!(delivery["templateKey"], "user.password_reset");
    assert_eq!(delivery["recipient"], "ops@example.test");
    assert_eq!(delivery["status"], "skipped");
    assert!(
        delivery["error"]
            .as_str()
            .expect("skip reason")
            .contains("disabled"),
        "{delivery}"
    );
    // The audit row is metadata only: no subject or body ever leaves the
    // server through this surface.
    assert!(delivery.get("subject").is_none());
    assert!(delivery.get("bodyText").is_none());

    // Unknown templates are an honest skip, not an error.
    let (status, missing) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/notifications/test-send",
        Some(json!({"templateKey": "no.such.event", "recipient": "ops@example.test"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{missing}");
    assert_eq!(missing["status"], "skipped");
    assert_eq!(missing["error"], "template not found");

    // Malformed recipients are rejected without recording anything.
    let (status, bad) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/notifications/test-send",
        Some(json!({"templateKey": "user.password_reset", "recipient": "not-an-email"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");

    // Both honest rows are listed newest first.
    let (status, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/notifications/deliveries?limit=10",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed.as_array().expect("array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["templateKey"], "no.such.event");
    assert_eq!(rows[1]["templateKey"], "user.password_reset");
    db.drop().await;
}
