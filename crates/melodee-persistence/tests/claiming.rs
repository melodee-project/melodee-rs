//! Claiming/lease concurrency tests against real PostgreSQL: exclusive claim,
//! parallel queue, lease blocking, expiry,
//! retry delay, duplicate-claim races.

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, seed_ready_item, test_config};
use melodee_domain::{IngestionState, RetryPolicy};
use melodee_persistence::{WorkItemStore, WorkerId};
use std::collections::HashSet;
use time::Duration;

#[tokio::test]
async fn exclusive_claim_single_item_two_workers() {
    let db = TestDb::create("claim_exclusive").await;
    let store = WorkItemStore;
    let config = test_config();
    seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker_a = WorkerId::new();
    let worker_b = WorkerId::new();
    let (a, b) = tokio::join!(
        store.claim(&db.pool, &worker_a, &config, T0),
        store.claim(&db.pool, &worker_b, &config, T0),
    );
    let claimed_a = a.expect("worker a");
    let claimed_b = b.expect("worker b");
    assert_eq!(
        claimed_a.len() + claimed_b.len(),
        1,
        "exactly one worker may claim the single ready item"
    );
    db.drop().await;
}

#[tokio::test]
async fn parallel_queue_no_duplicate_ownership() {
    let db = TestDb::create("claim_parallel").await;
    let store = WorkItemStore;
    let config = test_config();

    const ITEMS: usize = 1_000;
    const WORKERS: usize = 32;

    for index in 0..ITEMS {
        seed_ready_item(&store, &db.pool, &format!("/inbound/release-{index}")).await;
    }

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            let worker = WorkerId::new();
            let mut mine = Vec::new();
            loop {
                let batch = store
                    .claim(&pool, &worker, &config, T0)
                    .await
                    .expect("claim");
                if batch.is_empty() {
                    break;
                }
                mine.extend(batch.into_iter().map(|item| item.id));
            }
            mine
        }));
    }

    let mut all_claimed = Vec::new();
    for handle in handles {
        all_claimed.extend(handle.await.expect("worker task"));
    }

    let unique: HashSet<_> = all_claimed.iter().collect();
    assert_eq!(all_claimed.len(), ITEMS, "every item claimed exactly once");
    assert_eq!(unique.len(), all_claimed.len(), "zero duplicate claims");
    db.drop().await;
}

#[tokio::test]
async fn active_lease_blocks_second_claim() {
    let db = TestDb::create("claim_active_lease").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker_a = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker_a, &config, T0)
        .await
        .expect("claim a");
    assert_eq!(claimed.len(), 1);

    // Same instant: second worker cannot claim (lease active).
    let claimed_b = store
        .claim(&db.pool, &WorkerId::new(), &config, T0)
        .await
        .expect("claim b");
    assert!(
        claimed_b.is_empty(),
        "active lease must block other workers"
    );

    // Before expiry: still blocked.
    let before_expiry = T0 + config.lease_duration - Duration::seconds(1);
    let claimed_c = store
        .claim(&db.pool, &WorkerId::new(), &config, before_expiry)
        .await
        .expect("claim c");
    assert!(claimed_c.is_empty());

    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.lease_owner.as_deref(), Some(worker_a.as_str()));
    db.drop().await;
}

#[tokio::test]
async fn expired_lease_is_claimable_by_another_worker() {
    let db = TestDb::create("claim_expired_lease").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let worker_a = WorkerId::new();
    store
        .claim(&db.pool, &worker_a, &config, T0)
        .await
        .expect("claim a");

    let after_expiry = T0 + config.lease_duration + Duration::seconds(1);
    let claimed_b = store
        .claim(&db.pool, &WorkerId::new(), &config, after_expiry)
        .await
        .expect("claim b after expiry");
    assert_eq!(claimed_b.len(), 1, "expired lease must be reclaimable");
    assert_eq!(claimed_b[0].id, id);
    db.drop().await;
}

#[tokio::test]
async fn future_next_attempt_blocks_claim_until_eligible() {
    let db = TestDb::create("claim_retry_delay").await;
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    // Claim and start processing, then fail retryably at T0.
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
            melodee_domain::TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("start inspecting");
    let failed = store
        .record_failure(
            &db.pool,
            id,
            &worker,
            melodee_domain::WorkErrorCode::new(melodee_domain::WorkErrorCode::STAGE_FAILED),
            "synthetic transient failure",
            melodee_domain::FailureClass::Retryable,
            &config.retry_policy,
            T0,
        )
        .await
        .expect("record failure");
    let next = failed.next_attempt_at.expect("retry scheduled");
    assert_eq!(failed.state, IngestionState::Failed);
    assert_eq!(failed.attempt_count, 1);

    // Before eligibility: not claimable.
    let before = next - Duration::seconds(1);
    let claims = store
        .claim(&db.pool, &WorkerId::new(), &config, before)
        .await
        .expect("early claim");
    assert!(
        claims.is_empty(),
        "work must not be retried before eligibility"
    );

    // At eligibility: claimable, and the failed->ready requeue is recorded.
    let claims = store
        .claim(&db.pool, &WorkerId::new(), &config, next)
        .await
        .expect("eligible claim");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].state, IngestionState::Ready);
    let events = store.list_events(&db.pool, id).await.expect("events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type == melodee_domain::WorkEventType::RetryScheduled),
        "retry_scheduled event must exist"
    );
    assert!(
        events.iter().any(|e| {
            e.event_type == melodee_domain::WorkEventType::Transitioned
                && e.from_state == Some(IngestionState::Failed)
                && e.to_state == Some(IngestionState::Ready)
        }),
        "failed->ready requeue transition must be recorded"
    );
    db.drop().await;
}

#[tokio::test]
async fn repeated_parallel_polls_do_not_duplicate_ownership() {
    let db = TestDb::create("claim_poll_race").await;
    let store = WorkItemStore;
    let config = test_config();
    seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    // 64 simultaneous polls for one item.
    let mut handles = Vec::new();
    for _ in 0..64 {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            store
                .claim(&pool, &WorkerId::new(), &config, T0)
                .await
                .expect("claim")
        }));
    }
    let mut total = 0usize;
    for handle in handles {
        total += handle.await.expect("task").len();
    }
    assert_eq!(total, 1, "exactly one of 64 racing polls may win");
    db.drop().await;
}

#[tokio::test]
async fn claim_batch_is_bounded() {
    let db = TestDb::create("claim_bounded").await;
    let store = WorkItemStore;
    let config = test_config();
    for index in 0..20 {
        seed_ready_item(&store, &db.pool, &format!("/inbound/release-{index}")).await;
    }
    let claimed = store
        .claim(&db.pool, &WorkerId::new(), &config, T0)
        .await
        .expect("claim");
    assert_eq!(
        claimed.len(),
        config.claim_batch_size as usize,
        "claim must return at most claim_batch_size items"
    );
    db.drop().await;
}

#[tokio::test]
async fn claim_ordering_is_deterministic() {
    let db = TestDb::create("claim_order").await;
    let store = WorkItemStore;
    let config = test_config();
    let mut ids = Vec::new();
    for index in 0..6 {
        ids.push(seed_ready_item(&store, &db.pool, &format!("/inbound/release-{index:02}")).await);
    }
    let claimed = store
        .claim(&db.pool, &WorkerId::new(), &config, T0)
        .await
        .expect("claim");
    let claimed_ids: Vec<_> = claimed.iter().map(|i| i.id).collect();
    assert_eq!(
        claimed_ids,
        ids[..config.claim_batch_size as usize],
        "oldest first"
    );
    db.drop().await;
}

#[tokio::test]
async fn exhausted_retries_are_not_claimed() {
    let db = TestDb::create("claim_exhausted").await;
    let store = WorkItemStore;
    let config = test_config();
    let policy = RetryPolicy {
        max_attempts: 2,
        ..RetryPolicy::default()
    };
    let id = seed_ready_item(&store, &db.pool, "/inbound/release-1").await;

    let mut now = T0;
    for attempt in 1..=2 {
        let worker = WorkerId::new();
        let claimed = store
            .claim(&db.pool, &worker, &config, now)
            .await
            .expect("claim");
        assert_eq!(claimed.len(), 1, "attempt {attempt} should claim");
        store
            .transition(
                &db.pool,
                id,
                Some(&worker),
                melodee_domain::TransitionKind::Worker,
                IngestionState::Inspecting,
                None,
                now,
            )
            .await
            .expect("start");
        let failed = store
            .record_failure(
                &db.pool,
                id,
                &worker,
                melodee_domain::WorkErrorCode::new(melodee_domain::WorkErrorCode::STAGE_FAILED),
                "transient",
                melodee_domain::FailureClass::Retryable,
                &policy,
                now,
            )
            .await
            .expect("fail");
        now = failed.next_attempt_at.unwrap_or(now);
    }

    // attempt_count == max_attempts: no next_attempt_at, never claimable again
    // without operator action.
    let item = store.get(&db.pool, id).await.expect("get");
    assert_eq!(item.attempt_count, 2);
    assert_eq!(item.next_attempt_at, None);
    let claims = store
        .claim(
            &db.pool,
            &WorkerId::new(),
            &config,
            now + Duration::days(365),
        )
        .await
        .expect("claim after exhaustion");
    assert!(claims.is_empty(), "exhausted work must not be auto-retried");
    db.drop().await;
}
