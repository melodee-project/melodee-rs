//! Bulk purge API for failed pre-catalog work items (ADR-level destructive
//! operator action): administrator gate, explicit confirmation, storage
//! removal below configured roots, processing-log deletion, and the surviving
//! purge audit row.

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
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_purge::{AdminPurgeState, PurgeSourceRoot, router};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-purge-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("purge-admin", true, admin_public),
        ("purge-plain", false, plain_public),
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

fn app(
    pool: sqlx::PgPool,
    source_root: std::path::PathBuf,
    staging_root: std::path::PathBuf,
    work_root: std::path::PathBuf,
) -> axum::Router {
    app_with_canonical(pool, source_root, staging_root, work_root, None)
}

fn app_with_canonical(
    pool: sqlx::PgPool,
    source_root: std::path::PathBuf,
    staging_root: std::path::PathBuf,
    work_root: std::path::PathBuf,
    canonical_root: Option<std::path::PathBuf>,
) -> axum::Router {
    router().with_state(AdminPurgeState::new(
        AdminAccessState {
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
        },
        vec![PurgeSourceRoot {
            id: None,
            root: source_root,
        }],
        Some(staging_root),
        work_root,
        canonical_root,
    ))
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

async fn insert_failed_item(db: &TestDb, current_path: &str) -> (Uuid, Uuid) {
    let id = Uuid::now_v7();
    let api_key = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, attempt_count,
            correlation_id, discovered_at, updated_at, created_at
         ) VALUES ($1,$2,$3,$3,'failed',5,$4,$5,$5,$5)",
    )
    .bind(id)
    .bind(api_key)
    .bind(current_path)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert failed work item");
    sqlx::query(
        "INSERT INTO release_work_item_events (
            work_item_id, event_type, from_state, to_state, correlation_id, created_at
         ) VALUES ($1,'failed','normalizing','failed',$2,$3)",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert event");
    (id, api_key)
}

#[tokio::test]
async fn purge_requires_administrator_and_explicit_confirmation() {
    let db = TestDb::create("admin_purge_gate").await;
    let fixture = seed_users(&db).await;
    let root = tempfile::tempdir().expect("temp root");
    let app = app(
        db.pool.clone(),
        root.path().join("inbound"),
        root.path().join("staging"),
        root.path().join("work"),
    );
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [], "reason": "x", "confirm": true})),
        None,
    )
    .await;
    assert_eq!(StatusCode::UNAUTHORIZED, status);
    let plain = token_for(fixture.plain_public, "purge-plain");
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [], "reason": "x", "confirm": true})),
        Some(&plain),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, status);
    let admin = token_for(fixture.admin_public, "purge-admin");
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [], "reason": "x", "confirm": false})),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!(body["code"], "confirmation_required");
    db.drop().await;
}

#[tokio::test]
async fn purge_removes_failed_storage_event_log_and_keeps_an_audit_row() {
    let db = TestDb::create("admin_purge_failed").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    let (id, _) = insert_failed_item(&db, "Broken Release").await;
    let staging_relative = format!("releases/{}/source", id.simple());
    std::fs::create_dir_all(staging.join(&staging_relative)).expect("staging");
    std::fs::write(staging.join(&staging_relative).join("track.flac"), b"audio").expect("file");
    std::fs::create_dir_all(work.join(id.simple().to_string()).join("attempts")).expect("work");
    sqlx::query(
        "INSERT INTO release_work_item_staging_handoffs (
            work_item_id, staging_relative_path, source_generation, status,
            method, tree_hash, intended_at, staged_at
         ) VALUES ($1,$2,'gen','staged','rename',$3,$4,$4)",
    )
    .bind(id)
    .bind(&staging_relative)
    .bind("a".repeat(64))
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("handoff");

    let app = app(
        db.pool.clone(),
        inbound.clone(),
        staging.clone(),
        work.clone(),
    );
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({
            "workItemIds": [id],
            "reason": "discard corrupted failed import",
            "confirm": true
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert_eq!(body["deleted"][0]["id"], id.to_string());
    assert_eq!(body["failed"], json!([]));
    assert!(!staging.join(&staging_relative).exists());
    assert!(!work.join(id.simple().to_string()).exists());
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("count work item");
    assert_eq!(0, remaining);
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_work_item_events WHERE work_item_id=$1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .expect("count events");
    assert_eq!(0, events);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_audit_events
          WHERE entity_id=$1 AND event_type='work_item_purged'",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("count audit");
    assert_eq!(1, audit);
    db.drop().await;
}

#[tokio::test]
async fn purge_accepts_a_non_failed_pre_catalog_item() {
    let db = TestDb::create("admin_purge_state").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, attempt_count,
            correlation_id, discovered_at, updated_at, created_at
         ) VALUES ($1,$2,'Ready Release','Ready Release','ready',0,$3,$4,$4,$4)",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("insert ready item");
    let app = app(
        db.pool.clone(),
        root.path().join("inbound"),
        root.path().join("staging"),
        root.path().join("work"),
    );
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [id], "reason": "cleanup", "confirm": true})),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert_eq!(body["deleted"][0]["id"], json!(id));
    assert_eq!(body["failed"], json!([]));
    let survives: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("count work item");
    assert_eq!(survives, 0);
    db.drop().await;
}

#[tokio::test]
async fn purge_removes_inbound_source_when_no_staging_handoff_exists() {
    let db = TestDb::create("admin_purge_inbound").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    std::fs::create_dir_all(inbound.join("Never Acquired")).expect("inbound");
    let (id, _) = insert_failed_item(&db, "Never Acquired").await;
    let app = app(
        db.pool.clone(),
        inbound.clone(),
        root.path().join("staging"),
        root.path().join("work"),
    );
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [id], "confirm": true})),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert_eq!(body["deleted"][0]["paths"]["inbound"], "Never Acquired");
    assert!(!inbound.join("Never Acquired").exists());
    let audit_reason: String = sqlx::query_scalar(
        "SELECT reason FROM catalog_audit_events
          WHERE entity_id=$1 AND event_type='work_item_purged'",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("purge audit reason");
    assert!(audit_reason.contains("administrator confirmed bulk purge"));
    db.drop().await;
}

#[tokio::test]
async fn purge_refuses_ambiguous_legacy_and_reviewed_paths_before_any_deletion() {
    let db = TestDb::create("admin_purge_legacy_fence").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    std::fs::create_dir_all(inbound.join("bad-�")).expect("literal candidate");
    std::fs::write(inbound.join("bad-�/track.flac"), b"new literal").expect("file");
    let (legacy, _) = insert_failed_item(&db, "bad-�").await;
    sqlx::query("UPDATE release_work_items SET discovery_path_format=0 WHERE id=$1")
        .bind(legacy)
        .execute(&db.pool)
        .await
        .expect("legacy version");
    let (reviewed, _) = insert_failed_item(&db, "bad-�").await;
    let staged_relative = format!("releases/{reviewed}/source");
    std::fs::create_dir_all(staging.join(&staged_relative)).expect("staging");
    std::fs::write(
        staging.join(&staged_relative).join("track.flac"),
        b"reviewed",
    )
    .expect("staged file");
    std::fs::create_dir_all(work.join(legacy.simple().to_string())).expect("legacy work");
    std::fs::create_dir_all(work.join(reviewed.simple().to_string())).expect("reviewed work");
    sqlx::query("INSERT INTO release_work_item_staging_handoffs (work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES ($1,$2,'gen','staged','rename',$3,clock_timestamp(),clock_timestamp())")
        .bind(reviewed).bind(&staged_relative).bind("a".repeat(64)).execute(&db.pool).await.expect("receipt");
    sqlx::query("INSERT INTO ingest_legacy_path_resolutions (legacy_work_item_id,reviewed_work_item_id,receipt_tree_hash,actor,reason,resolved_at) VALUES ($1,$2,$3,'operator','reviewed',clock_timestamp())")
        .bind(legacy).bind(reviewed).bind("a".repeat(64)).execute(&db.pool).await.expect("audit");
    let (status, body) = call(
        app(
            db.pool.clone(),
            inbound.clone(),
            staging.clone(),
            work.clone(),
        ),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [legacy, reviewed], "reason":"reviewed", "confirm":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"].as_array().expect("failures").len(), 2);
    assert_eq!(
        std::fs::read(inbound.join("bad-�/track.flac")).expect("inbound"),
        b"new literal"
    );
    assert_eq!(
        std::fs::read(staging.join(staged_relative).join("track.flac")).expect("staged"),
        b"reviewed"
    );
    assert!(work.join(legacy.simple().to_string()).is_dir());
    assert!(work.join(reviewed.simple().to_string()).is_dir());
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ingest_legacy_path_resolutions WHERE legacy_work_item_id=$1",
    )
    .bind(legacy)
    .fetch_one(&db.pool)
    .await
    .expect("audit retained");
    assert_eq!(retained, 1);
    db.drop().await;
}

#[tokio::test]
async fn processing_queue_delete_hard_deletes_a_cataloged_release() {
    let db = TestDb::create("admin_delete_cataloged").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let cataloged = phase5_common::seed_cataloged_release(
        &db,
        8,
        CatalogFixturePresentation {
            release_title: "Queue Deleted Fixture",
            release_artist: "Queue Deleted Artist",
            track_title: "Queue Deleted Track",
            track_artist: "Queue Deleted Artist",
            release_date: "2021-01-01",
        },
    )
    .await;
    let work_item_id = cataloged.work_item_id();
    let release_internal = cataloged.release_id().into_uuid();
    let (legacy, _) = insert_failed_item(&db, "historical-�").await;
    sqlx::query("INSERT INTO ingest_legacy_path_resolutions (legacy_work_item_id,reviewed_work_item_id,receipt_tree_hash,actor,reason,resolved_at) VALUES ($1,$2,$3,'operator','reviewed',clock_timestamp())")
        .bind(legacy).bind(work_item_id).bind("a".repeat(64)).execute(&db.pool).await.expect("historical audit");
    let app = app_with_canonical(
        db.pool.clone(),
        root.path().join("inbound"),
        root.path().join("staging"),
        root.path().join("work"),
        Some(root.path().join("canonical")),
    );
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/delete",
        Some(json!({"workItemIds": [work_item_id], "confirm": true})),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert_eq!(body["deleted"][0]["id"], json!(work_item_id));
    assert_eq!(body["failed"], json!([]));
    for (table, sql) in [
        (
            "work item",
            "SELECT count(*) FROM release_work_items WHERE id=$1",
        ),
        ("release", "SELECT count(*) FROM releases WHERE id=$1"),
        ("track", "SELECT count(*) FROM tracks WHERE release_id=$1"),
    ] {
        let remaining: i64 = match table {
            "work item" => sqlx::query_scalar(sql)
                .bind(work_item_id)
                .fetch_one(&db.pool)
                .await
                .expect("count work item"),
            _ => sqlx::query_scalar(sql)
                .bind(release_internal)
                .fetch_one(&db.pool)
                .await
                .expect("count release"),
        };
        assert_eq!(remaining, 0, "{table} must be hard-deleted");
    }
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_legacy_path_resolutions WHERE legacy_work_item_id=$1 AND reviewed_work_item_id=$2")
        .bind(legacy).bind(work_item_id).fetch_one(&db.pool).await.expect("audit retained");
    assert_eq!(
        retained, 1,
        "non-owning audit survives catalog hard deletion"
    );
    db.drop().await;
}

#[tokio::test]
async fn purge_refuses_a_cataloged_release_even_when_marked_failed() {
    let db = TestDb::create("admin_purge_cataloged").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let cataloged = phase5_common::seed_cataloged_release(
        &db,
        7,
        CatalogFixturePresentation {
            release_title: "Cataloged Failed Fixture",
            release_artist: "Cataloged Artist",
            track_title: "Track",
            track_artist: "Cataloged Artist",
            release_date: "2020-01-01",
        },
    )
    .await;
    let work_item_id = cataloged.work_item_id();
    sqlx::query(
        "UPDATE release_work_items SET state='failed', completed_at=NULL, next_attempt_at=NULL
          WHERE id=$1",
    )
    .bind(work_item_id)
    .execute(&db.pool)
    .await
    .expect("mark failed");

    let app = app(
        db.pool.clone(),
        root.path().join("inbound"),
        root.path().join("staging"),
        root.path().join("work"),
    );
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds": [work_item_id], "reason": "cleanup", "confirm": true})),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"][0]["code"], "cataloged");
    let survives: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(work_item_id)
        .fetch_one(&db.pool)
        .await
        .expect("work item survives");
    assert_eq!(1, survives);
    let release_survives: i64 = sqlx::query_scalar("SELECT count(*) FROM releases WHERE id=$1")
        .bind(cataloged.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("release survives");
    assert_eq!(1, release_survives);
    db.drop().await;
}

#[tokio::test]
async fn no_handoff_purge_refuses_before_removing_any_storage() {
    let db = TestDb::create("admin_purge_source_custody_hold").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    std::fs::create_dir_all(inbound.join("Never Acquired")).expect("source");
    std::fs::write(inbound.join("Never Acquired/track.flac"), b"producer bytes")
        .expect("source bytes");
    let (id, _) = insert_failed_item(&db, "Never Acquired").await;
    std::fs::create_dir_all(work.join(id.simple().to_string())).expect("work area");
    std::fs::write(
        work.join(id.simple().to_string()).join("keep"),
        b"work bytes",
    )
    .expect("work bytes");
    let (status, body) = call(
        app(db.pool.clone(), inbound.clone(), staging, work.clone()),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds":[id],"confirm":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"][0]["code"], "source_custody_required");
    assert_eq!(
        std::fs::read(inbound.join("Never Acquired/track.flac")).expect("source retained"),
        b"producer bytes"
    );
    assert_eq!(
        std::fs::read(work.join(id.simple().to_string()).join("keep")).expect("work retained"),
        b"work bytes"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("item");
    assert_eq!(count, 1);
    db.drop().await;
}

#[tokio::test]
async fn abandoned_purge_guard_blocks_retry_before_any_filesystem_deletion() {
    let db = TestDb::create("admin_purge_abandoned_guard").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    let (id, _) = insert_failed_item(&db, "Candidate").await;
    let relative = format!("releases/{id}/source");
    std::fs::create_dir_all(staging.join(&relative)).expect("staging");
    std::fs::write(staging.join(&relative).join("track"), b"owned bytes").expect("staged bytes");
    std::fs::create_dir_all(work.join(id.simple().to_string())).expect("work area");
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES($1,$2,'gen','staged','rename',$3,clock_timestamp(),clock_timestamp())")
        .bind(id).bind(&relative).bind("a".repeat(64)).execute(&db.pool).await.expect("handoff");
    let guard = melodee_persistence::WorkItemStore
        .begin_pre_catalog_purge(&db.pool, id, &fixture.admin_public.to_string())
        .await
        .expect("reserve");
    drop(guard); // Simulate process loss before any filesystem operation.
    let (status, body) = call(
        app(db.pool.clone(), inbound, staging.clone(), work.clone()),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds":[id],"confirm":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"][0]["code"], "purge_guard_failed");
    assert_eq!(
        std::fs::read(staging.join(relative).join("track")).expect("stage retained"),
        b"owned bytes"
    );
    assert!(work.join(id.simple().to_string()).exists());
    let guards: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_occurrence_purge_guards WHERE work_item_id=$1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("guard");
    assert_eq!(guards, 1);
    db.drop().await;
}

#[tokio::test]
async fn unsafe_staging_path_is_rejected_before_guard_or_filesystem_changes() {
    let db = TestDb::create("admin_purge_preflight_unsafe_stage").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    let (id, _) = insert_failed_item(&db, "Candidate").await;
    std::fs::create_dir_all(work.join(id.simple().to_string())).expect("work area");
    std::fs::write(
        work.join(id.simple().to_string()).join("keep"),
        b"work bytes",
    )
    .expect("work bytes");
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES($1,'../outside','gen','staged','rename',$2,clock_timestamp(),clock_timestamp())")
        .bind(id).bind("a".repeat(64)).execute(&db.pool).await.expect("handoff");
    let (status, body) = call(
        app(db.pool.clone(), inbound, staging, work.clone()),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds":[id],"confirm":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"][0]["code"], "unsafe_path");
    assert_eq!(
        std::fs::read(work.join(id.simple().to_string()).join("keep")).expect("work retained"),
        b"work bytes"
    );
    let guards: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_occurrence_purge_guards WHERE work_item_id=$1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("guard count");
    assert_eq!(
        guards, 0,
        "a deterministic preflight failure must not strand a guard"
    );
    db.drop().await;
}

#[tokio::test]
async fn retained_occurrence_blocks_purge_after_original_item_is_deleted() {
    let db = TestDb::create("admin_purge_retained_after_original_deletion").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "purge-admin");
    let root = tempfile::tempdir().expect("temp root");
    let inbound = root.path().join("inbound");
    let staging = root.path().join("staging");
    let work = root.path().join("work");
    std::fs::create_dir_all(inbound.join("Candidate")).expect("source");
    std::fs::write(inbound.join("Candidate/track"), b"producer bytes").expect("source bytes");
    let (original, _) = insert_failed_item(&db, "Candidate").await;
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'Candidate','generation',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(original).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("retained obligation");
    sqlx::query("DELETE FROM release_work_items WHERE id=$1")
        .bind(original)
        .execute(&db.pool)
        .await
        .expect("remove original row");
    // Model a preexisting row/handoff from before occurrence fencing. Normal
    // discovery and handoff APIs now prevent creating this conflicting state.
    let (fresh, _) = insert_failed_item(&db, "Candidate").await;
    let relative = format!("releases/{fresh}/source");
    std::fs::create_dir_all(staging.join(&relative)).expect("stage");
    std::fs::write(staging.join(&relative).join("keep"), b"stage bytes").expect("stage bytes");
    std::fs::create_dir_all(work.join(fresh.simple().to_string())).expect("work");
    std::fs::write(
        work.join(fresh.simple().to_string()).join("keep"),
        b"work bytes",
    )
    .expect("work bytes");
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES($1,$2,'gen','staged','rename',$3,clock_timestamp(),clock_timestamp())")
        .bind(fresh).bind(&relative).bind("a".repeat(64)).execute(&db.pool).await.expect("old handoff");
    let (status, body) = call(
        app(
            db.pool.clone(),
            inbound.clone(),
            staging.clone(),
            work.clone(),
        ),
        "POST",
        "/api/admin/v1/ingest/work-items/purge",
        Some(json!({"workItemIds":[fresh],"confirm":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], json!([]));
    assert_eq!(body["failed"][0]["code"], "source_retained");
    assert_eq!(
        std::fs::read(inbound.join("Candidate/track")).expect("source retained"),
        b"producer bytes"
    );
    assert_eq!(
        std::fs::read(staging.join(relative).join("keep")).expect("stage retained"),
        b"stage bytes"
    );
    assert_eq!(
        std::fs::read(work.join(fresh.simple().to_string()).join("keep")).expect("work retained"),
        b"work bytes"
    );
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1 AND status='active'",
    )
    .bind(original)
    .fetch_one(&db.pool)
    .await
    .expect("obligation remains");
    assert_eq!(retained, 1);
    db.drop().await;
}
