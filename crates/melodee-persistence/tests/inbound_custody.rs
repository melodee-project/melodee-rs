//! Durable, lease-fenced custody evidence; no filesystem mutations occur here.
#![allow(clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{InboundCustodyIntent, WorkEngineConfig, WorkItemStore, WorkerId};
use time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn custody_intent_and_transitions_require_matching_fenced_evidence() {
    let db = common::TestDb::create("custody_intent_fence").await;
    let store = WorkItemStore;
    let mut item =
        ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), common::T0);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), common::T0)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);
    let hash = format!("v2:{}", "a".repeat(64));
    let legacy_hash = "a".repeat(64);
    let intent = InboundCustodyIntent {
        work_item_id: item.id,
        source_library_id: None,
        source_path: "candidate",
        source_root_generation: "unix:1:1",
        source_generation: "unix:1:2",
        preclaim_tree_hash: &hash,
        preclaim_legacy_tree_hash: &legacy_hash,
        now: common::T0,
    };
    let initial = store
        .begin_inbound_custody(&db.pool, &worker, &intent)
        .await
        .expect("intent");
    assert_eq!(initial.status, "intent");
    assert!(
        store
            .is_pending_custody_name(&db.pool, None, &initial.custody_name)
            .await
            .expect("reserved")
    );
    for status in ["replacement_preserved", "replacement_restored"] {
        assert!(
            store
                .advance_inbound_custody(
                    &db.pool,
                    &worker,
                    item.id,
                    status,
                    "unix:1:1",
                    "unix:1:2",
                    &hash,
                    common::T0
                )
                .await
                .is_err(),
            "replacement outcome requires a recoverable filesystem observation"
        );
    }
    assert_eq!(
        store
            .inbound_custody(&db.pool, item.id)
            .await
            .expect("query")
            .expect("intent")
            .status,
        "intent"
    );
    assert!(
        store
            .advance_inbound_custody(
                &db.pool,
                &worker,
                item.id,
                "held",
                "unix:1:1",
                "unix:1:3",
                &hash,
                common::T0
            )
            .await
            .is_err()
    );
    assert!(
        store
            .advance_inbound_custody(
                &db.pool,
                &worker,
                item.id,
                "held",
                "unix:1:9",
                "unix:1:2",
                &hash,
                common::T0
            )
            .await
            .is_err(),
        "a different source root cannot advance custody"
    );
    let held = store
        .advance_inbound_custody(
            &db.pool,
            &worker,
            item.id,
            "held",
            "unix:1:1",
            "unix:1:2",
            &hash,
            common::T0,
        )
        .await
        .expect("held");
    assert_eq!(held.status, "held");
    assert!(
        store
            .advance_inbound_custody(
                &db.pool,
                &worker,
                item.id,
                "staged",
                "unix:1:1",
                "unix:1:2",
                &hash,
                common::T0
            )
            .await
            .is_err(),
        "receipt is mandatory"
    );
    assert!(
        store
            .advance_inbound_custody(
                &db.pool,
                &WorkerId::new(),
                item.id,
                "held",
                "unix:1:1",
                "unix:1:2",
                &hash,
                common::T0
            )
            .await
            .is_err(),
        "other owner cannot advance"
    );
    assert!(
        store
            .advance_inbound_custody(
                &db.pool,
                &worker,
                item.id,
                "held",
                "unix:1:1",
                "unix:1:2",
                &hash,
                common::T0 + Duration::days(1)
            )
            .await
            .is_err(),
        "expired lease cannot advance"
    );
    db.drop().await;
}
