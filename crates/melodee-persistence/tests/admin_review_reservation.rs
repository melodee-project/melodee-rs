//! Reservation-bound review completion is DB-only; no filesystem claim is made.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::WorkItemStore;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn attestation() -> melodee_persistence::LifecycleQuiescenceAttestation<'static> {
    melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "verified test maintenance",
        backup_reference: "test-backup",
        stop_all_writers: true,
        inspection_complete: true,
    }
}

#[tokio::test]
async fn review_completion_consumes_exact_token_and_replays_after_lost_ack() {
    let db = common::TestDb::create("admin_review_reserved").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::NeedsReview;
    store.create(&db.pool, &item).await.unwrap();
    let old = store
        .begin_admin_media_reservation(&db.pool, &[item.id], &[], "operator", Duration::minutes(1))
        .await
        .unwrap();
    store
        .authorize_unpublished_delete_reserved(
            &db.pool,
            item.id,
            &old,
            Uuid::now_v7(),
            "administrator",
            Some("reason"),
            now,
        )
        .await
        .unwrap();
    assert!(
        store
            .complete_unpublished_delete(&db.pool, item.id, now)
            .await
            .is_err()
    );
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(old.id()).execute(&db.pool).await.unwrap();
    let current = store
        .reclaim_admin_media_reservation(&db.pool, old.id(), "operator", Duration::minutes(1))
        .await
        .unwrap();
    assert!(
        store
            .complete_unpublished_delete_reserved(&db.pool, item.id, &old, now)
            .await
            .is_err()
    );
    store
        .authorize_unpublished_delete_reserved(
            &db.pool,
            item.id,
            &current,
            Uuid::now_v7(),
            "administrator",
            Some("reason"),
            now,
        )
        .await
        .unwrap();
    let result = store
        .complete_unpublished_delete_reserved(&db.pool, item.id, &current, now)
        .await
        .unwrap();
    assert_eq!(result.state, IngestionState::Deleted);
    assert_eq!(
        store
            .complete_unpublished_delete_reserved(&db.pool, item.id, &current, now)
            .await
            .unwrap()
            .state,
        IngestionState::Deleted
    );
    assert!(
        store
            .complete_unpublished_delete_reserved(&db.pool, item.id, &old, now)
            .await
            .is_err()
    );
    let (reservations,events,leaked):(i64,i64,bool)=sqlx::query_as("SELECT (SELECT reservations FROM admin_media_reservation_budget WHERE singleton),(SELECT count(*) FROM release_work_item_events WHERE work_item_id=$1 AND reason_code='review.delete_completed'),EXISTS(SELECT 1 FROM release_work_item_events WHERE work_item_id=$1 AND metadata ? 'admin_reservation_token')").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!((reservations, events, leaked), (0, 1, false));
    db.drop().await;
}

#[tokio::test]
async fn cancelled_review_completion_rolls_back_item_and_guard_together() {
    let db = common::TestDb::create("admin_review_cancel").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::NeedsReview;
    store.create(&db.pool, &item).await.unwrap();
    let reservation = store
        .begin_admin_media_reservation(&db.pool, &[item.id], &[], "operator", Duration::minutes(1))
        .await
        .unwrap();
    store
        .authorize_unpublished_delete_reserved(
            &db.pool,
            item.id,
            &reservation,
            Uuid::now_v7(),
            "administrator",
            None,
            now,
        )
        .await
        .unwrap();
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_review_guard_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN OLD; END $$; CREATE TRIGGER test_review_guard_stall BEFORE DELETE ON admin_media_reservation_items FOR EACH ROW EXECUTE FUNCTION test_review_guard_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let held = reservation.clone();
    let id = item.id;
    let task = tokio::spawn(async move {
        WorkItemStore
            .complete_unpublished_delete_reserved(&pool, id, &held, now)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'DELETE FROM admin_media_reservation_items%')").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.expect("completion reached guard deletion");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let mut verify = db.pool.begin().await.unwrap();
    let (state,count,completed):(String,i64,bool)=sqlx::query_as("SELECT w.state,(SELECT count(*) FROM admin_media_reservation_items WHERE work_item_id=w.id),(SELECT completed_at IS NOT NULL FROM review_deletion_authorizations WHERE work_item_id=w.id) FROM release_work_items w WHERE w.id=$1 FOR UPDATE").bind(item.id).fetch_one(&mut *verify).await.unwrap();
    assert_eq!(
        (state, count, completed),
        ("needs_review".to_owned(), 1, false)
    );
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}

#[tokio::test]
async fn maintenance_reservation_keeps_global_fence_until_atomic_review_completion() {
    let db = common::TestDb::create("admin_review_maintenance").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::NeedsReview;
    store.create(&db.pool, &item).await.unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(&db.pool, &fence, &attestation())
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
    assert!(
        store
            .begin_maintenance_admin_media_reservation(
                &db.pool,
                &[item.id],
                &[],
                "other",
                Duration::minutes(1),
                &fence
            )
            .await
            .is_err()
    );
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[item.id],
            &[],
            "operator",
            Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .reserve_work_item_lifecycle(&db.pool, &fence, item.id)
        .await
        .unwrap();
    store
        .acknowledge_work_item_lifecycle(
            &db.pool,
            &fence,
            item.id,
            "lock-root",
            "lock-inode",
            "stage-root",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &fence)
            .await
            .unwrap(),
        None
    );
    assert!(
        store
            .finish_lifecycle_maintenance(&db.pool, &fence)
            .await
            .is_err()
    );
    store
        .authorize_unpublished_delete_reserved(
            &db.pool,
            item.id,
            &reservation,
            Uuid::now_v7(),
            "administrator",
            None,
            now,
        )
        .await
        .unwrap();
    store
        .complete_unpublished_delete_reserved(&db.pool, item.id, &reservation, now)
        .await
        .unwrap();
    store
        .finish_lifecycle_maintenance(&db.pool, &fence)
        .await
        .unwrap();
    db.drop().await;
}
