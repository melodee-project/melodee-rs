//! Deterministic PostgreSQL lease/fencing acceptance for policy work.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::IngestionState;
use melodee_persistence::{PolicyStore, WorkItemStore, WorkerId};
use phase5_common::seed_cataloged_release;
use time::Duration;

#[tokio::test]
async fn policy_claim_renew_expire_reclaim_and_release_are_exclusive_and_fenced() {
    let db = TestDb::create("phase5_policy_claim_fencing").await;
    let fixture = seed_cataloged_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Greatest Hits",
            release_artist: "Lease Fixture",
            track_title: "Known Song",
            track_artist: "Lease Fixture",
            release_date: "2005-06-07",
        },
    )
    .await;
    let store = PolicyStore;
    let mut config = test_config();
    config.claim_batch_size = 1;
    config.lease_duration = Duration::seconds(10);
    let first_worker = WorkerId::new();
    let winning_worker = WorkerId::new();
    let final_worker = WorkerId::new();
    let claimed_at = time::OffsetDateTime::now_utc();

    let first = store
        .claim_cataloged(&db.pool, &first_worker, &config, claimed_at)
        .await
        .expect("claim Cataloged policy work");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].item.id, fixture.work_item_id());
    assert_eq!(first[0].release_id, fixture.release_id());
    assert_eq!(first[0].title, "Greatest Hits");
    assert_eq!(
        first[0].source_fingerprint,
        fixture.published.sealed_manifest_hash
    );
    assert_eq!(
        first[0].semantic_manifest_hash,
        fixture.published.semantic_manifest_hash
    );
    assert!(
        store
            .claim_cataloged(&db.pool, &winning_worker, &config, claimed_at)
            .await
            .expect("concurrent claim is bounded")
            .is_empty(),
        "an active policy lease has exactly one owner"
    );

    let renewed_at = claimed_at + Duration::seconds(1);
    let renewed_expiry = store
        .renew_lease(
            &db.pool,
            fixture.work_item_id(),
            &first_worker,
            Duration::seconds(20),
            renewed_at,
        )
        .await
        .expect("renew active policy lease");
    assert_eq!(renewed_expiry, renewed_at + Duration::seconds(20));

    let original_expiry_passed = claimed_at + config.lease_duration + Duration::seconds(1);
    assert!(
        store
            .claim_cataloged(&db.pool, &winning_worker, &config, original_expiry_passed,)
            .await
            .expect("renewal remains authoritative")
            .is_empty(),
        "renewal prevents reclaim at the original expiry"
    );

    let after_renewed_expiry = renewed_expiry + Duration::seconds(1);
    let reclaimed = store
        .claim_cataloged(&db.pool, &winning_worker, &config, after_renewed_expiry)
        .await
        .expect("reclaim expired policy lease");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].item.id, fixture.work_item_id());
    assert!(
        store
            .renew_lease(
                &db.pool,
                fixture.work_item_id(),
                &first_worker,
                config.lease_duration,
                after_renewed_expiry,
            )
            .await
            .is_err(),
        "the stale worker cannot renew after another owner reclaims"
    );
    assert!(
        store
            .release_lease(
                &db.pool,
                fixture.work_item_id(),
                &first_worker,
                after_renewed_expiry,
            )
            .await
            .is_err(),
        "the stale worker cannot release the winner's lease"
    );

    store
        .release_lease(
            &db.pool,
            fixture.work_item_id(),
            &winning_worker,
            after_renewed_expiry,
        )
        .await
        .expect("owner cancels and releases policy lease");
    let immediately_reclaimed = store
        .claim_cataloged(&db.pool, &final_worker, &config, after_renewed_expiry)
        .await
        .expect("released Cataloged work is immediately reclaimable");
    assert_eq!(immediately_reclaimed.len(), 1);

    let durable = WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read claimed policy work");
    assert_eq!(durable.state, IngestionState::Cataloged);
    assert_eq!(durable.lease_owner.as_deref(), Some(final_worker.as_str()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_work_item_events
              WHERE work_item_id = $1 AND reason_code = 'policy.claimed'",
        )
        .bind(fixture.work_item_id())
        .fetch_one(&db.pool)
        .await
        .expect("count durable policy claims"),
        3
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}

#[tokio::test]
async fn expired_policy_claim_recovery_is_bounded_and_directly_reclaimable() {
    let db = TestDb::create("phase5_policy_recovery").await;
    let first = seed_cataloged_release(&db, 1, CatalogFixturePresentation::default()).await;
    let second = seed_cataloged_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Second Policy Fixture",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let store = PolicyStore;
    let mut config = test_config();
    config.claim_batch_size = 2;
    config.lease_duration = Duration::seconds(5);
    let crashed_worker = WorkerId::new();
    let restarted_worker = WorkerId::new();
    let claimed_at = time::OffsetDateTime::now_utc();
    assert_eq!(
        store
            .claim_cataloged(&db.pool, &crashed_worker, &config, claimed_at)
            .await
            .expect("claim two policy fixtures")
            .len(),
        2
    );

    let after_expiry = claimed_at + config.lease_duration + Duration::seconds(1);
    let recovered = store
        .recover_expired_claims(&db.pool, 1, after_expiry)
        .await
        .expect("recover one expired claim");
    assert_eq!(recovered.len(), 1, "recovery honors its explicit bound");
    let reclaim = store
        .claim_cataloged(&db.pool, &restarted_worker, &config, after_expiry)
        .await
        .expect("direct claim also reclaims remaining expired work");
    assert_eq!(reclaim.len(), 2);
    let claimed_ids = reclaim
        .iter()
        .map(|claim| claim.item.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        claimed_ids,
        std::collections::BTreeSet::from([first.work_item_id(), second.work_item_id()])
    );
    first.assert_media_paths_absent();
    second.assert_media_paths_absent();

    db.drop().await;
}
