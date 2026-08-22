//! Admin API integration tests: scripting hook administration
//! over the (event, name) upsert identity, the administrator-only gate, the
//! append-only execution audit listing, and honest operator test-runs in the
//! real sandbox (PRD-SCRIPT-001/004, ADR-0134 §6, ADR-0135).

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
use melodee_server::admin_scripts::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-scripts-jwt-32-bytes!!";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("wp11-admin", true), ("wp11-plain", false)] {
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

const EVENT: &str = "directoryProcessingStart";

#[tokio::test]
async fn script_lifecycle_upsert_get_list_enable_disable_delete() {
    let db = common::TestDb::create("wp11_scripts_lifecycle").await;
    let fixture = seed(&db).await;

    // Nothing is seeded: scripts are operator-installed only.
    let (status, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/scripts",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["scripts"].as_array().expect("array").len(), 0);
    assert_eq!(listed["wiredEvents"], json!([EVENT]));

    // Upsert creates a new (event, name) row, enabled by default.
    let (status, created) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        Some(json!({
            "source": "ctx.mediaFilesCount >= 3",
            "timeoutMs": 500
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["event"], EVENT);
    assert_eq!(created["name"], "min-three-tracks");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["wired"], true, "the wired event is flagged live");
    assert_eq!(created["version"], 1);
    assert_eq!(created["timeoutMs"], 500);
    let id = created["id"].as_str().expect("id").to_string();

    // Re-saving identical source churns updatedAt but not the version.
    let (status, resaved) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        Some(json!({"source": "ctx.mediaFilesCount >= 3", "timeoutMs": 500})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resaved}");
    assert_eq!(resaved["version"], 1);
    // A source change bumps the version.
    let (status, changed) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        Some(json!({"source": "ctx.mediaFilesCount >= 4"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["version"], 2);
    assert_eq!(
        changed["timeoutMs"], 250,
        "omitted limits reset to defaults"
    );

    // An inert event is accepted but flagged as not wired.
    let (status, inert) = call(
        app(db.pool.clone()),
        "PUT",
        "/api/admin/v1/scripts/userLoginStart/future-hook",
        Some(json!({"source": "true"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inert}");
    assert_eq!(inert["wired"], false);

    // Disable / enable by id; upsert never re-enables.
    let (status, disabled) = call(
        app(db.pool.clone()),
        "POST",
        &format!("/api/admin/v1/scripts/id/{id}/disable"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["enabled"], false);
    let (status, still_disabled) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        Some(json!({"source": "ctx.mediaFilesCount >= 5"})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{still_disabled}");
    assert_eq!(still_disabled["enabled"], false);
    let (status, enabled) = call(
        app(db.pool.clone()),
        "POST",
        &format!("/api/admin/v1/scripts/id/{id}/enable"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["enabled"], true);

    // Get by name and by id; list shows both rows.
    let (status, by_name) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_name}");
    let (status, by_id) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/scripts/id/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_id}");
    assert_eq!(by_id["name"], "min-three-tracks");
    let (status, listed) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/scripts?limit=10",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["scripts"].as_array().expect("array").len(), 2);

    // Delete by id; past runs survive (covered in the test-run test).
    let (status, _) = call(
        app(db.pool.clone()),
        "DELETE",
        &format!("/api/admin/v1/scripts/id/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, gone) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/scripts/id/{id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    db.drop().await;
}

#[tokio::test]
async fn validation_bounds_are_enforced() {
    let db = common::TestDb::create("wp11_scripts_bounds").await;
    let fixture = seed(&db).await;

    for (path, body) in [
        (
            format!("/api/admin/v1/scripts/{EVENT}/"),
            json!({"source": "true"}),
        ),
        (
            format!("/api/admin/v1/scripts/{EVENT}/ok"),
            json!({"source": ""}),
        ),
        (
            format!("/api/admin/v1/scripts/{EVENT}/ok"),
            json!({"source": "true", "timeoutMs": 10001}),
        ),
        (
            format!("/api/admin/v1/scripts/{EVENT}/ok"),
            json!({"source": "true", "timeoutMs": 0}),
        ),
        (
            format!("/api/admin/v1/scripts/{EVENT}/ok"),
            json!({"source": "true", "memoryBytes": 1024}),
        ),
        (
            format!("/api/admin/v1/scripts/{}/ok", "e".repeat(101)),
            json!({"source": "true"}),
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
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "PUT {path} -> {status} {body_out}"
        );
    }
    db.drop().await;
}

#[tokio::test]
async fn admin_gate_rejects_anonymous_and_non_admin() {
    let db = common::TestDb::create("wp11_scripts_gate").await;
    let fixture = seed(&db).await;

    for (bearer, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(fixture.plain_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        let (status, _) = call(
            app(db.pool.clone()),
            "GET",
            "/api/admin/v1/scripts",
            None,
            bearer,
        )
        .await;
        assert_eq!(status, expected);
        let (status, _) = call(
            app(db.pool.clone()),
            "POST",
            "/api/admin/v1/scripts/test-run",
            Some(json!({"event": EVENT, "source": "true", "ctx": {}})),
            bearer,
        )
        .await;
        assert_eq!(status, expected);
    }
    db.drop().await;
}

#[tokio::test]
async fn test_run_evaluates_in_the_sandbox_and_records_honest_runs() {
    let db = common::TestDb::create("wp11_scripts_testrun").await;
    let fixture = seed(&db).await;

    // Register a stored script and run it against a denying ctx mock.
    let (status, created) = call(
        app(db.pool.clone()),
        "PUT",
        &format!("/api/admin/v1/scripts/{EVENT}/min-three-tracks"),
        Some(json!({
            "source": "function check(ctx, scriptConfig) { if (ctx.mediaFilesCount < 3) { return { result: false, message: \"Release has fewer than three media files.\" }; } return true; }"
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let script_id = created["id"].as_str().expect("id").to_string();

    let (status, denied) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/scripts/test-run",
        Some(json!({
            "event": EVENT,
            "scriptId": script_id,
            "ctx": { "mediaFilesCount": 2 }
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    assert_eq!(denied["evaluation"]["outcome"], "deny");
    assert_eq!(
        denied["evaluation"]["message"],
        "Release has fewer than three media files."
    );
    assert_eq!(denied["effectiveOnDeny"], "skip");
    assert_eq!(denied["run"]["scriptId"], script_id);
    assert_eq!(denied["run"]["outcome"], "deny");

    // Ad-hoc source runs are recorded with a NULL script link.
    let (status, adhoc) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/scripts/test-run",
        Some(json!({
            "event": EVENT,
            "source": "ctx.mediaFilesCount >= 3 && !ctx.hasTrackNumberGaps",
            "ctx": { "mediaFilesCount": 5, "hasTrackNumberGaps": false }
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{adhoc}");
    assert_eq!(adhoc["evaluation"]["outcome"], "allow");
    assert_eq!(adhoc["run"]["scriptId"], Value::Null);

    // A throwing script is an honest error outcome, not an HTTP failure.
    let (status, thrown) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/scripts/test-run",
        Some(json!({
            "event": EVENT,
            "source": "function check(ctx, scriptConfig) { throw new Error('nope'); }",
            "ctx": {}
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{thrown}");
    assert_eq!(thrown["evaluation"]["outcome"], "error");
    assert!(
        thrown["evaluation"]["error"]
            .as_str()
            .expect("error")
            .contains("nope")
    );

    // The sandbox registers no host capabilities (proven via test-run).
    let (status, sandbox) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/scripts/test-run",
        Some(json!({
            "event": EVENT,
            "source": "typeof process === 'undefined' && typeof require === 'undefined' && typeof fetch === 'undefined'",
            "ctx": {}
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sandbox}");
    assert_eq!(sandbox["evaluation"]["outcome"], "allow");

    // Malformed requests: both or neither of scriptId/source; unknown id.
    for body in [
        json!({"event": EVENT, "ctx": {}}),
        json!({"event": EVENT, "scriptId": script_id, "source": "true", "ctx": {}}),
        json!({"event": "  ", "source": "true", "ctx": {}}),
        json!({"event": EVENT, "source": "  ", "ctx": {}}),
        json!({"event": EVENT, "scriptId": Uuid::now_v7(), "ctx": {}}),
    ] {
        let (status, out) = call(
            app(db.pool.clone()),
            "POST",
            "/api/admin/v1/scripts/test-run",
            Some(body.clone()),
            Some(&fixture.admin_token),
        )
        .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{body} -> {status} {out}"
        );
    }

    // The runs listing is bounded, newest first, and scopeable.
    let (status, runs) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/scripts/runs?limit=10",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{runs}");
    let rows = runs.as_array().expect("array");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["outcome"], "allow", "newest first");

    let (status, scoped) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/scripts/runs?scriptId={script_id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scoped}");
    let rows = scoped.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["outcome"], "deny");

    // Deleting the script leaves its run rows auditable (ON DELETE SET NULL).
    let (status, _) = call(
        app(db.pool.clone()),
        "DELETE",
        &format!("/api/admin/v1/scripts/id/{script_id}"),
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, runs) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/scripts/runs?limit=10",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{runs}");
    assert_eq!(runs.as_array().expect("array").len(), 4);
    db.drop().await;
}
