//! Append-only history/audit tests (work-package part XXI): the event trail
//! answers "what happened to this Release and why".

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, seed_ready_item, test_config};
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode, WorkEventType};
use melodee_persistence::{WorkItemStore, WorkerId};

#[tokio::test]
async fn full_lifecycle_writes_complete_audit_trail() {
    let db = TestDb::create("history_lifecycle").await;
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
            "transient",
            FailureClass::Retryable,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("fail");

    let item = store.get(&db.pool, id).await.expect("get");
    let next = item.next_attempt_at.expect("retry at");
    let worker2 = WorkerId::new();
    store
        .claim(&db.pool, &worker2, &config, next)
        .await
        .expect("reclaim");

    let events = store.list_events(&db.pool, id).await.expect("events");
    let types: Vec<_> = events.iter().map(|e| e.event_type).collect();

    assert_eq!(types[0], WorkEventType::Created);
    // discovered -> settling -> ready (seed path)
    let transitions: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == WorkEventType::Transitioned)
        .map(|e| (e.from_state, e.to_state))
        .collect();
    assert!(
        transitions.contains(&(
            Some(IngestionState::Discovered),
            Some(IngestionState::Settling)
        )),
        "discovery transition recorded: {transitions:?}"
    );
    assert!(transitions.contains(&(Some(IngestionState::Settling), Some(IngestionState::Ready))));
    assert!(types.contains(&WorkEventType::Claimed), "claim recorded");
    assert!(types.contains(&WorkEventType::Failed), "failure recorded");
    assert!(
        types.contains(&WorkEventType::RetryScheduled),
        "retry recorded"
    );
    assert!(
        transitions.contains(&(Some(IngestionState::Failed), Some(IngestionState::Ready))),
        "requeue recorded"
    );

    // Every event carries the correlation id; worker ids are recorded where relevant.
    assert!(
        events
            .iter()
            .all(|e| e.correlation_id == item.correlation_id)
    );
    let claim = events
        .iter()
        .find(|e| e.event_type == WorkEventType::Claimed)
        .expect("claim event");
    assert_eq!(claim.worker_id.as_deref(), Some(worker.as_str()));

    // Events are ordered and append-only (monotonic ids).
    let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
    assert!(ids.windows(2).all(|w| w[1] > w[0]), "append-only ordering");
    db.drop().await;
}

#[tokio::test]
async fn history_is_queryable_per_item_without_touching_others() {
    let db = TestDb::create("history_isolation").await;
    let store = WorkItemStore;
    let a = seed_ready_item(&store, &db.pool, "/inbound/a").await;
    let b = seed_ready_item(&store, &db.pool, "/inbound/b").await;

    let events_a = store.list_events(&db.pool, a).await.expect("events a");
    let events_b = store.list_events(&db.pool, b).await.expect("events b");
    assert!(events_a.iter().all(|e| e.work_item_id == a));
    assert!(events_b.iter().all(|e| e.work_item_id == b));
    db.drop().await;
}
