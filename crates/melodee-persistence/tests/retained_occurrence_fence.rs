//! Occurrence-level fences survive deletion of the original work item.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#[path = "common/mod.rs"]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkStoreError, WorkerId};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn retained_occurrence_blocks_discovery_creation_and_fresh_handoff_after_original_deletion() {
    let db = common::TestDb::create("retained_occurrence_fence").await;
    let now = OffsetDateTime::now_utc();
    let original = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore
        .create(&db.pool, &original)
        .await
        .expect("original");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'candidate','generation',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(original.id).bind(format!("v3:{}", "a".repeat(64))).execute(&db.pool).await.expect("obligation");
    sqlx::query("DELETE FROM release_work_items WHERE id=$1")
        .bind(original.id)
        .execute(&db.pool)
        .await
        .expect("delete original");
    let new_item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    assert!(matches!(
        WorkItemStore.create(&db.pool, &new_item).await,
        Err(WorkStoreError::RetainedFreshIntakeRequired)
    ));
    assert!(matches!(
        WorkItemStore.create_or_get(&db.pool, &new_item).await,
        Err(WorkStoreError::RetainedFreshIntakeRequired)
    ));
    assert!(
        WorkItemStore
            .inbound_occurrence_is_acquired(
                &db.pool,
                None,
                "candidate",
                "different-generation",
                now
            )
            .await
            .expect("retained hold")
    );
    // Simulate an item created through older code before this guard existed.
    let worker = WorkerId::new();
    sqlx::query("INSERT INTO release_work_items(id,api_key,source_path,current_path,state,attempt_count,correlation_id,discovered_at,updated_at,created_at,lease_owner,lease_acquired_at,lease_expires_at) VALUES($1,$2,'candidate','candidate','ready',0,$3,$4,$4,$4,$5,$4,$6)")
        .bind(new_item.id).bind(new_item.api_key).bind(new_item.correlation_id).bind(now).bind(worker.as_str()).bind(now+Duration::minutes(5)).execute(&db.pool).await.expect("preexisting work");
    assert!(matches!(
        WorkItemStore
            .begin_staging_handoff(
                &db.pool,
                new_item.id,
                &worker,
                "releases/test/source",
                "generation",
                now
            )
            .await,
        Err(WorkStoreError::RetainedFreshIntakeRequired)
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_staging_handoffs WHERE work_item_id=$1",
    )
    .bind(new_item.id)
    .fetch_one(&db.pool)
    .await
    .expect("handoffs");
    assert_eq!(count, 0);
    db.drop().await;
}

#[tokio::test]
async fn purge_reservation_and_claim_are_exclusive_in_both_orders() {
    let db = common::TestDb::create("retained_purge_claim").await;
    let now = OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("candidate".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &item).await.expect("item");
    let guard = WorkItemStore
        .begin_pre_catalog_purge(&db.pool, item.id, "operator")
        .await
        .expect("guard");
    let worker = WorkerId::new();
    let claimed = WorkItemStore
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("guarded claim");
    assert!(claimed.is_empty());
    assert!(
        WorkItemStore
            .begin_pre_catalog_purge(&db.pool, item.id, "operator")
            .await
            .is_err()
    );
    drop(guard); // A crash/drop must leave the durable guard intact.
    let guard_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_occurrence_purge_guards WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .expect("guard count");
    assert_eq!(guard_count, 1);
    assert!(
        WorkItemStore
            .purge_pre_catalog_work_item(
                &db.pool,
                item.id,
                "operator",
                "test",
                serde_json::json!({}),
                now
            )
            .await
            .is_err()
    );

    let mut other = ReleaseWorkItem::discovered("other".to_owned(), None, Uuid::now_v7(), now);
    other.state = IngestionState::Ready;
    WorkItemStore.create(&db.pool, &other).await.expect("other");
    let claimed = WorkItemStore
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("claim other");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, other.id);
    assert!(
        WorkItemStore
            .begin_pre_catalog_purge(&db.pool, other.id, "operator")
            .await
            .is_err(),
        "a live processing lease blocks purge"
    );
    let original_expiry: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(other.id)
            .fetch_one(&db.pool)
            .await
            .expect("lease expiry");
    let mut reservation_tx = db.pool.begin().await.expect("reservation transaction");
    sqlx::query("SELECT lock_inbound_occurrence(NULL,'other')")
        .execute(&mut *reservation_tx)
        .await
        .expect("occurrence lock");
    let renewal = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        WorkItemStore.renew_lease(&db.pool, other.id, &worker, Duration::minutes(20), now),
    )
    .await
    .expect("renewal must not deadlock with occurrence reservation");
    assert!(renewal.is_err(), "busy occurrence must not renew the lease");
    reservation_tx
        .rollback()
        .await
        .expect("release reservation");
    let expiry_after: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(other.id)
            .fetch_one(&db.pool)
            .await
            .expect("lease expiry");
    assert_eq!(expiry_after, original_expiry);
    let mut racing = ReleaseWorkItem::discovered("racing".to_owned(), None, Uuid::now_v7(), now);
    racing.state = IngestionState::Ready;
    WorkItemStore
        .create(&db.pool, &racing)
        .await
        .expect("racing item");
    let mut purge_tx = db.pool.begin().await.expect("purge transaction");
    sqlx::query("SELECT lock_inbound_occurrence(NULL,'racing')")
        .execute(&mut *purge_tx)
        .await
        .expect("occurrence lock");
    let pool = db.pool.clone();
    let worker_clone = WorkerId::new();
    let claim = tokio::spawn(async move {
        WorkItemStore
            .claim(&pool, &worker_clone, &WorkEngineConfig::default(), now)
            .await
    });
    let rows = tokio::time::timeout(std::time::Duration::from_secs(3), claim)
        .await
        .expect("claim must not deadlock")
        .expect("join")
        .expect("claim");
    assert!(
        rows.is_empty(),
        "a busy occurrence is skipped rather than claimed"
    );
    sqlx::query("INSERT INTO inbound_occurrence_purge_guards(work_item_id,source_path,guard_owner,fence_token) VALUES($1,'racing','operator',$2)")
        .bind(racing.id).bind(Uuid::now_v7()).execute(&mut *purge_tx).await.expect("guard insert");
    purge_tx.commit().await.expect("guard commit");
    assert!(
        WorkItemStore
            .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
            .await
            .expect("guarded claim")
            .is_empty()
    );
    let guards = WorkItemStore
        .list_pre_catalog_purge_guards(&db.pool, None)
        .await
        .expect("guard list");
    let saved = guards
        .iter()
        .find(|guard| guard.work_item_id == item.id)
        .expect("saved guard");
    let attestation = melodee_persistence::PurgeGuardRecoveryAttestation {
        writers_stopped: true,
        storage_inspected: true,
        backup_verified: true,
    };
    let missing_attestation = melodee_persistence::PurgeGuardRecoveryAttestation {
        backup_verified: false,
        ..attestation
    };
    assert!(
        WorkItemStore
            .recover_pre_catalog_purge_guard(
                &db.pool,
                item.id,
                saved.fence_token,
                &saved.guard_owner,
                "operator",
                "reviewed storage",
                missing_attestation
            )
            .await
            .is_err()
    );
    assert!(
        WorkItemStore
            .recover_pre_catalog_purge_guard(
                &db.pool,
                item.id,
                Uuid::now_v7(),
                &saved.guard_owner,
                "operator",
                "reviewed storage",
                attestation
            )
            .await
            .is_err()
    );
    assert!(
        WorkItemStore
            .recover_pre_catalog_purge_guard(
                &db.pool,
                item.id,
                saved.fence_token,
                "wrong-owner",
                "operator",
                "reviewed storage",
                attestation
            )
            .await
            .is_err()
    );
    WorkItemStore
        .recover_pre_catalog_purge_guard(
            &db.pool,
            item.id,
            saved.fence_token,
            &saved.guard_owner,
            "operator",
            "reviewed storage",
            attestation,
        )
        .await
        .expect("audited recovery");
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_audit_events WHERE entity_id=$1 AND event_type='work_item_purge_guard_released'")
        .bind(item.id).fetch_one(&db.pool).await.expect("recovery audit");
    assert_eq!(audit, 1);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(item.id)
        .fetch_one(&db.pool)
        .await
        .expect("item remains");
    assert_eq!(remaining, 1, "guard recovery must not purge work or media");
    db.drop().await;
}

#[tokio::test]
async fn purge_guard_allows_reused_path_with_inactive_historical_work() {
    let db = common::TestDb::create("retained_purge_historical_path").await;
    let now = OffsetDateTime::now_utc();
    let historical = ReleaseWorkItem::discovered("reused".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore
        .create(&db.pool, &historical)
        .await
        .expect("historical");
    sqlx::query("UPDATE release_work_items SET inbound_active=false,lease_owner='old-worker',lease_acquired_at=clock_timestamp(),lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE id=$1")
        .bind(historical.id).execute(&db.pool).await.expect("historical inactive lease");
    let current = ReleaseWorkItem::discovered("reused".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore
        .create(&db.pool, &current)
        .await
        .expect("current");
    let guard = WorkItemStore
        .begin_pre_catalog_purge(&db.pool, current.id, "operator")
        .await
        .expect("inactive history does not prevent purge");
    drop(guard);
    let saved = WorkItemStore
        .list_pre_catalog_purge_guards(&db.pool, None)
        .await
        .expect("guards")
        .into_iter()
        .find(|guard| guard.work_item_id == current.id)
        .expect("saved guard");
    WorkItemStore
        .recover_pre_catalog_purge_guard(
            &db.pool,
            current.id,
            saved.fence_token,
            &saved.guard_owner,
            "operator",
            "reviewed storage",
            melodee_persistence::PurgeGuardRecoveryAttestation {
                writers_stopped: true,
                storage_inspected: true,
                backup_verified: true,
            },
        )
        .await
        .expect("inactive historical lease does not prevent guard recovery");
    let guard = WorkItemStore
        .begin_pre_catalog_purge(&db.pool, current.id, "operator")
        .await
        .expect("reserve after recovery");
    WorkItemStore
        .purge_pre_catalog_work_item_guarded(
            &db.pool,
            &guard,
            "operator",
            "reviewed",
            serde_json::json!({}),
            now,
        )
        .await
        .expect("guarded finalization");
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE id=$1")
        .bind(historical.id)
        .fetch_one(&db.pool)
        .await
        .expect("history");
    assert_eq!(history, 1);
    db.drop().await;
}
