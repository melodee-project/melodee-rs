//! Protected-attempt epochs on actual processing claim paths.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod catalog_common;
mod common;
use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, PolicyStore, WorkEngineConfig, WorkItemStore, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
async fn epoch(pool: &sqlx::PgPool, id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT protected_attempt_lease_epoch FROM release_work_items WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn reset(pool: &sqlx::PgPool, id: Uuid, state: &str, old: Uuid) {
    sqlx::query("UPDATE release_work_items SET state=$2,lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL,protected_attempt_lease_epoch=$3 WHERE id=$1").bind(id).bind(state).bind(old).execute(pool).await.unwrap();
}
#[tokio::test]
async fn every_processing_claim_rotates_epoch_without_heartbeat_rotation() {
    let db = common::TestDb::create("claim_epoch_paths").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let config = WorkEngineConfig {
        claim_batch_size: 1,
        ..WorkEngineConfig::default()
    };
    // A failed retry is an actual inspection claim, not just a fresh insert.
    let retry = ReleaseWorkItem::discovered("retry".to_owned(), None, Uuid::now_v7(), now);
    store.create(&db.pool, &retry).await.unwrap();
    let previous = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET state='failed',next_attempt_at=$2,attempt_count=1,protected_attempt_lease_epoch=$3 WHERE id=$1").bind(retry.id).bind(now-Duration::seconds(1)).bind(previous).execute(&db.pool).await.unwrap();
    assert_eq!(
        store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let inspection = epoch(&db.pool, retry.id).await.unwrap();
    assert_ne!(inspection, previous);
    store
        .renew_lease(&db.pool, retry.id, &worker, Duration::minutes(5), now)
        .await
        .unwrap();
    assert_eq!(epoch(&db.pool, retry.id).await, Some(inspection));
    reset(&db.pool, retry.id, "normalizing", inspection).await;
    assert_eq!(
        store
            .claim_normalizing(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let normalization = epoch(&db.pool, retry.id).await.unwrap();
    assert_ne!(normalization, inspection);
    reset(&db.pool, retry.id, "validating", normalization).await;
    assert_eq!(
        store
            .claim_validating(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let validation = epoch(&db.pool, retry.id).await.unwrap();
    assert_ne!(validation, normalization);
    reset(&db.pool, retry.id, "discovered", validation).await;

    // The fixture supplies real persisted plan/receipt/catalog inputs for the
    // publication, catalog and two policy claim APIs.
    let published = catalog_common::seed_published_release(
        &db,
        1,
        catalog_common::CatalogFixturePresentation::default(),
    )
    .await;
    let prior = epoch(&db.pool, published.work_item_id).await.unwrap();
    reset(&db.pool, published.work_item_id, "staged", prior).await;
    assert_eq!(
        store
            .claim_publication(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let publication = epoch(&db.pool, published.work_item_id).await.unwrap();
    assert_ne!(publication, prior);
    reset(&db.pool, published.work_item_id, "published", publication).await;
    let catalog = CatalogStore;
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let catalog_epoch = epoch(&db.pool, published.work_item_id).await.unwrap();
    assert_ne!(catalog_epoch, publication);
    catalog
        .import_manifest_v3(
            &db.pool,
            published.work_item_id,
            &worker,
            &CatalogImportRequest::new("epoch.fixture", "epoch-test"),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        epoch(&db.pool, published.work_item_id).await,
        Some(catalog_epoch)
    );
    let policy_catalog = catalog
        .claim_policy_eligible(&db.pool, &worker, &config, now)
        .await
        .unwrap();
    assert_eq!(policy_catalog.len(), 1);
    let catalog_policy = epoch(&db.pool, published.work_item_id).await.unwrap();
    assert_ne!(catalog_policy, catalog_epoch);
    reset(
        &db.pool,
        published.work_item_id,
        "cataloged",
        catalog_policy,
    )
    .await;
    let policy = PolicyStore
        .claim_cataloged(&db.pool, &worker, &config, now)
        .await
        .unwrap();
    assert_eq!(policy.len(), 1);
    let policy_epoch = epoch(&db.pool, published.work_item_id).await.unwrap();
    assert_ne!(policy_epoch, catalog_policy);
    PolicyStore
        .renew_lease(
            &db.pool,
            published.work_item_id,
            &worker,
            Duration::minutes(5),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        epoch(&db.pool, published.work_item_id).await,
        Some(policy_epoch)
    );
    db.drop().await;
}
