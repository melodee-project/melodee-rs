//! Failure evidence and crash/recovery simulation (work-package parts XIX,
//! XXII, XXIII): durable failure info, restart durability, lease recovery,
//! rollback, renewal.

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, seed_ready_item, test_config};
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{WorkItemStore, WorkerId};
use time::Duration;

#[tokio::test]
async fn failure_evidence_survives_reconnect() {
    let db = TestDb::create("failure_durable").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

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
            "synthetic read error",
            FailureClass::Retryable,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail");

    // Destroy all application state: new pool (simulates process restart).
    db.pool.close().await;
    let db_url = admin_url().replace("/postgres", &format!("/{}", db.name));
    let pool = sqlx::PgPool::connect(&db_url).await.expect("reconnect");

    let item = store.get(&pool, id).await.expect("get after restart");
    assert_eq!(item.state, IngestionState::Failed);
    assert_eq!(item.attempt_count, 1);
    assert_eq!(
        item.last_error_code.as_ref().map(|c| c.as_str()),
        Some(WorkErrorCode::STAGE_FAILED)
    );
    assert_eq!(
        item.last_error_message.as_deref(),
        Some("synthetic read error")
    );
    assert!(item.last_attempt_at.is_some());
    assert!(item.next_attempt_at.is_some());

    let events = store.list_events(&pool, id).await.expect("events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type == melodee_domain::WorkEventType::Failed)
    );
    pool.close().await;
    db.drop().await;
}

#[tokio::test]
async fn crash_after_claim_recovers_via_expired_lease() {
    let db = TestDb::create("crash_after_claim").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    // Worker claims, then "disappears" (no release, no further calls).
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    // Before expiry: recovery finds nothing.
    let recovered = store
        .recover_expired_leases(&db.pool, &config.retry_policy, T0 + Duration::seconds(1))
        .await
        .expect("recover early");
    assert!(recovered.is_empty());

    // After expiry: recovered. Item was claimed but never started a stage
    // (still Ready), so it returns to the queue WITHOUT a failure mark.
    let after_expiry = T0 + config.lease_duration + Duration::seconds(1);
    let recovered = store
        .recover_expired_leases(&db.pool, &config.retry_policy, after_expiry)
        .await
        .expect("recover");
    assert_eq!(recovered, vec![id]);
    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(
        item.state,
        IngestionState::Ready,
        "unstarted work returns to queue"
    );
    assert_eq!(
        item.attempt_count, 0,
        "no attempt penalty for unstarted work"
    );
    assert_eq!(item.lease_owner, None);

    // Another worker can claim it.
    let claims = store
        .claim(&db.pool, &WorkerId::new(), &config, after_expiry)
        .await
        .expect("reclaim");
    assert_eq!(claims.len(), 1);
    db.drop().await;
}

#[tokio::test]
async fn crash_mid_stage_marks_lease_lost_and_schedules_retry() {
    let db = TestDb::create("crash_mid_stage").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

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
    // Worker disappears mid-stage (no failure recorded, no release).

    let after_expiry = T0 + config.lease_duration + Duration::seconds(1);
    let recovered = store
        .recover_expired_leases(&db.pool, &config.retry_policy, after_expiry)
        .await
        .expect("recover");
    assert_eq!(recovered, vec![id]);

    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.state, IngestionState::Failed);
    assert_eq!(
        item.attempt_count, 1,
        "crash consumes an attempt (crash-loop protection)"
    );
    assert_eq!(
        item.last_error_code.as_ref().map(|c| c.as_str()),
        Some(WorkErrorCode::LEASE_LOST)
    );
    assert!(item.next_attempt_at.is_some(), "retry scheduled");

    // Eligible retry can be claimed by a new worker and completed.
    let next = item.next_attempt_at.expect("next");
    let worker2 = WorkerId::new();
    let claims = store
        .claim(&db.pool, &worker2, &config, next)
        .await
        .expect("claim retry");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].state, IngestionState::Ready);

    let events = store.list_events(&db.pool, id).await.expect("events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type == melodee_domain::WorkEventType::LeaseExpiredRecovered),
        "recovery event recorded"
    );
    db.drop().await;
}

#[tokio::test]
async fn lease_renewal_extends_ownership() {
    let db = TestDb::create("lease_renewal").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    let midway = T0 + config.lease_duration / 2;
    let new_expiry = store
        .renew_lease(&db.pool, id, &worker, config.lease_duration, midway)
        .await
        .expect("renew");
    assert_eq!(new_expiry, midway + config.lease_duration);

    // A rival cannot claim at the ORIGINAL expiry — the renewal holds.
    let rival_at_old_expiry = T0 + config.lease_duration + Duration::seconds(1);
    let claims = store
        .claim(&db.pool, &WorkerId::new(), &config, rival_at_old_expiry)
        .await
        .expect("rival claim");
    assert!(claims.is_empty(), "renewed lease must hold");

    // Renewal by a stranger fails.
    let result = store
        .renew_lease(
            &db.pool,
            id,
            &WorkerId::new(),
            config.lease_duration,
            midway,
        )
        .await;
    assert!(result.is_err());
    db.drop().await;
}

#[tokio::test]
async fn permanent_failure_stops_automatic_retry() {
    let db = TestDb::create("failure_permanent").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

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
    let item = store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new("io.unreadable"),
            "synthetic permanent error",
            FailureClass::Permanent,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail");
    assert_eq!(item.state, IngestionState::Failed);
    assert_eq!(
        item.next_attempt_at, None,
        "permanent failures are not rescheduled"
    );

    let claims = store
        .claim(
            &db.pool,
            &WorkerId::new(),
            &config,
            T0 + Duration::days(365),
        )
        .await
        .expect("claim");
    assert!(claims.is_empty());
    db.drop().await;
}

#[tokio::test]
async fn review_failure_parks_in_needs_review() {
    let db = TestDb::create("failure_review").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");
    for next in [
        IngestionState::Inspecting,
        IngestionState::Normalizing,
        IngestionState::Validating,
    ] {
        store
            .transition(
                &db.pool,
                id,
                Some(&worker),
                TransitionKind::Worker,
                next,
                None,
                T0,
            )
            .await
            .expect("advance");
    }
    let item = store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::POLICY_REVIEW),
            "uncertain classification",
            FailureClass::NeedsReview,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail to review");
    assert_eq!(item.state, IngestionState::NeedsReview);
    assert_eq!(item.lease_owner, None, "parked work holds no lease");
    db.drop().await;
}

#[tokio::test]
async fn failure_without_lease_is_rejected() {
    let db = TestDb::create("failure_unleased").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let result = store
        .record_failure(
            &db.pool,
            id,
            &WorkerId::new(),
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "no lease held",
            FailureClass::Retryable,
            &config.retry_policy,
            T0,
        )
        .await;
    assert!(result.is_err(), "failure requires an active lease");
    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.state, IngestionState::Ready);
    assert_eq!(item.attempt_count, 0);
    db.drop().await;
}

use common::admin_url;
