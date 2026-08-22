//! Failure-isolation acceptance test.
//!
//! Proves the official exit criterion: "One failed Release does not block
//! others" (IMPLEMENTATION.md section 5, PRD section 56 Gate B).
//!
//! Scenario: 200 durable Releases become independent work items. Several are
//! deliberately failed (permanent failure). The remaining valid Releases
//! continue independently: they progress through the worker path while the
//! failed items remain explicitly `failed` with durable evidence. No Release
//! disappears, and the global queue does not stop because of the failures.
//!
//! This tests the durable work engine directly rather than manufacturing
//! media failures, keeping the proof at the lowest useful layer.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, test_config};
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{WorkItemStore, WorkerId};
use time::Duration;

#[tokio::test]
async fn one_failed_release_does_not_block_others() {
    let db = TestDb::create("phase2_failure_isolation").await;
    let store = WorkItemStore;
    let config = test_config();

    // 200 durable Releases become independent work items, all ready to claim.
    let mut ids = Vec::with_capacity(200);
    for index in 0..200 {
        let id =
            common::seed_ready_item(&store, &db.pool, &format!("/in/release-{index:03}")).await;
        ids.push(id);
    }
    let all_items = store.list(&db.pool, None, 1000, 0).await.expect("list");
    assert_eq!(
        200,
        all_items.len(),
        "200 candidates became independent work items"
    );
    assert!(
        all_items
            .iter()
            .all(|item| item.state == IngestionState::Ready),
        "all 200 must be ready"
    );

    // A single worker drives the queue. Three items (indices 7, 99, 150)
    // deliberately fail permanently during `inspecting`; the rest advance to
    // `normalizing` (the handoff state proven by the work engine).
    let worker = WorkerId::new();
    let mut now = T0;
    let permanent_failures = [7_usize, 99, 150];

    for (position, id) in ids.iter().enumerate() {
        // Claim advances time so each claim is distinct; bounded batch of 1
        // keeps the failure/progress interleaving explicit.
        now += Duration::seconds(1);
        let config_batch = melodee_persistence::WorkEngineConfig {
            claim_batch_size: 1,
            ..config
        };
        let claimed = store
            .claim(&db.pool, &worker, &config_batch, now)
            .await
            .expect("claim");
        assert_eq!(1, claimed.len(), "one item claimed per iteration");
        let claimed_id = claimed[0].id;
        assert_eq!(&claimed_id, id, "deterministic claim order");

        store
            .transition(
                &db.pool,
                claimed_id,
                Some(&worker),
                TransitionKind::Worker,
                IngestionState::Inspecting,
                None,
                now,
            )
            .await
            .expect("inspect");

        if permanent_failures.contains(&position) {
            store
                .record_failure(
                    &db.pool,
                    claimed_id,
                    &worker,
                    WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
                    "deliberate permanent failure",
                    FailureClass::Permanent,
                    &config.retry_policy,
                    now,
                )
                .await
                .expect("permanent fail");
        } else {
            store
                .transition(
                    &db.pool,
                    claimed_id,
                    Some(&worker),
                    TransitionKind::Worker,
                    IngestionState::Normalizing,
                    None,
                    now,
                )
                .await
                .expect("advance to normalizing");
        }
    }

    // Verify the outcome: failed items remain explicitly failed; valid items
    // progressed; nothing was lost; the queue did not stop.
    let final_items = store
        .list(&db.pool, None, 1000, 0)
        .await
        .expect("final list");
    assert_eq!(200, final_items.len(), "no Release disappeared");

    let failed = final_items
        .iter()
        .filter(|item| item.state == IngestionState::Failed)
        .count();
    assert_eq!(
        permanent_failures.len(),
        failed,
        "exactly the deliberate failures are failed"
    );

    let progressed = final_items
        .iter()
        .filter(|item| item.state == IngestionState::Normalizing)
        .count();
    assert_eq!(
        200 - permanent_failures.len(),
        progressed,
        "all valid Releases progressed independently"
    );

    // Every failed item carries durable, operator-readable failure evidence.
    for &failed_index in &permanent_failures {
        let item = store
            .get(&db.pool, ids[failed_index])
            .await
            .expect("get failed item");
        assert_eq!(item.state, IngestionState::Failed);
        assert_eq!(
            item.last_error_code
                .map(|c| c.as_str().to_string())
                .as_deref(),
            Some(WorkErrorCode::STAGE_FAILED)
        );
        assert_eq!(
            item.last_error_message.as_deref(),
            Some("deliberate permanent failure")
        );
    }

    // The global queue can still serve new work: a freshly seeded item is
    // claimable and progresses, proving the queue did not stop.
    let fresh = common::seed_ready_item(&store, &db.pool, "/in/release-fresh").await;
    now += Duration::seconds(1);
    let claimed = store
        .claim(&db.pool, &worker, &config, now)
        .await
        .expect("claim fresh");
    assert_eq!(1, claimed.len());
    assert_eq!(
        claimed[0].id, fresh,
        "queue still serves new work after failures"
    );

    db.drop().await;
}
