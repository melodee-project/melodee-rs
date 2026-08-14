//! Phase 9 WP-06 Admin API radio integration tests (PRD-RADIO-001/002,
//! ADR-0134 §1): station CRUD lifecycle, list/read for any authenticated
//! user, admin gating on mutations matching the captured OpenSubsonic gate,
//! validation failures, and cross-surface visibility with the OpenSubsonic
//! adapter over the shared `opensubsonic_radio_stations` table.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::opensubsonic::{MusicFolder, OsState};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-radio-jwt-key-at-least-32b";
const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const OS_SECRET: &str = "fixture-radio-os-secret-NOT-REAL";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
    role_admin_public: Uuid,
}

/// Seeds three users: a flag admin, a plain user, and a user holding only the
/// `Administrator` role (no `is_admin` flag) to prove the mutation gate
/// matches the captured OpenSubsonic gating. The flag admin also gets an
/// OpenSubsonic secret for the cross-surface calls.
async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let fixture = Fixture {
        admin_public: Uuid::now_v7(),
        plain_public: Uuid::now_v7(),
        role_admin_public: Uuid::now_v7(),
    };
    for (username, is_admin, public_id, roles) in [
        (
            "wp06-admin",
            true,
            fixture.admin_public,
            vec!["Administrator".to_string(), "HasStreamRole".to_string()],
        ),
        (
            "wp06-plain",
            false,
            fixture.plain_public,
            vec!["HasStreamRole".to_string()],
        ),
        (
            "wp06-role-admin",
            false,
            fixture.role_admin_public,
            vec!["Administrator".to_string()],
        ),
    ] {
        let email = format!("{username}@example.test");
        let normalized = username.to_uppercase();
        let normalized_email = email.to_uppercase();
        AuthStore::new()
            .import_user(
                &db.pool,
                &NativeUserImport {
                    id: Uuid::now_v7(),
                    public_api_id: public_id,
                    username,
                    normalized_username: &normalized,
                    email: &email,
                    normalized_email: &normalized_email,
                    password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("fixture hash"),
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
                    roles: &roles,
                    created_at: now,
                    updated_at: now,
                },
            )
            .await
            .expect("import user");
    }
    let internal_id: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username='WP06-ADMIN'")
            .fetch_one(&db.pool)
            .await
            .expect("admin internal id");
    sqlx::query(
        "INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) \
         VALUES ($1, $2, now(), now())",
    )
    .bind(internal_id)
    .bind(OS_SECRET)
    .execute(&db.pool)
    .await
    .expect("seed os secret");
    fixture
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

fn app(pool: sqlx::PgPool) -> axum::Router {
    melodee_server::admin_radio::router().with_state(AdminAccessState {
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

// ---------------------------------------------------------------------------
// OpenSubsonic cross-surface helpers (same pool, captured token auth)
// ---------------------------------------------------------------------------

fn os_state(pool: sqlx::PgPool) -> OsState {
    OsState {
        pool,
        server_version: "0.0.0-test".to_string(),
        public_origin: "http://melodee.test".to_string(),
        base_url_host: Some("capture-fixture.example.test".to_string()),
        music_folders: vec![Arc::new(MusicFolder {
            id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
            name: "Fixture Library".to_string(),
        })],
        podcasts_enabled: true,
        downloads_enabled: true,
        jukebox_enabled: false,
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
    }
}

fn os_authed(path: &str) -> String {
    let salt = path.len().to_string();
    let mut hasher = Md5::new();
    hasher.update(OS_SECRET.as_bytes());
    hasher.update(salt.as_bytes());
    let token = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{path}&u=wp06-admin&t={token}&s={salt}")
}

async fn os_call(state: OsState, path: &str) -> Value {
    let mut request = Request::builder()
        .method("GET")
        .uri(os_authed(path))
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
    let response = melodee_server::opensubsonic::configured_router(state)
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice::<Value>(&bytes).expect("json")["subsonic-response"].clone()
}

async fn os_radio_list(state: OsState) -> Vec<Value> {
    os_call(state, "/rest/getInternetRadioStations.view?f=json")
        .await["internetRadioStations"]["internetRadioStation"]
        .as_array()
        .expect("radio list")
        .clone()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn station_lifecycle_via_admin_api() {
    let db = common::TestDb::create("wp06_radio_lifecycle").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp06-admin");
    let plain = token_for(fixture.plain_public, "wp06-plain");
    let app = app(db.pool.clone());

    // Create (admin).
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/radio/stations",
        Some(serde_json::json!({
            "name": "WP06 Radio",
            "streamUrl": "http://wp06.example.test/stream",
            "homePageUrl": "http://wp06.example.test"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("station id").to_string();
    assert_eq!(created["name"], "WP06 Radio");
    assert_eq!(created["streamUrl"], "http://wp06.example.test/stream");
    assert_eq!(created["homePageUrl"], "http://wp06.example.test");

    // List/read are available to any authenticated user, not only admins.
    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/radio/stations",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("array").len(), 1);
    let (status, fetched) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["id"], id);

    // Update: rename and clear the homepage via explicit null.
    let (status, updated) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/radio/stations/{id}"),
        Some(serde_json::json!({"name": "WP06 Renamed", "homePageUrl": null})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "WP06 Renamed");
    assert_eq!(updated["homePageUrl"], Value::Null);
    assert_eq!(updated["streamUrl"], "http://wp06.example.test/stream");
    // The public id is stable across updates.
    assert_eq!(updated["id"], id);

    // Delete, then reads are 404.
    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn mutations_are_admin_gated_listing_is_authenticated() {
    let db = common::TestDb::create("wp06_radio_gating").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp06-admin");
    let plain = token_for(fixture.plain_public, "wp06-plain");
    let role_admin = token_for(fixture.role_admin_public, "wp06-role-admin");
    let app = app(db.pool.clone());

    // Anonymous callers: 401 on every route.
    for (method, path, body) in [
        ("GET", "/api/admin/v1/radio/stations".to_string(), None),
        (
            "POST",
            "/api/admin/v1/radio/stations".to_string(),
            Some(serde_json::json!({"name": "Nope", "streamUrl": "http://x.example.test"})),
        ),
        (
            "PUT",
            format!("/api/admin/v1/radio/stations/{}", Uuid::now_v7()),
            Some(serde_json::json!({"name": "Nope"})),
        ),
        (
            "DELETE",
            format!("/api/admin/v1/radio/stations/{}", Uuid::now_v7()),
            None,
        ),
    ] {
        let (status, _) = call(app.clone(), method, &path, body.clone(), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {path}"
        );
        let (status, _) = call(app.clone(), method, &path, body, Some("not-a-jwt")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "bad token {method} {path}"
        );
    }

    // Authenticated non-admin: list/read succeed, mutations are 403.
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/radio/stations",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (method, path, body) in [
        (
            "POST",
            "/api/admin/v1/radio/stations".to_string(),
            Some(serde_json::json!({"name": "Nope", "streamUrl": "http://x.example.test"})),
        ),
        (
            "PUT",
            format!("/api/admin/v1/radio/stations/{}", Uuid::now_v7()),
            Some(serde_json::json!({"name": "Nope"})),
        ),
        (
            "DELETE",
            format!("/api/admin/v1/radio/stations/{}", Uuid::now_v7()),
            None,
        ),
    ] {
        let (status, body) = call(app.clone(), method, &path, body, Some(&plain)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "non-admin {method} {path}");
        assert_eq!(body["code"], "forbidden");
    }

    // The captured gate accepts the `Administrator` role even without the
    // `is_admin` flag (matching opensubsonic/radio.rs require_admin).
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/radio/stations",
        Some(serde_json::json!({"name": "Role Admin Radio", "streamUrl": "http://role.example.test/s"})),
        Some(&role_admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // The flag admin can delete what the role admin created.
    let id = created["id"].as_str().expect("id").to_string();
    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    db.drop().await;
}

#[tokio::test]
async fn validation_failures_are_400_and_persist_nothing() {
    let db = common::TestDb::create("wp06_radio_validation").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp06-admin");
    let app = app(db.pool.clone());

    for (body, code) in [
        (
            serde_json::json!({"name": "  ", "streamUrl": "http://x.example.test"}),
            "invalid_name",
        ),
        (
            serde_json::json!({"name": "n".repeat(501), "streamUrl": "http://x.example.test"}),
            "invalid_name",
        ),
        (
            serde_json::json!({"name": "Ok", "streamUrl": " "}),
            "invalid_stream_url",
        ),
        (
            serde_json::json!({
                "name": "Ok",
                "streamUrl": "http://x.example.test",
                "homePageUrl": "h".repeat(2001)
            }),
            "invalid_home_page_url",
        ),
    ] {
        let (status, body) = call(
            app.clone(),
            "POST",
            "/api/admin/v1/radio/stations",
            Some(body),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], code);
    }

    // Invalid updates fail the same way and unknown ids are 404.
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/radio/stations",
        Some(serde_json::json!({"name": "Valid", "streamUrl": "http://v.example.test/s"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().expect("id").to_string();
    let (status, body) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/radio/stations/{id}"),
        Some(serde_json::json!({"streamUrl": ""})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_stream_url");
    let (status, fetched) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/radio/stations/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["streamUrl"], "http://v.example.test/s");

    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/radio/stations/{}", Uuid::now_v7()),
        Some(serde_json::json!({"name": "Ghost"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn admin_and_opensubsonic_surfaces_share_one_store() {
    let db = common::TestDb::create("wp06_radio_cross_surface").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp06-admin");
    let app = app(db.pool.clone());
    let state = os_state(db.pool.clone());

    // Admin API -> OpenSubsonic: a station created via the admin API appears
    // in getInternetRadioStations with the captured `radio_{uuid}` id shape.
    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/radio/stations",
        Some(serde_json::json!({
            "name": "Admin Created",
            "streamUrl": "http://admin.example.test/stream",
            "homePageUrl": "http://admin.example.test"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let admin_id = created["id"].as_str().expect("id").to_string();

    let os_list = os_radio_list(state.clone()).await;
    assert_eq!(os_list.len(), 1, "OS list sees the admin-created station");
    assert_eq!(os_list[0]["id"], format!("radio_{admin_id}"));
    assert_eq!(os_list[0]["name"], "Admin Created");
    assert_eq!(os_list[0]["streamUrl"], "http://admin.example.test/stream");
    assert_eq!(os_list[0]["homePageUrl"], "http://admin.example.test");

    // OpenSubsonic -> Admin API: a station created via the captured OS route
    // is listed and readable through the admin API with the same public id.
    let create_path = "/rest/createInternetRadioStation.view?f=json&name=OS+Created&streamUrl=http://os.example.test/stream";
    let env = os_call(state.clone(), create_path).await;
    assert_eq!(env["status"], "ok");

    let (status, list) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/radio/stations",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stations = list.as_array().expect("array");
    assert_eq!(stations.len(), 2, "admin list sees both surfaces' stations");
    let os_station = stations
        .iter()
        .find(|station| station["name"] == "OS Created")
        .expect("os-created station in admin list");
    let os_public_id = os_station["id"]
        .as_str()
        .expect("os station id")
        .to_string();
    let os_wire = os_radio_list(state.clone()).await;
    let wire = os_wire
        .iter()
        .find(|station| station["name"] == "OS Created")
        .expect("os-created station in OS list");
    assert_eq!(wire["id"], format!("radio_{os_public_id}"));

    // Admin-side update/delete are visible on the OS surface immediately.
    let (status, updated) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/radio/stations/{os_public_id}"),
        Some(serde_json::json!({"name": "OS Renamed By Admin"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let os_wire = os_radio_list(state.clone()).await;
    assert!(
        os_wire
            .iter()
            .any(|station| station["name"] == "OS Renamed By Admin"),
        "OS list reflects the admin rename: {os_wire:?}"
    );
    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/radio/stations/{os_public_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let os_wire = os_radio_list(state.clone()).await;
    assert_eq!(os_wire.len(), 1);
    assert_eq!(os_wire[0]["name"], "Admin Created");
    db.drop().await;
}
