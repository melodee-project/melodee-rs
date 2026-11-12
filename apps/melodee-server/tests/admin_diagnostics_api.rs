//! Admin diagnostics API integration tests covering ADR-0139 gating,
//! pagination envelopes, and sorting.
//!
//! These exercise the real Axum router against real PostgreSQL — the store
//! semantics are not mocked. Covers: admin gating (401 anonymous, 403
//! non-admin, 200 admin per route), empty queue, state counts, bounded
//! listing with the pagination envelope, filtering, whitelisted sorting,
//! work-item detail, event history, active-lease representation, failed work
//! items, pagination bounds, nonexistent work item, malformed identifier, and
//! no secrets in responses.

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
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{AuthStore, NativeUserImport, WorkEngineConfig, WorkItemStore, WorkerId};
use melodee_server::admin_access::AdminAccessState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const T0: time::OffsetDateTime = common::T0;
const JWT_KEY: &str = "test-admin-diagnostics-jwt-key-at-least-32b";

struct Fixture {
    admin_token: String,
    plain_token: String,
}

/// Seeds an administrator and a plain user, then mints native access JWTs
/// for both (the `AdminPrincipal` gate checks the database, not just the
/// token).
async fn seed_users(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let mut tokens = Vec::new();
    for (username, is_admin) in [("diag-admin", true), ("diag-plain", false)] {
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

/// Builds the admin diagnostics router over a real test database pool.
fn admin_router(pool: sqlx::PgPool) -> axum::Router {
    melodee_server::admin::router().with_state(AdminAccessState {
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

/// Asserts the stable auth failure envelope (`code`, `correlationId`).
fn assert_auth_envelope(body: &Value, code: &str) {
    let body = body.as_object().expect("envelope object");
    assert_eq!(body["code"], code);
    assert!(body["correlationId"].as_str().is_some());
}

/// Seeds a ready work item through the legal path and returns its id.
async fn seed_ready(store: &WorkItemStore, pool: &sqlx::PgPool, path: &str) -> Uuid {
    common::seed_ready_item(store, pool, path).await
}

#[tokio::test]
async fn anonymous_requests_get_401_with_envelope_on_every_route() {
    let db = common::TestDb::create("admin_diag_anon_401").await;
    let _fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    for path in [
        "/api/admin/v1/system/info".to_string(),
        "/api/admin/v1/ingest/work-items/counts".to_string(),
        "/api/admin/v1/ingest/work-items".to_string(),
        format!("/api/admin/v1/ingest/work-items/{missing}"),
        format!("/api/admin/v1/ingest/work-items/{missing}/events"),
        "/api/admin/v1/ingest/leases".to_string(),
    ] {
        let (status, body) = get_json(router.clone(), &path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_auth_envelope(&body, "unauthenticated");
    }
    db.drop().await;
}

#[tokio::test]
async fn non_admin_requests_get_403_with_envelope_on_every_route() {
    let db = common::TestDb::create("admin_diag_plain_403").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    for path in [
        "/api/admin/v1/system/info".to_string(),
        "/api/admin/v1/ingest/work-items/counts".to_string(),
        "/api/admin/v1/ingest/work-items".to_string(),
        format!("/api/admin/v1/ingest/work-items/{missing}"),
        format!("/api/admin/v1/ingest/work-items/{missing}/events"),
        "/api/admin/v1/ingest/leases".to_string(),
    ] {
        let (status, body) = get_json(router.clone(), &path, Some(&fixture.plain_token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_auth_envelope(&body, "forbidden");
    }
    db.drop().await;
}

#[tokio::test]
async fn admin_gets_200_on_every_route() {
    let db = common::TestDb::create("admin_diag_admin_200").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let id = seed_ready(&store, &db.pool, "/in/gated").await;
    let router = admin_router(db.pool.clone());
    for path in [
        "/api/admin/v1/system/info".to_string(),
        "/api/admin/v1/ingest/work-items/counts".to_string(),
        "/api/admin/v1/ingest/work-items".to_string(),
        format!("/api/admin/v1/ingest/work-items/{id}"),
        format!("/api/admin/v1/ingest/work-items/{id}/events"),
        "/api/admin/v1/ingest/leases".to_string(),
    ] {
        let (status, _body) = get_json(router.clone(), &path, Some(&fixture.admin_token)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    db.drop().await;
}

#[tokio::test]
async fn system_info_returns_health_metadata_for_admins() {
    let db = common::TestDb::create("admin_diag_system_info").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let (status, body) = get_json(
        router,
        "/api/admin/v1/system/info",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["service"], "melodee-server");
    assert_eq!(body["status"], "ok");
    db.drop().await;
}

#[tokio::test]
async fn empty_queue_returns_zero_counts_and_empty_page() {
    let db = common::TestDb::create("admin_empty_queue").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());

    let (status, counts) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items/counts",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(counts.as_array().expect("array").is_empty());

    let (status, page) = get_json(
        router,
        "/api/admin/v1/ingest/work-items",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().expect("items").is_empty());
    assert_eq!(page["total"], 0);
    assert_eq!(page["limit"], 50);
    assert_eq!(page["offset"], 0);
    db.drop().await;
}

#[tokio::test]
async fn state_counts_reflect_seeded_items() {
    let db = common::TestDb::create("admin_state_counts").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    seed_ready(&store, &db.pool, "/in/a").await;
    seed_ready(&store, &db.pool, "/in/b").await;

    let router = admin_router(db.pool.clone());
    let (status, counts) = get_json(
        router,
        "/api/admin/v1/ingest/work-items/counts",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counts = counts.as_array().expect("array");
    let ready = counts
        .iter()
        .find(|c| c["state"] == "ready")
        .expect("ready count");
    assert_eq!(ready["count"], 2);
    db.drop().await;
}

#[tokio::test]
async fn bounded_listing_is_paginated_and_filtered_with_envelope() {
    let db = common::TestDb::create("admin_listing").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    for index in 0..10 {
        seed_ready(&store, &db.pool, &format!("/in/item-{index:02}")).await;
    }

    let router = admin_router(db.pool.clone());
    let admin = Some(fixture.admin_token.as_str());

    // Default limit (50) returns all 10 with the honest total.
    let (status, page) = get_json(router.clone(), "/api/admin/v1/ingest/work-items", admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().expect("items").len(), 10);
    assert_eq!(page["total"], 10);
    assert_eq!(page["limit"], 50);
    assert_eq!(page["offset"], 0);

    // limit=3 returns 3; the total still reflects the whole filter.
    let (status, page) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?limit=3",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().expect("items").len(), 3);
    assert_eq!(page["total"], 10);
    assert_eq!(page["limit"], 3);

    // limit + offset pages through.
    let (status, page1) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?limit=4&offset=0",
        admin,
    )
    .await;
    let (status2, page2) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?limit=4&offset=4",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(page1["items"].as_array().expect("items").len(), 4);
    assert_eq!(page1["offset"], 0);
    assert_eq!(page2["items"].as_array().expect("items").len(), 4);
    assert_eq!(page2["offset"], 4);
    assert_eq!(page2["total"], 10);
    // Pages are disjoint.
    let ids1: Vec<&str> = page1["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    let ids2: Vec<&str> = page2["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(ids1.iter().all(|id| !ids2.contains(id)));

    // state filter returns only matching items, and total respects it.
    let (status, page) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?state=ready&limit=4",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().expect("items").len(), 4);
    assert_eq!(page["total"], 10);

    let (status, page) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?state=failed",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().expect("items").is_empty());
    assert_eq!(page["total"], 0);

    // limit over MAX is clamped, not rejected.
    let (status, page) =
        get_json(router, "/api/admin/v1/ingest/work-items?limit=99999", admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().expect("items").len(), 10);
    assert_eq!(page["limit"], 500);
    db.drop().await;
}

#[tokio::test]
async fn sort_whitelist_orders_results_and_rejects_unknown_keys() {
    let db = common::TestDb::create("admin_sort").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let failed_id = seed_ready(&store, &db.pool, "/in/sort-failed").await;
    seed_ready(&store, &db.pool, "/in/sort-ready-a").await;
    seed_ready(&store, &db.pool, "/in/sort-ready-b").await;

    // Drive one item to `failed` so a state sort has distinct values.
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");
    store
        .transition(
            &db.pool,
            failed_id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("inspect");
    store
        .record_failure(
            &db.pool,
            failed_id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "synthetic inspection error",
            FailureClass::Permanent,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail");

    let router = admin_router(db.pool.clone());
    let admin = Some(fixture.admin_token.as_str());

    // state:asc puts "failed" before "ready".
    let (status, page) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?sort=state:asc",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["state"], "failed");

    // state:desc puts the two "ready" items first.
    let (status, page) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?sort=state:desc",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items[0]["state"], "ready");
    assert_eq!(items[1]["state"], "ready");
    assert_eq!(items[2]["state"], "failed");

    // Whitelisted keys with both directions are accepted.
    for sort in [
        "addedAt:asc",
        "addedAt:desc",
        "updatedAt:asc",
        "updatedAt:desc",
        "attemptCount:asc",
        "attemptCount:desc",
    ] {
        let (status, page) = get_json(
            router.clone(),
            &format!("/api/admin/v1/ingest/work-items?sort={sort}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sort}");
        assert_eq!(page["items"].as_array().expect("items").len(), 3, "{sort}");
    }

    // Unknown keys, unknown directions, and missing suffixes are 400 with
    // the stable error envelope.
    for sort in [
        "bogus:asc",
        "addedAt:sideways",
        "addedAt",
        "state:asc:extra",
    ] {
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/admin/v1/ingest/work-items?sort={sort}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{sort}");
        let body = body.as_object().expect("envelope");
        assert_eq!(body["code"], "unknown_sort", "{sort}");
        assert!(body["correlationId"].as_str().is_some());
    }
    db.drop().await;
}

#[tokio::test]
async fn unknown_state_filter_returns_400() {
    let db = common::TestDb::create("admin_bad_state").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let (status, body) = get_json(
        router,
        "/api/admin/v1/ingest/work-items?state=bogus",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body = body.as_object().expect("object");
    assert_eq!(body["code"], "unknown_state");
    assert!(body["correlationId"].as_str().is_some());
    db.drop().await;
}

#[tokio::test]
async fn work_item_detail_returns_full_dto() {
    let db = common::TestDb::create("admin_detail").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let id = seed_ready(&store, &db.pool, "/in/detail").await;

    let router = admin_router(db.pool.clone());
    let (status, item) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{id}"),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = item.as_object().expect("object");
    assert_eq!(
        item["id"].as_str().map(str::to_string),
        Some(id.to_string())
    );
    assert_eq!(item["state"], "ready");
    assert_eq!(item["sourcePath"], "/in/detail");
    assert_eq!(item["attemptCount"], 0);
    // Operator-safe fields present; no secret fields exist in the DTO shape.
    assert!(item.get("leaseOwner").is_some());
    assert!(item.get("lastErrorCode").is_some());
    db.drop().await;
}

#[tokio::test]
async fn nonexistent_work_item_returns_404_with_envelope() {
    let db = common::TestDb::create("admin_not_found").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    let (status, body) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{missing}"),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body = body.as_object().expect("object");
    assert_eq!(body["code"], "not_found");
    assert!(body["correlationId"].as_str().is_some());
    db.drop().await;
}

#[tokio::test]
async fn malformed_identifier_returns_400_or_404() {
    let db = common::TestDb::create("admin_bad_id").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    // A non-UUID path param fails Axum extraction with a 400/404; the exact
    // code depends on the extractor, but it must not be a 200 or a 500.
    let (status, _body) = get_json(
        router,
        "/api/admin/v1/ingest/work-items/not-a-uuid",
        Some(&fixture.admin_token),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "malformed id should be 400 or 404, got {status}"
    );
    db.drop().await;
}

#[tokio::test]
async fn event_history_lists_append_only_audit() {
    let db = common::TestDb::create("admin_events").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let id = seed_ready(&store, &db.pool, "/in/events").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");
    store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("inspect");

    let router = admin_router(db.pool.clone());
    let (status, events) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{id}/events"),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = events.as_array().expect("array");
    // created + (discovered->settling + settling->ready from seed) + claimed + transitioned
    assert!(
        events.len() >= 4,
        "expected audit trail, got {}",
        events.len()
    );
    // Every event references the work item.
    assert!(
        events
            .iter()
            .all(|e| e["workItemId"].as_str().map(str::to_string) == Some(id.to_string()))
    );
    db.drop().await;
}

#[tokio::test]
async fn events_for_nonexistent_item_returns_404() {
    let db = common::TestDb::create("admin_events_404").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    let (status, _body) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{missing}/events"),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn active_leases_reflect_claimed_items_only() {
    let db = common::TestDb::create("admin_leases").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let id_a = seed_ready(&store, &db.pool, "/in/leased").await;
    // id_b stays in `discovered` (never claimable), so only id_a can be leased.
    let item_b = melodee_domain::ReleaseWorkItem::discovered(
        "/in/free".to_string(),
        None,
        Uuid::now_v7(),
        T0,
    );
    let id_b = item_b.id;
    store.create(&db.pool, &item_b).await.expect("create b");

    // Claim at the real clock so the lease is active relative to now_utc()
    // used by the API handler.
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, time::OffsetDateTime::now_utc())
        .await
        .expect("claim");

    let router = admin_router(db.pool.clone());
    let (status, leases) = get_json(
        router,
        "/api/admin/v1/ingest/leases",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let leases = leases.as_array().expect("array");
    assert_eq!(leases.len(), 1, "exactly one active lease expected");
    let lease = &leases[0];
    assert_eq!(
        lease["workItemId"].as_str().map(str::to_string),
        Some(id_a.to_string())
    );
    assert_eq!(lease["leaseOwner"].as_str(), Some(worker.as_str()));
    assert_eq!(lease["state"], "ready");
    // id_b is not leased.
    assert!(
        leases
            .iter()
            .all(|l| l["workItemId"].as_str().map(str::to_string) != Some(id_b.to_string()))
    );
    db.drop().await;
}

#[tokio::test]
async fn expired_lease_not_listed_as_active() {
    let db = common::TestDb::create("admin_expired_lease").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let id = seed_ready(&store, &db.pool, "/in/expiring").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    // Query leases at T0 + 1h: the default lease is 300s, so it has expired.
    let router = admin_router(db.pool.clone());
    let (status, leases) = get_json(
        router,
        "/api/admin/v1/ingest/leases",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // list_active_leases uses now_utc(); the T0 claim (lease 300s) is long
    // expired relative to the real clock, so it must not appear as active.
    let leases = leases.as_array().expect("array");
    assert!(
        leases
            .iter()
            .all(|l| l["workItemId"].as_str().map(str::to_string) != Some(id.to_string())),
        "expired lease must not appear active"
    );
    db.drop().await;
}

#[tokio::test]
async fn failed_work_item_visible_in_counts_and_detail() {
    let db = common::TestDb::create("admin_failed").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let id = seed_ready(&store, &db.pool, "/in/failing").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");
    store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("inspect");
    store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "synthetic inspection error",
            FailureClass::Permanent,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail");

    let router = admin_router(db.pool.clone());
    let admin = Some(fixture.admin_token.as_str());

    let (status, counts) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items/counts",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counts = counts.as_array().expect("array");
    let failed = counts
        .iter()
        .find(|c| c["state"] == "failed")
        .expect("failed count");
    assert_eq!(failed["count"], 1);

    let (status, item) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = item.as_object().expect("object");
    assert_eq!(item["state"], "failed");
    assert_eq!(item["lastErrorCode"], "stage.failed");
    assert_eq!(item["lastErrorMessage"], "synthetic inspection error");
    db.drop().await;
}

#[tokio::test]
async fn response_contains_no_secrets() {
    let db = common::TestDb::create("admin_no_secrets").await;
    let fixture = seed_users(&db).await;
    let store = WorkItemStore;
    let id = seed_ready(&store, &db.pool, "/in/secret-check").await;

    let router = admin_router(db.pool.clone());
    let admin = Some(fixture.admin_token.as_str());
    let (status, item) = get_json(
        router.clone(),
        &format!("/api/admin/v1/ingest/work-items/{id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&item).expect("serialize");
    // The store never receives credentials; the DTO must not carry any.
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("postgres://"));
    assert!(!serialized.contains("DATABASE_URL"));

    // Counts and leases also must not leak connection strings.
    let (status, counts) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items/counts",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&counts).expect("serialize");
    assert!(!serialized.contains("postgres://"));

    let (status, leases) = get_json(router, "/api/admin/v1/ingest/leases", admin).await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&leases).expect("serialize");
    assert!(!serialized.contains("postgres://"));
    db.drop().await;
}

/// ADR-0185: operators filter and group by phase, a derived state set, while
/// `state` stays available for one exact state. The response also carries the
/// derived `phase` so clients never re-implement the mapping.
#[tokio::test]
async fn work_item_phase_filter_groups_states_and_reports_the_derived_phase() {
    let db = common::TestDb::create("admin_diag_phase_filter").await;
    let fixture = seed_users(&db).await;
    let router = admin_router(db.pool.clone());
    let store = WorkItemStore;

    // One item per phase is unnecessary; cover an in-flight phase and a
    // terminal one, plus a state that shares a phase with another.
    let ready_id = seed_ready(&store, &db.pool, "phase/ready").await;
    let mut tx = db.pool.begin().await.expect("begin");
    sqlx::query("UPDATE release_work_items SET state = 'settling', updated_at = $2 WHERE id = $1")
        .bind(ready_id)
        .bind(T0)
        .execute(&mut *tx)
        .await
        .expect("park in settling");
    tx.commit().await.expect("commit");
    let failed_id = seed_ready(&store, &db.pool, "phase/failed").await;
    let mut tx = db.pool.begin().await.expect("begin");
    sqlx::query("UPDATE release_work_items SET state = 'failed', updated_at = $2 WHERE id = $1")
        .bind(failed_id)
        .bind(T0)
        .execute(&mut *tx)
        .await
        .expect("park in failed");
    tx.commit().await.expect("commit");

    // A queue item that is not yet accepted by policy is not client-visible.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=inbound",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["clientVisible"], false);

    // `inbound` covers discovered/settling/ready, so it matches the settling item.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=inbound",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["state"], "settling");
    assert_eq!(body["items"][0]["phase"], "inbound");

    // `failed` is its own phase.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=failed",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["phase"], "failed");

    // An in-flight phase with no members returns an honest empty page.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=active",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);

    // `state` still filters one exact state and reports the shared phase.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?state=settling",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["phase"], "inbound");

    // Unknown phase is a client error, not an empty result.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=nonsense",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_phase");

    // Supplying both filters is ambiguous and rejected.
    let (status, body) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?phase=inbound&state=ready",
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_filter");

    db.drop().await;
}
