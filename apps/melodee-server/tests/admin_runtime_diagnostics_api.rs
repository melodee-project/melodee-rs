//! Admin runtime diagnostics API integration tests
//! (`/api/admin/v1/diagnostics/*`).
//!
//! These exercise the real router against real PostgreSQL: the administrator
//! gate, the selftest check set, effective-settings redaction, the recent
//! event ring buffer, and the redacted support bundle. The point of these
//! routes is troubleshooting by an operator or an automation agent, so the
//! tests assert the machine-readable shape stays stable and no secret value
//! leaks.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

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
use melodee_server::admin_diagnostics::AdminDiagnosticsState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-runtime-diagnostics-jwt-key-at-least-32b";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

async fn seed_users(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("runtime-diag-admin", true), ("runtime-diag-plain", false)] {
        let public_id = Uuid::now_v7();
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
        let issued = OffsetDateTime::now_utc().unix_timestamp();
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
    let mut tokens = tokens.into_iter();
    Fixture {
        admin_token: tokens.next().expect("admin token"),
        plain_token: tokens.next().expect("plain token"),
    }
}

fn diagnostics_router(pool: sqlx::PgPool) -> axum::Router {
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
    melodee_server::admin_diagnostics::router().with_state(AdminDiagnosticsState {
        admin,
        search: None,
        direct_play: None,
        global_stream_cap: 32,
        per_user_stream_cap: 0,
        started_at: OffsetDateTime::now_utc(),
    })
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn get_json(router: axum::Router, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = router
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    (status, body_json(response).await)
}

const ROUTES: &[&str] = &[
    "/api/admin/v1/diagnostics/selftest",
    "/api/admin/v1/diagnostics/settings",
    "/api/admin/v1/diagnostics/events",
    "/api/admin/v1/diagnostics/bundle",
];

#[tokio::test]
async fn diagnostics_routes_require_administrator() {
    let db = common::TestDb::create("runtime_diag_gate").await;
    let fixture = seed_users(&db).await;
    let router = diagnostics_router(db.pool.clone());
    // The live server installs the process-wide recent-event buffer during
    // logging setup, so `/diagnostics/events` answers 503 only in a process that
    // never installed one. Install it here so this gate does not depend on which
    // other test in the binary happened to run first.
    melodee_server::diagnostics::install(melodee_server::diagnostics::EVENT_CAPACITY);

    for path in ROUTES {
        let (status, body) = get_json(router.clone(), path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["code"], "unauthenticated", "{path}");

        let (status, body) = get_json(router.clone(), path, Some(&fixture.plain_token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(body["code"], "forbidden", "{path}");

        let (status, _) = get_json(router.clone(), path, Some(&fixture.admin_token)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }

    // The stream probe is the one POST route; it uses the same gate.
    let (status, body) = call_probe(router.clone(), "", Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    db.drop().await;
}

#[tokio::test]
async fn selftest_reports_the_documented_check_set() {
    let db = common::TestDb::create("runtime_diag_selftest").await;
    let fixture = seed_users(&db).await;
    let router = diagnostics_router(db.pool.clone());

    let (status, body) = get_json(
        router,
        "/api/admin/v1/diagnostics/selftest",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["generatedAt"].as_str().is_some());
    let overall = body["overall"].as_str().expect("overall");
    assert!(["pass", "warn", "fail"].contains(&overall), "{overall}");

    let checks = body["checks"].as_array().expect("checks array");
    let codes: Vec<&str> = checks
        .iter()
        .map(|check| check["code"].as_str().expect("code"))
        .collect();
    for expected in [
        "database",
        "migrations",
        "settings",
        "storage",
        "search",
        "streaming",
        "blacklist",
        "notifications",
    ] {
        assert!(
            codes.contains(&expected),
            "missing check {expected}: {codes:?}"
        );
    }
    let by_code = |code: &str| {
        checks
            .iter()
            .find(|check| check["code"] == code)
            .expect("check present")
    };
    // A freshly migrated test database must pass these two.
    assert_eq!(by_code("database")["status"], "pass");
    assert_eq!(by_code("migrations")["status"], "pass");
    // Direct play is intentionally unconfigured in this fixture, and that must
    // be reported as a failure with a hint rather than silently passing.
    assert_eq!(by_code("streaming")["status"], "fail");
    assert!(by_code("streaming")["hint"].as_str().is_some());
    db.drop().await;
}

#[tokio::test]
async fn effective_settings_are_redacted_and_explained() {
    let db = common::TestDb::create("runtime_diag_settings").await;
    let fixture = seed_users(&db).await;
    SettingsStore::new()
        .set(
            &db.pool,
            "email.smtpPassword",
            &serde_json::json!("super-secret-relay-password"),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("store secret");
    let router = diagnostics_router(db.pool.clone());

    let (status, body) = get_json(
        router,
        "/api/admin/v1/diagnostics/settings",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let total = body["total"].as_u64().expect("total");
    assert!(total >= 200, "expected the full registry, got {total}");
    assert!(body["secretCount"].as_u64().expect("secretCount") >= 1);
    let serialized = body.to_string();
    assert!(
        !serialized.contains("super-secret-relay-password"),
        "secret value leaked into the settings report"
    );

    let settings = body["settings"].as_array().expect("settings array");
    let password = settings
        .iter()
        .find(|setting| setting["key"] == "email.smtpPassword")
        .expect("smtp password row");
    assert_eq!(password["isSecret"], true);
    assert_eq!(password["source"], "database");
    assert_eq!(
        password["value"],
        melodee_persistence::SECRET_REDACTION_MARKER
    );

    // Inventoried keys expose the consumer or the inert rationale.
    let retention = settings
        .iter()
        .find(|setting| setting["key"] == "podcast.retention.keepUnplayedOnly")
        .expect("retention row");
    assert_eq!(retention["consumer"], "podcast_worker");
    assert_eq!(retention["kind"], "boolean");
    let retired = settings
        .iter()
        .find(|setting| setting["key"] == "partyMode.enabled")
        .expect("party row");
    assert!(retired["consumer"].is_null());
    assert!(retired["inertReason"].as_str().is_some());

    // Every row carries the stable shape, including value/source/kind.
    for setting in settings {
        assert!(setting["key"].as_str().is_some());
        assert!(setting["source"].as_str().is_some());
        assert!(setting.get("value").is_some());
    }
    db.drop().await;
}

#[tokio::test]
async fn events_report_is_bounded_and_installed() {
    let db = common::TestDb::create("runtime_diag_events").await;
    let fixture = seed_users(&db).await;
    let router = diagnostics_router(db.pool.clone());

    // The test harness installs no logging subscriber, so the process buffer is
    // created on demand by `install`. Emit one warning and read it back.
    let buffer = melodee_server::diagnostics::install(melodee_server::diagnostics::EVENT_CAPACITY);
    buffer.record_for_tests("WARN", "melodee_server::probe", "probe warning");

    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/diagnostics/events?limit=5",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["capacity"].as_u64().expect("capacity"),
        buffer.capacity() as u64
    );
    let events = body["events"].as_array().expect("events");
    assert!(
        events
            .iter()
            .any(|event| event["message"] == "probe warning")
    );

    // The limit is clamped rather than rejected, and never exceeds the cap.
    let (status, body) = get_json(
        router,
        "/api/admin/v1/diagnostics/events?limit=100000",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["events"].as_array().expect("events").len()
            <= body["capacity"].as_u64().expect("capacity") as usize
    );
    db.drop().await;
}

#[tokio::test]
async fn support_bundle_combines_identity_health_and_settings_summary() {
    let db = common::TestDb::create("runtime_diag_bundle").await;
    let fixture = seed_users(&db).await;
    let router = diagnostics_router(db.pool.clone());

    let (status, body) = get_json(
        router,
        "/api/admin/v1/diagnostics/bundle",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["server"]["name"], "melodee");
    assert!(body["server"]["version"].as_str().is_some());
    assert!(body["uptimeSeconds"].as_i64().expect("uptime") >= 0);
    assert!(body["selftest"]["checks"].as_array().expect("checks").len() >= 8);
    assert!(body["settings"]["total"].as_u64().expect("total") >= 200);
    assert_eq!(body["streaming"]["configured"], false);
    assert_eq!(body["streaming"]["perUserCap"], 0);
    assert!(body["recentEvents"].as_array().is_some());
    // No secret material anywhere in the bundle.
    let serialized = body.to_string();
    assert!(!serialized.contains("JWT_KEY"));
    assert!(!serialized.contains(JWT_KEY));
    db.drop().await;
}

fn diagnostics_router_with_playback(pool: sqlx::PgPool, root: &std::path::Path) -> axum::Router {
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
    let direct_play = melodee_server::native_stream::CanonicalAssetRoot::open(root)
        .ok()
        .and_then(|canonical| melodee_server::native_stream::DirectPlayService::new(canonical, 8));
    melodee_server::admin_diagnostics::router().with_state(AdminDiagnosticsState {
        admin,
        search: None,
        direct_play,
        global_stream_cap: 8,
        per_user_stream_cap: 2,
        started_at: OffsetDateTime::now_utc(),
    })
}

/// Seeds an active Release with one playable asset and returns the Track id.
async fn seed_track(db: &common::TestDb, relative_path: &str, byte_size: i64) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let release_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,
             display_artist, release_date, status, created_at, updated_at)
         VALUES ($1,$2,'Probe Release','probe release','Probe Release','Probe Artist',
             DATE '2020-01-01','active',$3,$3)",
    )
    .bind(release_id)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("release");
    let audio_identity = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO audio_identities (id, status, created_at, updated_at)
         VALUES ($1,'provisional',$2,$2)",
    )
    .bind(audio_identity)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("audio identity");
    let media_asset_id = Uuid::now_v7();
    let track_id = Uuid::now_v7();
    let track_public_id = Uuid::now_v7();
    // The track→playable-asset foreign key is deferred: the media asset and
    // the playable link must be inserted in the same transaction.
    let mut tx = db.pool.begin().await.expect("track transaction");
    sqlx::query(
        "INSERT INTO tracks (id, public_api_id, release_id, playable_media_asset_id,
             audio_identity_id, ordinal, title, normalized_title, sort_title,
             track_number, duration_ms, source_path, created_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,1,'Probe Track','probe track','Probe Track',
             1,1000,$6,$7,$7)",
    )
    .bind(track_id)
    .bind(track_public_id)
    .bind(release_id)
    .bind(media_asset_id)
    .bind(audio_identity)
    .bind(relative_path)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("track");
    sqlx::query(
        "INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size,
             mime_type, container, codec, sample_rate_hz, channels, bitrate_bps,
             duration_ms, created_at)
         VALUES ($1,$2,$3,$4,'audio/flac','flac','flac',44100,2,1000000,1000,$5)",
    )
    .bind(media_asset_id)
    .bind(relative_path)
    .bind("0".repeat(64))
    .bind(byte_size)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("media asset");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
         VALUES ($1,$2,'playable',1,$3)",
    )
    .bind(track_id)
    .bind(media_asset_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("track media link");
    tx.commit().await.expect("track commit");
    track_public_id
}

#[tokio::test]
async fn stream_probe_diagnoses_track_and_asset_state() {
    let db = common::TestDb::create("runtime_diag_stream_probe").await;
    let fixture = seed_users(&db).await;
    let storage = tempfile::tempdir().expect("canonical root");
    std::fs::create_dir_all(storage.path().join("releases/01")).expect("asset dir");
    std::fs::write(
        storage.path().join("releases/01/probe.flac"),
        b"probe-bytes",
    )
    .expect("asset file");
    let track_id = seed_track(&db, "releases/01/probe.flac", 11).await;
    let app = diagnostics_router_with_playback(db.pool.clone(), storage.path());

    // Healthy track.
    let (status, body) = call_probe(app.clone(), &fixture.admin_token, track_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["verdict"], "ok");
    assert_eq!(body["filePresent"], true);
    assert_eq!(body["capacity"], 8);
    assert_eq!(body["perUserCap"], 2);

    // Unknown track.
    let (status, body) = call_probe(app.clone(), &fixture.admin_token, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["verdict"], "track_not_found");
    assert!(body["hint"].as_str().is_some());

    // Size mismatch: the recorded byte size differs from the file on disk.
    sqlx::query("UPDATE media_assets SET byte_size = 99999 WHERE canonical_relative_path=$1")
        .bind("releases/01/probe.flac")
        .execute(&db.pool)
        .await
        .expect("size mismatch");
    let (status, body) = call_probe(app, &fixture.admin_token, track_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["verdict"], "asset_file_missing");
    assert_eq!(body["filePresent"], false);
    db.drop().await;
}

#[tokio::test]
async fn stream_probe_reports_disabled_direct_play() {
    let db = common::TestDb::create("runtime_diag_stream_probe_disabled").await;
    let fixture = seed_users(&db).await;
    let track_id = seed_track(&db, "releases/01/missing.flac", 5).await;
    let app = diagnostics_router(db.pool.clone());
    let (status, body) = call_probe(app, &fixture.admin_token, track_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["verdict"], "direct_play_disabled");
    assert_eq!(body["directPlayConfigured"], false);
    assert!(body["hint"].as_str().is_some());
    db.drop().await;
}

async fn call_probe(app: axum::Router, token: &str, track_id: Uuid) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/admin/v1/diagnostics/stream-probe")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"trackId": track_id})).expect("serialize"),
        ))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
