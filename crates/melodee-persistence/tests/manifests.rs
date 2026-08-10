//! PostgreSQL proof for atomic manifest publication and durable handoff.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_domain::{IngestionState, ManifestReleaseObservation, ReleaseManifest, TransitionKind};
use melodee_persistence::{WorkItemStore, WorkerId};

#[tokio::test]
async fn manifest_publication_is_durable_and_advances_stage_atomically() {
    let database = common::TestDb::create("manifest-publication").await;
    let store = WorkItemStore;
    let id = common::seed_ready_item(&store, &database.pool, "release/manifest").await;
    let worker = WorkerId::new();
    let claimed = store
        .claim(&database.pool, &worker, &common::test_config(), common::T0)
        .await
        .expect("claim");
    assert_eq!(1, claimed.len());
    store
        .transition(
            &database.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            Some("test-inspection"),
            common::T0,
        )
        .await
        .expect("inspect transition");

    let manifest = ReleaseManifest::new(
        id,
        None,
        "release/manifest".to_owned(),
        Some(1),
        Some("signature".to_owned()),
        Some("8.1.2".to_owned()),
        Some("ffprobe".to_owned()),
        Some(0),
        Some(30_000),
        true,
        ManifestReleaseObservation {
            title: Some("Observed".to_owned()),
            artists: vec!["Artist".to_owned()],
            date: None,
            source_labels: serde_json::json!({}),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    store
        .publish_manifest(
            &database.pool,
            id,
            &worker,
            &manifest,
            "manifest-hash",
            Some(1),
            Some("signature"),
            12,
            common::T0,
        )
        .await
        .expect("publish");

    let item = store.get(&database.pool, id).await.expect("item");
    assert_eq!(IngestionState::Normalizing, item.state);
    assert_eq!(Some("manifest-hash".to_owned()), item.manifest_hash);
    assert!(item.lease_owner.is_none());

    let stored = store
        .get_manifest(&database.pool, id)
        .await
        .expect("manifest lookup")
        .expect("manifest exists");
    assert_eq!(id, stored.work_item_id);
    assert_eq!(1, stored.schema_version);
    assert_eq!("manifest-hash", stored.manifest_hash);
    assert_eq!("Observed", stored.payload["release_observation"]["title"]);

    let events = store.list_events(&database.pool, id).await.expect("events");
    assert!(events.iter().any(|event| {
        event.to_state == Some(IngestionState::Normalizing)
            && event.reason_code.as_deref() == Some("inspection-complete")
    }));

    database.drop().await;
}
