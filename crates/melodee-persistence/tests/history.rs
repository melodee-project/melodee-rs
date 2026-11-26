//! Append-only history/audit tests: the event trail
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

/// PERF-011 / RM-16 / ADR-0215: operator-controlled retention removes only
/// high-frequency `lease_renewed` rows older than an explicit cutoff, keeps all
/// transition/failure/completion history, is read-only by default, and is
/// idempotent.
#[tokio::test]
async fn lease_renewal_retention_is_opt_in_bounded_and_preserves_audit_history() {
    let db = TestDb::create("history_lease_retention").await;
    let store = WorkItemStore;
    let id = seed_ready_item(&store, &db.pool, "/inbound/retention-1").await;
    let now = time::OffsetDateTime::now_utc();
    let old = now - time::Duration::days(30);
    let correlation = uuid::Uuid::now_v7();

    sqlx::query(
        "INSERT INTO release_work_item_events
            (work_item_id, event_type, reason_code, worker_id, correlation_id, created_at)
         VALUES ($1, 'lease_renewed', NULL, 'retention-worker', $2, $3),
                ($1, 'lease_renewed', NULL, 'retention-worker', $2, $4),
                ($1, 'transitioned', 'catalog.import_complete', 'retention-worker', $2, $3)",
    )
    .bind(id)
    .bind(correlation)
    .bind(old)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed retention events");
    sqlx::query(
        "UPDATE release_work_item_events SET from_state='ready', to_state='inspecting'
          WHERE event_type='transitioned'",
    )
    .execute(&db.pool)
    .await
    .expect("shape transition event");

    let cutoff = now - time::Duration::days(7);
    let eligible = store
        .prune_expired_lease_renewal_events(&db.pool, cutoff, 1_000, false)
        .await
        .expect("read-only retention check");
    assert_eq!(eligible, 1, "only the old lease renewal is eligible");
    let unchanged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_events WHERE event_type='lease_renewed'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count renewals after check");
    assert_eq!(unchanged, 2, "check mode never deletes");

    let removed = store
        .prune_expired_lease_renewal_events(&db.pool, cutoff, 1, true)
        .await
        .expect("apply retention");
    assert_eq!(removed, 1);
    let remaining: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE event_type='lease_renewed'),
                count(*) FILTER (
                    WHERE event_type='transitioned'
                      AND reason_code='catalog.import_complete')
           FROM release_work_item_events",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count remaining history");
    assert_eq!(
        remaining,
        (1, 1),
        "the recent renewal and the transition audit row remain"
    );
    assert_eq!(
        store
            .prune_expired_lease_renewal_events(&db.pool, cutoff, 1_000, true)
            .await
            .expect("idempotent rerun"),
        0
    );
    db.drop().await;
}
