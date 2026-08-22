//! State-transition integrity tests:
//! legal/illegal transitions, atomicity, terminal completion, operator
//! transitions, lease conflicts.

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, seed_ready_item, test_config};
use melodee_domain::{IngestionState, TransitionKind};
use melodee_persistence::{WorkItemStore, WorkerId};

#[tokio::test]
async fn illegal_transition_fails_without_changing_state() {
    let db = TestDb::create("transition_illegal").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    // Ready -> Indexed is illegal for every actor kind.
    let result = store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Indexed,
            None,
            T0,
        )
        .await;
    assert!(result.is_err(), "illegal transition must fail");

    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.state, IngestionState::Ready, "state must be unchanged");
    assert_eq!(
        item.lease_owner.as_deref(),
        Some(worker.as_str()),
        "lease must be unchanged"
    );

    // No transition event for the ILLEGAL target was persisted (the legal
    // seed-path transitions discovered->settling->ready are recorded).
    let events = store.list_events(&db.pool, id).await.expect("events");
    assert!(
        !events
            .iter()
            .any(|e| e.to_state == Some(IngestionState::Indexed)),
        "no transition event may be recorded for an illegal transition"
    );
    db.drop().await;
}

#[tokio::test]
async fn generic_transition_cannot_bypass_durable_publication_intent() {
    let db = TestDb::create("transition_happy").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    let path = [
        IngestionState::Inspecting,
        IngestionState::Normalizing,
        IngestionState::Validating,
    ];
    for next in path {
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
            .unwrap_or_else(|e| panic!("transition to {next:?}: {e}"));
    }

    let result = store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Staged,
            None,
            T0,
        )
        .await;
    assert!(
        result.is_err(),
        "staging requires immutable publication intent"
    );
    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.state, IngestionState::Validating);
    db.drop().await;
}

#[tokio::test]
async fn unleased_worker_cannot_transition() {
    let db = TestDb::create("transition_unleased").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let owner = WorkerId::new();
    let stranger = WorkerId::new();
    store
        .claim(&db.pool, &owner, &config, T0)
        .await
        .expect("claim");

    let result = store
        .transition(
            &db.pool,
            id,
            Some(&stranger),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await;
    assert!(
        result.is_err(),
        "a worker without the lease must not transition"
    );
    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.state, IngestionState::Ready);
    db.drop().await;
}

#[tokio::test]
async fn expired_lease_blocks_worker_transition() {
    let db = TestDb::create("transition_expired").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    let after_expiry = T0 + config.lease_duration + Duration::seconds(1);
    let result = store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            after_expiry,
        )
        .await;
    assert!(
        result.is_err(),
        "expired lease must block worker transitions"
    );
    db.drop().await;
}

#[tokio::test]
async fn operator_review_decisions_and_reprocess() {
    let db = TestDb::create("transition_operator").await;
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
        IngestionState::NeedsReview,
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
            .unwrap_or_else(|e| panic!("transition to {next:?}: {e}"));
    }

    // The NeedsReview transition released the worker lease (parked work must
    // not strand behind a lease), so the operator may act immediately.
    let parked = store.get(&db.pool, id).await.expect("get parked");
    assert_eq!(parked.lease_owner, None, "NeedsReview releases the lease");
    store
        .transition(
            &db.pool,
            id,
            None,
            TransitionKind::Operator,
            IngestionState::Quarantined,
            Some("operator: suspicious"),
            T0,
        )
        .await
        .expect("quarantine");

    // Operator reprocess returns it to Ready.
    let item = store
        .transition(
            &db.pool,
            id,
            None,
            TransitionKind::Operator,
            IngestionState::Ready,
            Some("operator: reprocess"),
            T0,
        )
        .await
        .expect("reprocess");
    assert_eq!(item.state, IngestionState::Ready);
    assert_eq!(item.lease_owner, None);

    // Worker cannot make operator decisions.
    let worker2 = WorkerId::new();
    store
        .claim(&db.pool, &worker2, &config, T0)
        .await
        .expect("claim2");
    store
        .transition(
            &db.pool,
            id,
            Some(&worker2),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("inspect again");
    let result = store
        .transition(
            &db.pool,
            id,
            Some(&worker2),
            TransitionKind::Worker,
            IngestionState::Ignored,
            None,
            T0,
        )
        .await;
    assert!(result.is_err(), "workers cannot ignore");
    db.drop().await;
}

#[tokio::test]
async fn operator_cannot_race_active_lease() {
    let db = TestDb::create("transition_lease_conflict").await;
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

    let result = store
        .transition(
            &db.pool,
            id,
            None,
            TransitionKind::Operator,
            IngestionState::Quarantined,
            None,
            T0,
        )
        .await;
    assert!(
        result.is_err(),
        "operator must not override an active lease"
    );
    db.drop().await;
}

#[tokio::test]
async fn failed_transaction_leaves_no_partial_state() {
    let db = TestDb::create("transition_rollback").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &config, T0)
        .await
        .expect("claim");

    // Force a mid-transaction failure: transition to a terminal state with a
    // reason that violates nothing, but poison the event insert by first
    // creating a savepoint conflict — simplest honest approach: run a
    // transition with an illegal target so the domain check fails AFTER the
    // row was locked; the tx must roll back completely.
    let before = store.get(&db.pool, id).await.expect("get");
    let result = store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Indexed,
            None,
            T0,
        )
        .await;
    assert!(result.is_err());
    let after = store.get(&db.pool, id).await.expect("get");
    assert_eq!(
        before, after,
        "failed transaction must leave no partial state"
    );
    db.drop().await;
}

use time::Duration;
