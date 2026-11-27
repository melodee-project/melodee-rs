//! Metadata-only guard proofs; filesystem callers still require lifecycle locks.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn expired_intent_stays_fenced_until_explicit_token_reclaim() {
    let db = common::TestDb::create("admin_reserve").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let config = WorkEngineConfig::default();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let reservation = store
        .begin_admin_media_reservation(&db.pool, &[item.id], &[], "operator", Duration::minutes(1))
        .await
        .unwrap();
    assert!(
        store
            .claim(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .begin_admin_media_reservation(&db.pool, &[item.id], &[], "other", Duration::minutes(1))
            .await
            .is_err()
    );
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.unwrap();
    assert!(
        store
            .finish_admin_media_reservation(&db.pool, &reservation)
            .await
            .is_err()
    );
    assert!(
        store
            .claim(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    // Synthetic metadata test: the real caller must already hold all relevant
    // lifecycle locks and establish that prior filesystem work has terminated.
    let reclaimed = store
        .reclaim_admin_media_reservation(
            &db.pool,
            reservation.id(),
            "operator",
            Duration::minutes(1),
        )
        .await
        .unwrap();
    assert!(
        store
            .renew_admin_media_reservation(&db.pool, &reservation, Duration::minutes(1))
            .await
            .is_err()
    );
    assert!(
        store
            .finish_admin_media_reservation(&db.pool, &reservation)
            .await
            .is_err()
    );
    store
        .finish_admin_media_reservation(&db.pool, &reclaimed)
        .await
        .unwrap();
    assert_eq!(
        store
            .claim(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .len(),
        1
    );
    let budget: i64 = sqlx::query_scalar(
        "SELECT reservations FROM admin_media_reservation_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, 0);
    db.drop().await;
}

#[tokio::test]
async fn reservation_and_protected_transition_are_mutually_exclusive() {
    let db = common::TestDb::create("admin_reserve_race").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let config = WorkEngineConfig::default();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let (_, claim) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(
        store
            .begin_admin_media_reservation(
                &db.pool,
                &[item.id],
                &[],
                "operator",
                Duration::minutes(1)
            )
            .await
            .is_err()
    );
    store
        .begin_protected_staging_handoff(
            &db.pool,
            &claim,
            &worker,
            "releases/item/source",
            "generation",
            now,
        )
        .await
        .unwrap();
    store
        .release_protected_lease(&db.pool, &claim, &worker, now)
        .await
        .unwrap();
    assert!(
        store
            .begin_admin_media_reservation(
                &db.pool,
                &[item.id],
                &[],
                "operator",
                Duration::minutes(1)
            )
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_media_reservations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.drop().await;
}

#[tokio::test]
async fn blocked_reservation_cancellation_keeps_budget_and_scope_clean() {
    let db = common::TestDb::create("admin_reserve_cancel").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let oversized: Vec<Uuid> = (0..129).map(|_| Uuid::now_v7()).collect();
    assert!(
        store
            .begin_admin_media_reservation(
                &db.pool,
                &oversized,
                &[],
                "operator",
                Duration::minutes(1)
            )
            .await
            .is_err()
    );
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let item_id = item.id;
    let task = tokio::spawn(async move {
        WorkItemStore
            .begin_admin_media_reservation(&pool, &[item_id], &[], "operator", Duration::minutes(1))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT * FROM release_work_items WHERE id=ANY%')").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.expect("reservation reached row lock");
    let worker = WorkerId::new();
    assert!(
        store
            .claim_with_protected_epoch(&db.pool, &worker, &WorkEngineConfig::default(), now)
            .await
            .unwrap()
            .is_empty()
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    blocker.rollback().await.unwrap();
    let mut verify = db.pool.begin().await.unwrap();
    let (budget,count):(i64,i64)=sqlx::query_as("SELECT reservations,(SELECT count(*) FROM admin_media_reservations) FROM admin_media_reservation_budget WHERE singleton FOR UPDATE").fetch_one(&mut *verify).await.unwrap();
    assert_eq!((budget, count), (0, 0));
    verify.rollback().await.unwrap();
    assert_eq!(
        store
            .claim_with_protected_epoch(&db.pool, &worker, &WorkEngineConfig::default(), now)
            .await
            .unwrap()
            .len(),
        1
    );
    db.drop().await;
}
