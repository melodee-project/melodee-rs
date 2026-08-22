//! Cross-slice PostgreSQL acceptance tests for the v3 catalog handoff.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    FailureClass, IngestionState, PUBLICATION_MANIFEST_VERSION, RetryPolicy, WorkErrorCode,
};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkItemStore, WorkerId};
use time::Duration;

#[tokio::test]
async fn published_v3_fixture_exposes_complete_catalog_handoff_without_media_files() {
    let db = TestDb::create("catalog_v3_handoff").await;
    let fixture = seed_published_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Catalog Handoff",
            release_artist: "Fixture Artist",
            track_title: "No Media Reread",
            track_artist: "Fixture Artist",
            release_date: "2001-02-03",
        },
    )
    .await;

    let store = WorkItemStore;
    let item = store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read published item");
    let stored_manifest = store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable v3 manifest")
        .expect("manifest exists");
    let publication = store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable publication")
        .expect("publication exists");

    assert_eq!(item.state, IngestionState::Published);
    assert_eq!(
        stored_manifest.schema_version,
        i32::from(PUBLICATION_MANIFEST_VERSION)
    );
    let decoded_manifest: melodee_domain::ReleaseManifest =
        serde_json::from_value(stored_manifest.payload).expect("decode durable v3 manifest");
    assert_eq!(decoded_manifest, fixture.manifest);
    assert_eq!(publication.release_id, fixture.release_id);
    assert_eq!(publication.plan_hash, fixture.plan_hash);
    assert_eq!(
        publication.sealed_manifest_hash.as_deref(),
        Some(fixture.sealed_manifest_hash.as_str())
    );
    assert!(
        !std::path::Path::new(&fixture.source_path).exists(),
        "fixture deliberately provides no readable inbound media"
    );
    assert!(
        !std::path::Path::new(&fixture.canonical_asset_path).exists(),
        "fixture deliberately provides no readable canonical media"
    );

    db.drop().await;
}

#[tokio::test]
async fn import_adopts_reserved_release_id_is_idempotent_and_never_rewrites_v3() {
    let db = TestDb::create("catalog_import_exact_once").await;
    let fixture = seed_published_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Reserved Identity",
            release_artist: "Same Raw Name",
            track_title: "Manifest Only",
            track_artist: "Same Raw Name",
            release_date: "2002-03-04",
        },
    )
    .await;
    let work_store = WorkItemStore;
    let before_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import manifest")
        .expect("pre-import manifest exists");
    let before_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import publication")
        .expect("pre-import publication exists");

    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), catalog_now)
        .await
        .expect("claim catalog import");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, fixture.work_item_id);
    assert_eq!(claimed[0].state, IngestionState::Cataloging);

    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("import manifest v3");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    assert_eq!(imported.track_count, 1);
    assert_eq!(imported.media_asset_count, 1);
    assert_eq!(
        imported.artist_count, 2,
        "equal raw names are separate provisional observations, not name-merged"
    );
    assert!(imported.set_based_mutation_statement_count > 0);

    let cataloged = work_store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read cataloged item");
    assert_eq!(cataloged.state, IngestionState::Cataloged);
    assert!(cataloged.lease_owner.is_none());
    let counts_after_first = catalog.counts(&db.pool).await.expect("catalog counts");
    assert_eq!(counts_after_first.releases, 1);
    assert_eq!(counts_after_first.tracks, 1);
    assert_eq!(counts_after_first.artists, 2);
    assert_eq!(counts_after_first.media_assets, 1);
    assert_eq!(counts_after_first.imports, 1);

    let stored_release: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT id, public_api_id FROM releases WHERE id = $1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read imported Release");
    assert_eq!(stored_release.0, fixture.release_id.into_uuid());
    assert_eq!(
        stored_release.1,
        fixture.release_id.into_uuid(),
        "new public ID starts equal but remains a separate namespace"
    );
    let stored_asset: (String, String, i64) =
        sqlx::query_as("SELECT canonical_relative_path, byte_hash, byte_size FROM media_assets")
            .fetch_one(&db.pool)
            .await
            .expect("read imported canonical asset");
    assert_eq!(
        stored_asset.0,
        format!(
            "{}/{}",
            fixture.plan.target_relative_path, fixture.canonical_asset_path
        )
    );
    assert_eq!(stored_asset.1, fixture.normalized_byte_hash);
    assert_eq!(stored_asset.2, 42);

    let retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("identical uncertain-commit retry");
    assert!(!retry.inserted);
    assert_eq!(retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        counts_after_first
    );

    let conflicting_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase4.different-resolver.v2", "catalog-test"),
            catalog_now,
        )
        .await;
    assert!(conflicting_retry.is_err(), "different binding fails closed");
    assert_eq!(
        catalog.counts(&db.pool).await.expect("conflict counts"),
        counts_after_first
    );

    let after_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import manifest")
        .expect("post-import manifest exists");
    let after_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import publication")
        .expect("post-import publication exists");
    assert_eq!(after_manifest.manifest_hash, before_manifest.manifest_hash);
    assert_eq!(after_manifest.payload, before_manifest.payload);
    assert_eq!(after_publication, before_publication);
    assert!(
        sqlx::query(
            "UPDATE release_work_item_manifests
             SET manifest_hash = $2 WHERE work_item_id = $1",
        )
        .bind(fixture.work_item_id)
        .bind("f".repeat(64))
        .execute(&db.pool)
        .await
        .is_err(),
        "sealed v3 is immutable at the database boundary"
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_lease_expiry_fences_stale_worker_and_direct_reclaim_converges() {
    let db = TestDb::create("catalog_import_lease_fencing").await;
    let fixture = seed_published_release(&db, 3, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let config = test_config();
    let stale_worker = WorkerId::new();
    let winning_worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();

    let first_claim = catalog
        .claim_cataloging(&db.pool, &stale_worker, &config, catalog_now)
        .await
        .expect("initial catalog claim");
    assert_eq!(first_claim.len(), 1);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &winning_worker, &config, catalog_now)
            .await
            .expect("blocked concurrent claim")
            .is_empty(),
        "active lease admits one authoritative worker"
    );

    let after_expiry = catalog_now + config.lease_duration + Duration::seconds(1);
    let reclaimed = catalog
        .claim_cataloging(&db.pool, &winning_worker, &config, after_expiry)
        .await
        .expect("directly reclaim abandoned Cataloging");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, fixture.work_item_id);
    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");

    let stale_result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &stale_worker,
            &request,
            after_expiry,
        )
        .await;
    assert!(
        stale_result.is_err(),
        "stale lease is fenced before catalog writes"
    );
    let empty_counts = catalog.counts(&db.pool).await.expect("post-fence counts");
    assert_eq!(empty_counts.releases, 0);
    assert_eq!(empty_counts.tracks, 0);
    assert_eq!(empty_counts.imports, 0);

    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("winning retry catalogs atomically");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    let final_counts = catalog.counts(&db.pool).await.expect("final counts");
    assert_eq!(final_counts.releases, 1);
    assert_eq!(final_counts.tracks, 1);
    assert_eq!(final_counts.imports, 1);
    let identical_restart_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("completed restart retry finds authoritative result");
    assert!(!identical_restart_retry.inserted);
    assert_eq!(identical_restart_retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        final_counts
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_renewal_cancellation_release_and_retry_are_fenced_and_reclaimable() {
    let db = TestDb::create("catalog_lease_renew_cancel").await;
    let fixture = seed_published_release(&db, 4, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let owner = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let mut config = test_config();
    config.lease_duration = Duration::seconds(2);
    let claimed = catalog
        .claim_cataloging(&db.pool, &owner, &config, now)
        .await
        .expect("claim short catalog lease");
    assert_eq!(claimed.len(), 1);
    let original_expiry = claimed[0].lease_expires_at.expect("initial lease expiry");

    let renewed = catalog
        .renew_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            Duration::seconds(10),
            now + Duration::seconds(1),
        )
        .await
        .expect("renew active catalog lease");
    assert_eq!(renewed.state, IngestionState::Cataloging);
    assert_eq!(renewed.lease_owner.as_deref(), Some(owner.as_str()));
    assert!(renewed.lease_expires_at.expect("renewed expiry") > original_expiry);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
            .await
            .expect("renewal prevents theft")
            .is_empty()
    );

    catalog
        .release_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            now + Duration::seconds(3),
        )
        .await
        .expect("cancellation releases catalog lease");
    let released = WorkItemStore
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read released Cataloging work");
    assert_eq!(released.state, IngestionState::Cataloging);
    assert!(released.lease_owner.is_none());
    assert!(released.lease_expires_at.is_none());
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial rows")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
        .await
        .expect("reclaim cancellation-released work");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.cancel-retry.v1", "catalog-test");
    let result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            now + Duration::seconds(3),
        )
        .await
        .expect("retry after cancellation catalogs atomically");
    assert!(result.inserted);
    assert_eq!(result.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn retryable_catalog_failure_records_backoff_then_direct_reclaim_converges() {
    let db = TestDb::create("catalog_failure_backoff").await;
    let fixture = seed_published_release(&db, 5, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let first_worker = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let config = test_config();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &first_worker, &config, now)
            .await
            .expect("claim catalog work")
            .len(),
        1
    );
    let policy = RetryPolicy {
        base_delay: Duration::seconds(5),
        max_delay: Duration::seconds(5),
        max_attempts: 3,
    };
    let failed = catalog
        .record_failure(
            &db.pool,
            fixture.work_item_id,
            &first_worker,
            WorkErrorCode::new("catalog.synthetic_restart"),
            "simulated process termination before import",
            FailureClass::Retryable,
            &policy,
            now,
        )
        .await
        .expect("record retryable catalog failure");
    assert_eq!(failed.state, IngestionState::Cataloging);
    assert_eq!(failed.attempt_count, 1);
    assert!(failed.lease_owner.is_none());
    let retry_at = failed.next_attempt_at.expect("durable retry deadline");
    assert!(
        catalog
            .claim_cataloging(
                &db.pool,
                &retry_worker,
                &config,
                retry_at - Duration::milliseconds(1),
            )
            .await
            .expect("backoff blocks early reclaim")
            .is_empty()
    );
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial import")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, retry_at)
        .await
        .expect("direct reclaim at retry deadline");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.failure-retry.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("retry catalogs atomically");
    assert!(imported.inserted);
    let identical = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("identical completed retry");
    assert!(!identical.inserted);
    assert_eq!(identical.catalog_import_id, imported.catalog_import_id);
    let event_counts: (i64, i64) = sqlx::query_as(
        "SELECT
            count(*) FILTER (WHERE event_type='failed'),
            count(*) FILTER (WHERE event_type='retry_scheduled')
         FROM release_work_item_events WHERE work_item_id=$1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("query durable retry evidence");
    assert_eq!(event_counts, (1, 1));
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}
