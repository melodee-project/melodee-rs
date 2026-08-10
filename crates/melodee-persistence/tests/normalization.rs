//! PostgreSQL proof for normalization claim and publication.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_domain::{
    IngestionState, ManifestReleaseObservation, NormalizationExecutionProvenance,
    NormalizationObservations, ReleaseManifest, TransitionKind,
};
use melodee_persistence::{WorkItemStore, WorkerId};

async fn seed_normalizing_item(
    store: &WorkItemStore,
    pool: &sqlx::PgPool,
    path: &str,
) -> (uuid::Uuid, ReleaseManifest) {
    let id = common::seed_ready_item(store, pool, path).await;
    let worker = WorkerId::new();
    store
        .claim(pool, &worker, &common::test_config(), common::T0)
        .await
        .expect("claim");
    store
        .transition(
            pool,
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
        path.to_owned(),
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
            pool,
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
    (id, manifest)
}

async fn seed_normalizing_with_manifest(
    store: &WorkItemStore,
    pool: &sqlx::PgPool,
    path: &str,
) -> (uuid::Uuid, WorkerId) {
    let (id, _manifest) = seed_normalizing_item(store, pool, path).await;
    let worker = WorkerId::new();
    // After publish_manifest, the item is in 'normalizing' with no lease.
    // Claim it for normalization.
    store
        .claim_normalizing(pool, &worker, &common::test_config(), common::T0)
        .await
        .expect("claim normalizing");
    (id, worker)
}

#[tokio::test]
async fn claim_normalizing_acquires_lease_on_normalizing_item() {
    let database = common::TestDb::create("claim-normalizing").await;
    let store = WorkItemStore;
    let (id, _worker) =
        seed_normalizing_with_manifest(&store, &database.pool, "release/test").await;

    let worker2 = WorkerId::new();
    // The first worker's claim should have acquired a lease.
    // Let's verify by re-claiming with a different worker - should get nothing.
    let config = common::test_config();
    let items = store
        .claim_normalizing(&database.pool, &worker2, &config, common::T0)
        .await
        .expect("claim_normalizing");
    assert!(
        items.is_empty(),
        "no items should be claimable by other worker"
    );

    // Verify the item has an active lease
    let item = store.get(&database.pool, id).await.expect("get item");
    assert_eq!(IngestionState::Normalizing, item.state);
    assert!(item.lease_owner.is_some());

    database.drop().await;
}

#[tokio::test]
async fn publish_normalization_advances_to_validating() {
    let database = common::TestDb::create("publish-normalization").await;
    let store = WorkItemStore;
    let (id, _) = seed_normalizing_item(&store, &database.pool, "release/test").await;

    let worker = WorkerId::new();
    let claimed = store
        .claim_normalizing(&database.pool, &worker, &common::test_config(), common::T0)
        .await
        .expect("claim normalizing");
    assert_eq!(1, claimed.len());

    // Build a manifest with normalization observations
    let mut manifest = ReleaseManifest::new(
        id,
        None,
        "release/test".to_owned(),
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
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "1.0".to_owned(),
        outputs: Vec::new(),
        fingerprints: Vec::new(),
        normalized_artwork: Vec::new(),
        parsed_sidecars: Vec::new(),
        provenance: NormalizationExecutionProvenance {
            worker_id: Some(worker.as_str().to_owned()),
            ..Default::default()
        },
        findings: Vec::new(),
    });

    store
        .publish_normalization(
            &database.pool,
            id,
            &worker,
            &manifest,
            "normalized-hash",
            "release/test",
            5000,
            common::T0,
        )
        .await
        .expect("publish normalization");

    let item = store.get(&database.pool, id).await.expect("get item");
    assert_eq!(IngestionState::Validating, item.state);
    assert_eq!(Some("normalized-hash".to_owned()), item.manifest_hash);
    assert!(item.lease_owner.is_none());

    let stored = store
        .get_manifest(&database.pool, id)
        .await
        .expect("get manifest")
        .expect("manifest exists");
    assert!(
        stored.payload.get("normalization").is_some(),
        "manifest should contain normalization observations"
    );

    database.drop().await;
}

#[tokio::test]
async fn publish_normalization_rejects_stale_lease() {
    let database = common::TestDb::create("stale-normalization").await;
    let store = WorkItemStore;
    let (id, _) = seed_normalizing_item(&store, &database.pool, "release/test").await;

    let worker1 = WorkerId::new();
    let worker2 = WorkerId::new();

    // Worker 1 claims
    store
        .claim_normalizing(&database.pool, &worker1, &common::test_config(), common::T0)
        .await
        .expect("claim normalizing");

    // Worker 2 cannot claim (worker 1 holds the lease)
    let items = store
        .claim_normalizing(&database.pool, &worker2, &common::test_config(), common::T0)
        .await
        .expect("claim normalizing");
    assert!(items.is_empty());

    // Worker 2 tries to publish anyway - should fail with NotLeaseOwner
    let manifest = ReleaseManifest::new(
        id,
        None,
        "release/test".to_owned(),
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
    let result = store
        .publish_normalization(
            &database.pool,
            id,
            &worker2,
            &manifest,
            "stale-hash",
            "release/test",
            5000,
            common::T0,
        )
        .await;
    assert!(result.is_err());

    database.drop().await;
}
