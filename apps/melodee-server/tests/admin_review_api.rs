//! Release Review API integration tests: the administrator
//! gate, the review queue with per-state counts, the full review detail
//! (manifest summary, classification, catalog comparison, durable decision),
//! audited accept/quarantine/delete decisions, reprocess, and the
//! path-safe pre-publication track preview with Range support.

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
use common::{TestDb, seed_ready_item, test_config};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_domain::{
    Disposition, FailureClass, IngestionState, PolicyRuleSetId, TransitionKind, WorkErrorCode,
};
use melodee_persistence::{
    AuthStore, CatalogReleaseSort, CatalogStore, CatalogTrackSort, NativeUserImport, PolicyStore,
    WorkItemStore, WorkerId,
};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_review::{AdminReviewState, PreviewSourceRoot};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-review-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp12-admin", true, admin_public),
        ("wp12-plain", false, plain_public),
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

fn build_app(
    pool: sqlx::PgPool,
    mut source_roots: Vec<PreviewSourceRoot>,
    work_area: &std::path::Path,
) -> axum::Router {
    let canonical_root = work_area.join("library");
    std::fs::create_dir_all(&canonical_root).expect("canonical test root");
    if source_roots.is_empty() {
        let staging_root = work_area.join("staging");
        std::fs::create_dir_all(&staging_root).expect("staging test root");
        source_roots.push(PreviewSourceRoot {
            id: None,
            root: staging_root,
        });
    }
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
    melodee_server::admin_review::router().with_state(AdminReviewState::new(
        admin,
        source_roots,
        work_area.to_path_buf(),
        Some(canonical_root),
    ))
}

struct HttpResponse {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> HttpResponse {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
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
    let headers = response.headers().clone();
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
    HttpResponse {
        status,
        headers,
        body: json,
    }
}

/// Seeds one cataloged Release whose policy evaluation parked the work item
/// in `needs_review` with a pending Review decision (the target state).
async fn seed_pending_review(db: &TestDb, ordinal: u32) -> phase5_common::CatalogedPolicyFixture {
    let fixture = phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: "Review API Fixture",
            release_artist: "Review Artist",
            track_title: "Preview Track",
            track_artist: "Review Artist",
            release_date: "2006-07-08",
        },
    )
    .await;
    let store = PolicyStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim review fixture");
    let claim = claims.pop().expect("one review claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load review context");
    let rule_set = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist review rules");
    let committed = store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("commit pending review decision");
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert_eq!(committed.decision.disposition, Disposition::Review);
    fixture
}

/// Seeds one work item parked in `failed` through the legal worker path.
async fn seed_failed_item(db: &TestDb, source_path: &str) -> Uuid {
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, source_path).await;
    let worker = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim failure fixture");
    assert!(claimed.iter().any(|item| item.id == id));
    store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("start inspection");
    store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "fixture failure",
            FailureClass::Permanent,
            &config.retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("record permanent failure");
    id
}

#[tokio::test]
async fn review_routes_require_administrator() {
    let db = TestDb::create("wp12_review_gate").await;
    let fixture = seed_users(&db).await;
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());

    let response = call(
        app.clone(),
        "GET",
        "/api/admin/v1/review/queue",
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let plain = token_for(fixture.plain_public, "wp12-plain");
    let response = call(
        app.clone(),
        "GET",
        "/api/admin/v1/review/queue",
        None,
        Some(&plain),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    let admin = token_for(fixture.admin_public, "wp12-admin");
    let response = call(
        app,
        "GET",
        "/api/admin/v1/review/queue",
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);

    db.drop().await;
}

#[tokio::test]
async fn review_queue_lists_review_states_with_counts_and_filters() {
    let db = TestDb::create("wp12_review_queue").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp12-admin");
    let review_fixture = seed_pending_review(&db, 1).await;
    let failed_id = seed_failed_item(&db, "wp12/review-queue/failed").await;
    let _ignored_ready = seed_ready_item(&WorkItemStore, &db.pool, "wp12/review-queue/ready").await;
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());

    let response = call(
        app.clone(),
        "GET",
        "/api/admin/v1/review/queue",
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["total"], 2);
    let items = response.body["items"].as_array().expect("items");
    let states: Vec<&str> = items
        .iter()
        .map(|item| item["state"].as_str().expect("state"))
        .collect();
    assert!(states.contains(&"needs_review"));
    assert!(states.contains(&"failed"));
    // The cataloged fixture row carries its Release binding.
    let review_row = items
        .iter()
        .find(|item| item["state"] == "needs_review")
        .expect("needs_review row");
    assert_eq!(
        review_row["releaseId"].as_str().expect("releaseId"),
        review_fixture.release_id().into_uuid().to_string()
    );
    let counts = response.body["countsByState"].as_array().expect("counts");
    let count_for = |state: &str| -> i64 {
        counts
            .iter()
            .find(|count| count["state"] == state)
            .map(|count| count["count"].as_i64().expect("count"))
            .unwrap_or(0)
    };
    assert_eq!(count_for("needs_review"), 1);
    assert_eq!(count_for("failed"), 1);
    assert_eq!(count_for("quarantined"), 0);

    // State filter narrows the listing but keeps full counts.
    let response = call(
        app.clone(),
        "GET",
        "/api/admin/v1/review/queue?state=failed",
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["total"], 1);
    assert_eq!(
        response.body["items"][0]["id"].as_str().expect("id"),
        failed_id.to_string()
    );
    assert_eq!(count_for("needs_review"), 1);

    // A non-review-able state is rejected.
    let response = call(
        app.clone(),
        "GET",
        "/api/admin/v1/review/queue?state=ready",
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["code"], "state_not_reviewable");

    let response = call(
        app,
        "GET",
        "/api/admin/v1/review/queue?state=bogus",
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["code"], "unknown_state");

    db.drop().await;
}

#[tokio::test]
async fn review_detail_returns_manifest_classification_comparison_and_decision() {
    let db = TestDb::create("wp12_review_detail").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp12-admin");
    let review = seed_pending_review(&db, 2).await;
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());
    let path = format!("/api/admin/v1/review/{}", review.work_item_id());

    let response = call(app.clone(), "GET", &path, None, Some(&admin), &[]).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let body = &response.body;
    assert_eq!(body["workItem"]["state"], "needs_review");
    assert_eq!(
        body["releaseId"].as_str().expect("releaseId"),
        review.release_id().into_uuid().to_string()
    );

    let manifest = &body["manifest"];
    assert_eq!(manifest["releaseTitle"], "Review API Fixture");
    assert_eq!(manifest["tracks"].as_array().expect("tracks").len(), 1);
    assert_eq!(manifest["tracks"][0]["index"], 0);
    assert_eq!(
        manifest["tracks"][0]["filePath"],
        "disc-01/02 - Preview Track.flac"
    );
    assert_eq!(manifest["tracks"][0]["fingerprintStatus"], "captured");

    let classification = &body["classification"];
    assert_eq!(
        classification["traits"].as_array().expect("traits"),
        &vec![serde_json::json!("unknown")]
    );
    assert_eq!(
        classification["classifierVersion"],
        "melodee.policy.classifier-v1"
    );

    let comparison = &body["catalogComparison"];
    assert_eq!(comparison["uncertain"], 1);
    assert_eq!(comparison["known"], 0);
    assert_eq!(comparison["new"], 0);
    assert_eq!(comparison["alternate"], 0);
    assert_eq!(comparison["totalTracks"], 1);
    assert_eq!(comparison["tracks"].as_array().expect("tracks").len(), 1);

    let decision = &body["decision"];
    assert_eq!(decision["disposition"], "review");
    assert_eq!(decision["completionState"], "pending");
    assert!(
        !body["timeline"].as_array().expect("timeline").is_empty(),
        "timeline must carry the durable work-item history"
    );

    // Unknown work item -> 404 envelope.
    let response = call(
        app,
        "GET",
        &format!("/api/admin/v1/review/{}", Uuid::now_v7()),
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body["code"], "not_found");

    db.drop().await;
}

#[tokio::test]
async fn review_decision_accept_and_delete_flows() {
    let db = TestDb::create("wp12_review_decision").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp12-admin");
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());

    // Accept: applies the policy override and resumes the pipeline.
    let accept_fixture = seed_pending_review(&db, 3).await;
    let path = format!(
        "/api/admin/v1/review/{}/decision",
        accept_fixture.work_item_id()
    );
    let response = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"action": "accept", "reason": "Verified in review"})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["workItem"]["state"], "indexing");
    assert_eq!(response.body["decision"]["disposition"], "accept");
    assert_eq!(response.body["decision"]["completionState"], "completed");
    assert_eq!(
        response.body["decision"]["overrideReason"],
        "Verified in review"
    );
    assert_eq!(response.body["pendingExecution"], false);

    // Rejecting an already-resolved item conflicts.
    let response = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"action": "quarantine", "reason": "second pass"})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(response.body["code"], "invalid_state");

    // Delete requires the explicit destructive acknowledgement.
    let delete_fixture = seed_pending_review(&db, 4).await;
    let publication = WorkItemStore
        .get_publication(&db.pool, delete_fixture.work_item_id())
        .await
        .expect("read deletion publication")
        .expect("deletion publication exists");
    let canonical_release = work_area
        .path()
        .join("library")
        .join(&publication.plan.target_relative_path);
    std::fs::create_dir_all(&canonical_release).expect("canonical Release tree");
    std::fs::write(canonical_release.join("track.mp3"), b"canonical").expect("canonical media");
    let staging_relative = format!("releases/{}", delete_fixture.work_item_id());
    let staging_release = work_area.path().join("staging").join(&staging_relative);
    std::fs::create_dir_all(&staging_release).expect("staged Release tree");
    std::fs::write(staging_release.join("track.flac"), b"source").expect("staged media");
    sqlx::query(
        "INSERT INTO release_work_item_staging_handoffs (
            work_item_id, staging_relative_path, source_generation, status,
            method, tree_hash, intended_at, staged_at, inbound_removed_at
         ) VALUES ($1,$2,'test-generation','inbound_removed','verified_copy',
                   $3,$4,$4,$4)",
    )
    .bind(delete_fixture.work_item_id())
    .bind(&staging_relative)
    .bind("a".repeat(64))
    .bind(time::OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("seed staging handoff");
    let delete_path = format!(
        "/api/admin/v1/review/{}/decision",
        delete_fixture.work_item_id()
    );
    let response = call(
        app.clone(),
        "POST",
        &delete_path,
        Some(serde_json::json!({"action": "delete", "reason": "not wanted"})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["code"], "override_required");

    let response = call(
        app.clone(),
        "POST",
        &delete_path,
        Some(serde_json::json!({
            "action": "delete",
            "reason": "confirmed destructive",
            "override": true
        })),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["decision"]["disposition"], "delete");
    assert_eq!(response.body["pendingExecution"], false);
    assert_eq!(response.body["workItem"]["state"], "deleted");
    assert!(!canonical_release.exists(), "canonical media is removed");
    assert!(
        !staging_release.exists(),
        "retained source media is removed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id = $1")
            .bind(delete_fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read deleted Release tombstone"),
        "deleted"
    );
    let deleted_release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(delete_fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read deleted Release public id");
    let (visible_releases, release_total) = CatalogStore
        .list_catalog_releases(&db.pool, None, None, 50, 0, CatalogReleaseSort::TitleAsc)
        .await
        .expect("browse Releases after deletion");
    assert_eq!(release_total, 1, "the accepted fixture remains visible");
    assert!(
        visible_releases
            .iter()
            .all(|release| release.public_api_id != deleted_release_public_id)
    );
    let (visible_tracks, track_total) = CatalogStore
        .list_catalog_tracks(&db.pool, None, None, 50, 0, CatalogTrackSort::TitleAsc)
        .await
        .expect("browse Tracks after deletion");
    assert_eq!(track_total, 1, "the accepted fixture Track remains visible");
    assert!(
        visible_tracks
            .iter()
            .all(|track| track.release_public_api_id != deleted_release_public_id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_audit_events
              WHERE entity_id = $1 AND event_type = 'release_deleted'",
        )
        .bind(delete_fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read deletion audit"),
        1
    );

    // Blank reason and unknown action are rejected before any state reads.
    let response = call(
        app.clone(),
        "POST",
        &delete_path,
        Some(serde_json::json!({"action": "accept", "reason": "   "})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["code"], "invalid_reason");
    let response = call(
        app,
        "POST",
        &delete_path,
        Some(serde_json::json!({"action": "launch", "reason": "nope"})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["code"], "unknown_action");

    db.drop().await;
}

#[tokio::test]
async fn review_reprocess_moves_failed_to_ready_only() {
    let db = TestDb::create("wp12_review_reprocess").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp12-admin");
    let failed_id = seed_failed_item(&db, "wp12/reprocess/failed").await;
    let review = seed_pending_review(&db, 5).await;
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());

    let response = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/review/{failed_id}/reprocess"),
        Some(serde_json::json!({"reason": "transient scanner failure"})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["state"], "ready");

    // needs_review is awaiting a decision, not a reprocess.
    let response = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/review/{}/reprocess", review.work_item_id()),
        Some(serde_json::json!({})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(response.body["code"], "not_reprocessable");

    let response = call(
        app,
        "POST",
        &format!("/api/admin/v1/review/{}/reprocess", Uuid::now_v7()),
        Some(serde_json::json!({})),
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    db.drop().await;
}

#[tokio::test]
async fn track_preview_streams_from_staging_with_range_and_no_store() {
    let db = TestDb::create("wp12_review_preview").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp12-admin");
    let review = seed_pending_review(&db, 6).await;

    // Materialize the observed source file under its deterministic staging root.
    let source_root = tempfile::tempdir().expect("staging root");
    let release_dir = source_root
        .path()
        .join("releases")
        .join(review.work_item_id().to_string())
        .join("source");
    std::fs::create_dir_all(release_dir.join("disc-01")).expect("release dir");
    let track_path = release_dir.join("disc-01/06 - Preview Track.flac");
    let bytes = b"fLaC-fixture-preview-bytes-0123456789".to_vec();
    std::fs::write(&track_path, &bytes).expect("write fixture audio");
    let work_area = tempfile::tempdir().expect("work area");

    let app = build_app(
        db.pool.clone(),
        vec![PreviewSourceRoot {
            id: None,
            root: source_root.path().to_path_buf(),
        }],
        work_area.path(),
    );
    let preview_path = format!(
        "/api/admin/v1/review/{}/tracks/0/preview",
        review.work_item_id()
    );

    // Full body.
    let response = call(app.clone(), "GET", &preview_path, None, Some(&admin), &[]).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(
        response
            .headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    // Ranged body.
    let response = call(
        app.clone(),
        "GET",
        &preview_path,
        None,
        Some(&admin),
        &[("range", "bytes=0-3")],
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::PARTIAL_CONTENT,
        "{}",
        response.body
    );
    assert_eq!(
        response
            .headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    // Out-of-range track index -> 404 preview_unavailable.
    let response = call(
        app.clone(),
        "GET",
        &format!(
            "/api/admin/v1/review/{}/tracks/9/preview",
            review.work_item_id()
        ),
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body["code"], "preview_unavailable");

    // Without any configured root that contains the file, preview is 404
    // (containment enforced: nothing outside the configured roots is read).
    let empty_root = tempfile::tempdir().expect("empty root");
    let app_no_files = build_app(
        db.pool.clone(),
        vec![PreviewSourceRoot {
            id: None,
            root: empty_root.path().to_path_buf(),
        }],
        work_area.path(),
    );
    let response = call(app_no_files, "GET", &preview_path, None, Some(&admin), &[]).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body["code"], "preview_unavailable");

    // Unknown work item -> 404 not_found.
    let work_area = tempfile::tempdir().expect("work area");
    let app = build_app(db.pool.clone(), vec![], work_area.path());
    let response = call(
        app,
        "GET",
        &format!("/api/admin/v1/review/{}/tracks/0/preview", Uuid::now_v7()),
        None,
        Some(&admin),
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body["code"], "not_found");

    db.drop().await;
}
