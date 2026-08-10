//! Integration tests for the Admin diagnostics API (Phase 2 work-package:
//! "Processing queue/worker diagnostics API").
//!
//! These exercise the real Axum router against real PostgreSQL — the store
//! semantics are not mocked. Covers: empty queue, state counts, bounded
//! listing, filtering, work-item detail, event history, active-lease
//! representation, failed work items, pagination bounds, nonexistent work
//! item, malformed identifier, and no secrets in responses.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const T0: time::OffsetDateTime = common::T0;

/// Builds the admin diagnostics router over a real test database pool.
fn admin_router(pool: sqlx::PgPool) -> axum::Router {
    let state = melodee_server::admin::AdminState {
        pool,
        store: WorkItemStore,
    };
    axum::Router::new()
        .route(
            "/api/admin/v1/ingest/work-items/counts",
            axum::routing::get(melodee_server::admin::work_item_counts),
        )
        .route(
            "/api/admin/v1/ingest/work-items",
            axum::routing::get(melodee_server::admin::list_work_items),
        )
        .route(
            "/api/admin/v1/ingest/work-items/{id}",
            axum::routing::get(melodee_server::admin::get_work_item),
        )
        .route(
            "/api/admin/v1/ingest/work-items/{id}/events",
            axum::routing::get(melodee_server::admin::list_work_item_events),
        )
        .route(
            "/api/admin/v1/ingest/leases",
            axum::routing::get(melodee_server::admin::list_active_leases),
        )
        .with_state(state)
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

async fn get_json(router: axum::Router, path: &str) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    (status, body_json(response).await)
}

/// Seeds a ready work item through the legal path and returns its id.
async fn seed_ready(store: &WorkItemStore, pool: &sqlx::PgPool, path: &str) -> Uuid {
    common::seed_ready_item(store, pool, path).await
}

#[tokio::test]
async fn empty_queue_returns_zero_counts_and_empty_list() {
    let db = common::TestDb::create("admin_empty_queue").await;
    let router = admin_router(db.pool.clone());

    let (status, counts) = get_json(router.clone(), "/api/admin/v1/ingest/work-items/counts").await;
    assert_eq!(status, StatusCode::OK);
    assert!(counts.as_array().expect("array").is_empty());

    let (status, items) = get_json(router, "/api/admin/v1/ingest/work-items").await;
    assert_eq!(status, StatusCode::OK);
    assert!(items.as_array().expect("array").is_empty());
    db.drop().await;
}

#[tokio::test]
async fn state_counts_reflect_seeded_items() {
    let db = common::TestDb::create("admin_state_counts").await;
    let store = WorkItemStore;
    seed_ready(&store, &db.pool, "/in/a").await;
    seed_ready(&store, &db.pool, "/in/b").await;

    let router = admin_router(db.pool.clone());
    let (status, counts) = get_json(router, "/api/admin/v1/ingest/work-items/counts").await;
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
async fn bounded_listing_is_paginated_and_filtered() {
    let db = common::TestDb::create("admin_listing").await;
    let store = WorkItemStore;
    for index in 0..10 {
        seed_ready(&store, &db.pool, &format!("/in/item-{index:02}")).await;
    }

    let router = admin_router(db.pool.clone());

    // Default limit (50) returns all 10.
    let (status, items) = get_json(router.clone(), "/api/admin/v1/ingest/work-items").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().expect("array").len(), 10);

    // limit=3 returns 3.
    let (status, items) = get_json(router.clone(), "/api/admin/v1/ingest/work-items?limit=3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().expect("array").len(), 3);

    // limit + offset pages through.
    let (status, page1) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?limit=4&offset=0",
    )
    .await;
    let (status2, page2) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?limit=4&offset=4",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(page1.as_array().expect("array").len(), 4);
    assert_eq!(page2.as_array().expect("array").len(), 4);

    // state filter returns only matching items.
    let (status, items) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?state=ready",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().expect("array").len(), 10);

    let (status, items) = get_json(
        router.clone(),
        "/api/admin/v1/ingest/work-items?state=failed",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(items.as_array().expect("array").is_empty());

    // limit over MAX is clamped, not rejected.
    let (status, items) = get_json(router, "/api/admin/v1/ingest/work-items?limit=99999").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items.as_array().expect("array").len(), 10);
    db.drop().await;
}

#[tokio::test]
async fn unknown_state_filter_returns_400() {
    let db = common::TestDb::create("admin_bad_state").await;
    let router = admin_router(db.pool.clone());
    let (status, body) = get_json(router, "/api/admin/v1/ingest/work-items?state=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body = body.as_object().expect("object");
    assert_eq!(body["code"], "unknown_state");
    assert!(body["correlationId"].as_str().is_some());
    db.drop().await;
}

#[tokio::test]
async fn work_item_detail_returns_full_dto() {
    let db = common::TestDb::create("admin_detail").await;
    let store = WorkItemStore;
    let id = seed_ready(&store, &db.pool, "/in/detail").await;

    let router = admin_router(db.pool.clone());
    let (status, item) = get_json(router, &format!("/api/admin/v1/ingest/work-items/{id}")).await;
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
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    let (status, body) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{missing}"),
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
    let router = admin_router(db.pool.clone());
    // A non-UUID path param fails Axum extraction with a 400/404; the exact
    // code depends on the extractor, but it must not be a 200 or a 500.
    let (status, _body) = get_json(router, "/api/admin/v1/ingest/work-items/not-a-uuid").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "malformed id should be 400 or 404, got {status}"
    );
    db.drop().await;
}

#[tokio::test]
async fn event_history_lists_append_only_audit() {
    let db = common::TestDb::create("admin_events").await;
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
    let router = admin_router(db.pool.clone());
    let missing = Uuid::now_v7();
    let (status, _body) = get_json(
        router,
        &format!("/api/admin/v1/ingest/work-items/{missing}/events"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn active_leases_reflect_claimed_items_only() {
    let db = common::TestDb::create("admin_leases").await;
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
    let (status, leases) = get_json(router, "/api/admin/v1/ingest/leases").await;
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
    let (status, leases) = get_json(router, "/api/admin/v1/ingest/leases").await;
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

    let (status, counts) = get_json(router.clone(), "/api/admin/v1/ingest/work-items/counts").await;
    assert_eq!(status, StatusCode::OK);
    let counts = counts.as_array().expect("array");
    let failed = counts
        .iter()
        .find(|c| c["state"] == "failed")
        .expect("failed count");
    assert_eq!(failed["count"], 1);

    let (status, item) = get_json(router, &format!("/api/admin/v1/ingest/work-items/{id}")).await;
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
    let store = WorkItemStore;
    let id = seed_ready(&store, &db.pool, "/in/secret-check").await;

    let router = admin_router(db.pool.clone());
    let (status, item) = get_json(
        router.clone(),
        &format!("/api/admin/v1/ingest/work-items/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&item).expect("serialize");
    // The store never receives credentials; the DTO must not carry any.
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("postgres://"));
    assert!(!serialized.contains("DATABASE_URL"));

    // Counts and leases also must not leak connection strings.
    let (status, counts) = get_json(router.clone(), "/api/admin/v1/ingest/work-items/counts").await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&counts).expect("serialize");
    assert!(!serialized.contains("postgres://"));

    let (status, leases) = get_json(router, "/api/admin/v1/ingest/leases").await;
    assert_eq!(status, StatusCode::OK);
    let serialized = serde_json::to_string(&leases).expect("serialize");
    assert!(!serialized.contains("postgres://"));
    db.drop().await;
}

// Silence unused import warnings for Arc (kept for potential future shared state).
#[allow(dead_code)]
type _ArcPool = Arc<sqlx::PgPool>;
