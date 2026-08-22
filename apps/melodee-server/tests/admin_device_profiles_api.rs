//! Admin API integration tests: device profile CRUD
//! lifecycle, the single-default invariant, legacy-parity validation, and
//! the administrator-only authorization gate (PRD-DEVICE-001/002,
//! ADR-0134 §5).

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
use melodee_server::admin_device_profiles::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-device-profiles-jwt-32b";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("wp10-admin", true), ("wp10-plain", false)] {
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

fn app(pool: sqlx::PgPool) -> axum::Router {
    router().with_state(AdminAccessState {
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(pool.clone()),
        ),
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

fn valid_profile(name: &str) -> Value {
    json!({
        "name": name,
        "clientMatch": {"clientName": "feishin"},
        "profile": {"directPlay": false, "targetCodec": "opus", "maxBitrate": 192}
    })
}

#[tokio::test]
async fn lifecycle_create_get_update_delete() {
    let db = common::TestDb::create("wp10_lifecycle").await;
    let fixture = seed(&db).await;

    let (status, created) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/device-profiles",
        Some(valid_profile("mobile")),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");
    assert_eq!(created["name"], "mobile");
    assert_eq!(created["isDefault"], false);

    let (status, fetched) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/device-profiles/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["profile"]["targetCodec"], "opus");

    let (status, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/device-profiles?limit=10",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().expect("array").len(), 1);

    let (status, updated) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/device-profiles/{id}"),
        Some(json!({"name": "mobile-hq", "profile": {"directPlay": true}})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "mobile-hq");
    assert_eq!(updated["profile"], json!({"directPlay": true}));
    // Omitted fields keep their stored value.
    assert_eq!(updated["clientMatch"], json!({"clientName": "feishin"}));

    let (status, _) = call(
        app(db.pool.clone()),
        "DELETE",
        &format!("/api/admin/v1/device-profiles/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, gone) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/device-profiles/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    assert_eq!(gone["code"], "not_found");
    db.drop().await;
}

#[tokio::test]
async fn single_default_invariant_over_the_api() {
    let db = common::TestDb::create("wp10_default").await;
    let fixture = seed(&db).await;

    let mut first = valid_profile("first");
    first["isDefault"] = json!(true);
    let (status, first) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/device-profiles",
        Some(first),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let first_id = first["id"].as_str().expect("id").to_string();

    // Creating a second default clears the first.
    let mut second = valid_profile("second");
    second["isDefault"] = json!(true);
    let (status, second) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/device-profiles",
        Some(second),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["isDefault"], true);

    let (_, first_after) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/device-profiles/{first_id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(first_after["isDefault"], false);

    // set-default flips atomically back.
    let (status, flipped) = call(
        app(db.pool.clone()),
        "POST",
        &format!("/api/admin/v1/device-profiles/{first_id}/default"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{flipped}");
    assert_eq!(flipped["isDefault"], true);
    let (_, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/device-profiles",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    let defaults = listed
        .as_array()
        .expect("array")
        .iter()
        .filter(|p| p["isDefault"] == true)
        .count();
    assert_eq!(defaults, 1, "exactly one default remains");

    // Unknown id on set-default is a 404.
    let (status, missing) = call(
        app(db.pool.clone()),
        "POST",
        &format!("/api/admin/v1/device-profiles/{}/default", Uuid::now_v7()),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    db.drop().await;
}

#[tokio::test]
async fn validation_rejects_legacy_rule_violations() {
    let db = common::TestDb::create("wp10_validation").await;
    let fixture = seed(&db).await;

    for (name, body) in [
        ("blank-name", json!({"name": "  "})),
        (
            "direct-play-with-codec",
            json!({"name": "x", "profile": {"directPlay": true, "targetCodec": "mp3"}}),
        ),
        (
            "transcode-without-codec",
            json!({"name": "x", "profile": {"directPlay": false}}),
        ),
        ("non-object-profile", json!({"name": "x", "profile": 42})),
        (
            "non-string-matcher",
            json!({"name": "x", "clientMatch": {"clientName": 7}}),
        ),
    ] {
        let (status, body_out) = call(
            app(db.pool.clone()),
            "POST",
            "/api/admin/v1/device-profiles",
            Some(body),
            Some(&fixture.admin_token),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body_out}");
        assert_eq!(body_out["code"], "invalid_profile", "{name}");
    }
    db.drop().await;
}

#[tokio::test]
async fn administrator_gate_rejects_anonymous_and_non_admin() {
    let db = common::TestDb::create("wp10_gate").await;
    let fixture = seed(&db).await;

    for (bearer, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(fixture.plain_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        for (method, path) in [
            ("GET", "/api/admin/v1/device-profiles".to_string()),
            ("POST", "/api/admin/v1/device-profiles".to_string()),
        ] {
            let body = (method == "POST").then(|| valid_profile("gated"));
            let (status, _) = call(app(db.pool.clone()), method, &path, body, bearer).await;
            assert_eq!(status, expected, "{method} {path}");
        }
    }
    db.drop().await;
}
