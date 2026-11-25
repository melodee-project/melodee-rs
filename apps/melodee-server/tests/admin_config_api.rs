//! Admin API integration tests: configuration export with
//! secret redaction, import preview/apply, unknown-key and redacted-marker
//! rejection, and the administrator gate (PRD-CONFIG-002, PRD-AUTH-007,
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
use melodee_persistence::{AuthStore, NativeUserImport, SECRET_REDACTION_MARKER, SettingsStore};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_config::router;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-config-jwt-key-32-bytes!!";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("wp08-admin", true), ("wp08-plain", false)] {
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
async fn export_redacts_secrets_and_carries_the_warning() {
    let db = common::TestDb::create("wp08_config_export").await;
    let fixture = seed(&db).await;
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();
    store
        .set(&db.pool, "system.siteName", &json!("Example"), None, now)
        .await
        .expect("set site name");
    store
        .set(
            &db.pool,
            "email.smtpPassword",
            &json!("super-secret-password"),
            None,
            now,
        )
        .await
        .expect("set secret");

    let (status, document) = call(
        app(db.pool.clone()),
        "GET",
        "/api/admin/v1/config/export",
        None,
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{document}");
    assert_eq!(document["format"], "melodee-config-export/v1");
    assert!(
        document["warning"]
            .as_str()
            .expect("warning")
            .contains("REDACTED")
    );
    let settings = &document["settings"];
    assert_eq!(settings["system.siteName"], "Example");
    assert_eq!(settings["magic.enabled"], true, "built-in default");
    assert_eq!(settings["email.smtpPassword"], SECRET_REDACTION_MARKER);
    // The raw document text never contains the secret value.
    assert!(!document.to_string().contains("super-secret-password"));
    // Compromised/placeholder legacy defaults are never carried.
    assert!(settings.get("encryption.privateKey").is_none());
    assert!(settings.get("jellyfin.token.pepper").is_none());
    db.drop().await;
}

#[tokio::test]
async fn import_previews_by_default_and_applies_explicitly() {
    let db = common::TestDb::create("wp08_config_import").await;
    let fixture = seed(&db).await;
    let document = json!({
        "format": "melodee-config-export/v1",
        "warning": "...",
        "exportedAt": "2026-08-14T00:00:00Z",
        "settings": {
            "system.siteName": "Imported",
            "email.smtpPassword": SECRET_REDACTION_MARKER,
            "scrobbling.lastFm.apiKey": "reentered-secret",
            "custom.unknown": "value"
        }
    });

    // Default is preview: nothing written, diff reported, secrets named
    // only.
    let (status, plan) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import",
        Some(document.clone()),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["applied"], false);
    assert!(
        plan["added"]
            .as_array()
            .expect("added")
            .contains(&json!("system.siteName"))
    );
    assert_eq!(plan["secretsSkipped"], json!(["email.smtpPassword"]));
    assert_eq!(
        plan["secretsReentered"],
        json!(["scrobbling.lastFm.apiKey"])
    );
    assert_eq!(plan["unknown"], json!(["custom.unknown"]));
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning.as_str().expect("warning").contains("re-enter"))
    );
    // The plan never leaks the re-entered secret value.
    assert!(!plan.to_string().contains("reentered-secret"));
    assert!(
        SettingsStore::new()
            .get(&db.pool, "system.siteName")
            .await
            .expect("get")
            .is_none(),
        "preview writes nothing"
    );

    // Apply rejects unknown keys without allowUnknown (fail closed, atomic).
    let (status, rejection) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import?preview=false",
        Some(document.clone()),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejection}");
    assert_eq!(rejection["code"], "invalid_document");
    assert!(
        SettingsStore::new()
            .get(&db.pool, "system.siteName")
            .await
            .expect("get")
            .is_none()
    );

    // Apply with unknown admitted writes rows, skips the redacted secret,
    // stores the re-entered secret force-flagged, and records evidence.
    let (status, applied) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import?preview=false&allowUnknown=true",
        Some(document.clone()),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["applied"], true);
    let store = SettingsStore::new();
    let site = store
        .get(&db.pool, "system.siteName")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(site.value.to_json(), json!("Imported"));
    assert!(
        store
            .get(&db.pool, "email.smtpPassword")
            .await
            .expect("get")
            .is_none(),
        "the redaction marker is never stored"
    );
    let reentered = store
        .get(&db.pool, "scrobbling.lastFm.apiKey")
        .await
        .expect("get")
        .expect("row");
    assert!(reentered.is_secret);
    assert_eq!(reentered.value.to_json(), json!(SECRET_REDACTION_MARKER));
    let custom = store
        .get(&db.pool, "custom.unknown")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(custom.value.to_json(), json!("value"));
    let onboarding = melodee_persistence::OnboardingStore::new()
        .get_state(&db.pool)
        .await
        .expect("state");
    let imports = onboarding.evidence.expect("evidence")["configImports"]
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["secretsReentered"], 1);
    assert_eq!(imports[0]["secretsSkipped"], 1);
    db.drop().await;
}

#[tokio::test]
async fn import_rejects_redacted_markers_and_invalid_values_fail_closed() {
    let db = common::TestDb::create("wp08_config_reject").await;
    let fixture = seed(&db).await;

    // Marker for a non-secret key: hard rejection.
    let (status, body) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import",
        Some(json!({
            "format": "melodee-config-export/v1",
            "settings": { "system.siteName": SECRET_REDACTION_MARKER }
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "redacted_marker_rejected");

    // Kind violation (string where an integer range applies).
    let (status, body) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import",
        Some(json!({
            "format": "melodee-config-export/v1",
            "settings": { "defaults.batchSize": 100 }
        })),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_value");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("defaults.batchSize")
    );

    // Malformed document.
    let (status, body) = call(
        app(db.pool.clone()),
        "POST",
        "/api/admin/v1/config/import",
        Some(json!({"settings": {}})),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_document");
    db.drop().await;
}

#[tokio::test]
async fn administrator_gate_rejects_anonymous_and_non_admin() {
    let db = common::TestDb::create("wp08_config_gate").await;
    let fixture = seed(&db).await;
    for (bearer, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(fixture.plain_token.as_str()), StatusCode::FORBIDDEN),
    ] {
        let (status, _) = call(
            app(db.pool.clone()),
            "GET",
            "/api/admin/v1/config/export",
            None,
            bearer,
        )
        .await;
        assert_eq!(status, expected, "export");
        let (status, _) = call(
            app(db.pool.clone()),
            "POST",
            "/api/admin/v1/config/import",
            Some(json!({"format": "melodee-config-export/v1", "settings": {}})),
            bearer,
        )
        .await;
        assert_eq!(status, expected, "import");
    }
    db.drop().await;
}

#[tokio::test]
async fn administrator_tunes_workers_and_host_settings_cannot_be_changed() {
    use melodee_persistence::runtime_settings as runtime;
    let db = common::TestDb::create("admin_worker_edit").await;
    let fixture = seed(&db).await;
    let app = app(db.pool.clone());
    let entries = "/api/admin/v1/config/entries";
    let key = "MELODEE__INSPECTION__CONCURRENCY";
    let path = format!("{entries}/{key}");
    let admin = Some(fixture.admin_token.as_str());
    let value = |v: Value| Some(json!({"action":"set","value":v}));
    assert_eq!(
        call(app.clone(), "GET", entries, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            &path,
            value(json!(7)),
            Some(&fixture.plain_token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(app.clone(), "PUT", &path, value(json!(7)), admin)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (status, rows) = call(app.clone(), "GET", entries, None, admin).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let entry = |key: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .unwrap()
    };
    assert_eq!(entry(key)["savedValue"], "7");
    assert_eq!(entry(key)["savedSource"], "database");
    assert_eq!(
        entry(key)["pendingRestart"],
        entry(key)["value"] != "7" || entry(key)["source"] != "database"
    );
    assert_eq!(entry(key)["canReset"], true);
    let db_url = entry("MELODEE__DATABASE__URL");
    assert_eq!(db_url["editable"], false);
    assert!(db_url["value"].is_null() || db_url["value"] == SECRET_REDACTION_MARKER);
    let loaded = runtime::load(&db.pool).await.unwrap();
    let config = runtime::preview(loaded, melodee_ingestion::IngestionConfig::from_env).unwrap();
    assert_eq!(config.inspection_concurrency, 7);

    for (key, bad) in [
        (key, json!(0)),
        ("MELODEE__NATIVE__AUTH__JWT_KEY", json!("replacement")),
        (
            "MELODEE__NORMALIZATION__FFMPEG_PATH",
            json!("/different/tool"),
        ),
        ("MELODEE__STORAGE__ROOT", json!("/different/library")),
        ("database.url", json!("secret")),
        ("MELODEE__WORK__RETRY_BASE_SECONDS", json!(4000000000_i64)),
    ] {
        let (status, _) = call(
            app.clone(),
            "PUT",
            &format!("{entries}/{key}"),
            value(bad),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}");
    }
    assert_eq!(runtime::load(&db.pool).await.unwrap().len(), 1);
    let import =
        json!({"format":"melodee-config-export/v1","settings":{"mpv.path":"/different/tool"}});
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/api/admin/v1/config/import?preview=false&allowUnknown=true",
            Some(import),
            admin
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            &path,
            Some(json!({"action":"reset"})),
            admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(runtime::load(&db.pool).await.unwrap().is_empty());

    // An individually valid boolean still cannot enable a broken SMTP configuration.
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            &format!("{entries}/email.enabled"),
            value(json!(true)),
            admin
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        SettingsStore::new()
            .read_raw_value(&db.pool, "email.enabled")
            .await
            .unwrap()
            .is_none()
    );
    let path = format!("{entries}/email.smtpPassword");
    assert_eq!(
        call(
            app.clone(),
            "PUT",
            &path,
            value(json!("synthetic-test-only")),
            admin
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, rows) = call(app.clone(), "GET", entries, None, admin).await;
    assert!(!rows.to_string().contains("synthetic-test-only"));
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["key"] == "email.smtpPassword")
        .unwrap();
    assert_eq!(row["value"], SECRET_REDACTION_MARKER);
    let env =
        |name: &str| (name == "MELODEE__EMAIL__SMTP_PASSWORD").then(|| "host-test-value".into());
    let resolver = melodee_persistence::SettingsResolver::with_env_reader(&db.pool, &env);
    assert_eq!(
        resolver.resolve("email.smtpPassword").await.unwrap().source,
        melodee_persistence::SettingSource::Database
    );
    assert_eq!(
        call(app, "PUT", &path, Some(json!({"action":"reset"})), admin)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        resolver.resolve("email.smtpPassword").await.unwrap().source,
        melodee_persistence::SettingSource::Environment
    );
    db.drop().await;
}
