//! Durable protected-attempt metadata does not prove filesystem ownership.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    ProtectedStageIdentity, WorkEngineConfig, WorkItemStore, WorkStoreError, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn protected_attempts_fence_same_owner_aba_and_quarantine_ambiguous_namespaces() {
    let db = common::TestDb::create("protected_epoch").await;
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
    let (claimed, epoch) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert_eq!(claimed.id, item.id);
    store
        .begin_protected_staging_handoff(
            &db.pool,
            &epoch,
            &worker,
            "releases/item/source",
            "src-gen",
            now,
        )
        .await
        .unwrap();
    let reserved = store
        .reserve_protected_stage_attempt(
            &db.pool,
            &epoch,
            "root-gen",
            "src-gen",
            "releases/item/source",
        )
        .await
        .unwrap();
    assert!(reserved.namespace_name().starts_with("protected-"));
    assert!(!reserved.is_created());
    store
        .validate_reserved_protected_stage_attempt(&db.pool, &epoch, &reserved)
        .await
        .unwrap();
    assert!(
        store
            .acknowledge_protected_stage_created(
                &db.pool,
                &epoch,
                &reserved,
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "",
                    marker_generation: "marker",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path='wrong' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .acknowledge_protected_stage_created(
                &db.pool,
                &epoch,
                &reserved,
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:1",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path='releases/item/source' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let created = store
        .acknowledge_protected_stage_created(
            &db.pool,
            &epoch,
            &reserved,
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "namespace:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    assert!(created.is_created());
    assert!(
        store
            .validate_reserved_protected_stage_attempt(&db.pool, &epoch, &reserved)
            .await
            .is_err()
    );
    store
        .acknowledge_protected_stage_created(
            &db.pool,
            &epoch,
            &created,
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "namespace:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .acknowledge_protected_stage_created(
                &db.pool,
                &epoch,
                &created,
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:2",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    let ambiguous = store
        .reserve_protected_stage_attempt(
            &db.pool,
            &epoch,
            "root-gen",
            "src-gen",
            "releases/item/source",
        )
        .await
        .unwrap();
    sqlx::query("UPDATE inbound_protected_attempt_budget SET max_attempts=2 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .reserve_protected_stage_attempt(
                &db.pool,
                &epoch,
                "root-gen",
                "src-gen",
                "releases/item/source"
            )
            .await,
        Err(WorkStoreError::RetainedSourceBudgetExhausted)
    ));
    let before: Uuid = sqlx::query_scalar(
        "SELECT protected_attempt_lease_epoch FROM release_work_items WHERE id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    store
        .renew_protected_claim_epoch(&db.pool, &epoch, Duration::minutes(5))
        .await
        .unwrap();
    let renewed: Uuid = sqlx::query_scalar(
        "SELECT protected_attempt_lease_epoch FROM release_work_items WHERE id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(before, renewed);
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, replacement) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    let after: Uuid = sqlx::query_scalar(
        "SELECT protected_attempt_lease_epoch FROM release_work_items WHERE id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_ne!(before, after);
    let acquired: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_acquired_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(acquired, now);
    assert!(matches!(
        store
            .renew_protected_claim_epoch(&db.pool, &epoch, Duration::minutes(1))
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    assert!(matches!(
        store
            .acknowledge_protected_stage_created(
                &db.pool,
                &epoch,
                &created,
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:1",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path='wrong' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .adopt_created_protected_stage_attempt(
                &db.pool,
                &replacement,
                created.id(),
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:1",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path='releases/item/source' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let candidates = store
        .list_created_protected_stage_attempts(&db.pool, &replacement, None, 16)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id(), created.id());
    assert!(
        store
            .adopt_created_protected_stage_attempt(
                &db.pool,
                &replacement,
                ambiguous.id(),
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:1",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .adopt_created_protected_stage_attempt(
                &db.pool,
                &replacement,
                created.id(),
                &ProtectedStageIdentity {
                    root_generation: "root-gen",
                    namespace_generation: "namespace:bad",
                    marker_generation: "marker:1",
                    lock_generation: "lock:1"
                }
            )
            .await
            .is_err()
    );
    let adopted = store
        .adopt_created_protected_stage_attempt(
            &db.pool,
            &replacement,
            created.id(),
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "namespace:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    assert_eq!(adopted.id(), created.id());
    assert_eq!(adopted.source_path(), "source");
    assert_eq!(adopted.final_binding(), "releases/item/source");
    let intent = store
        .begin_protected_stage_promotion(
            &db.pool,
            &replacement,
            &adopted,
            "child:1",
            "final-parent:1",
        )
        .await
        .unwrap();
    assert_eq!(intent.state(), "promotion_intent");
    assert_eq!(
        store
            .begin_protected_stage_promotion(
                &db.pool,
                &replacement,
                &adopted,
                "child:1",
                "final-parent:1"
            )
            .await
            .unwrap()
            .state(),
        "promotion_intent"
    );
    assert!(
        store
            .acknowledge_protected_stage_promotion(
                &db.pool,
                &replacement,
                &intent,
                "different-child",
                "final-parent:1"
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, resumed) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(matches!(
        store
            .acknowledge_protected_stage_promotion(
                &db.pool,
                &replacement,
                &intent,
                "child:1",
                "final-parent:1"
            )
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    let recovered = store
        .list_created_protected_stage_attempts(&db.pool, &resumed, None, 16)
        .await
        .unwrap();
    assert_eq!(recovered[0].state(), "promotion_intent");
    let adopted = store
        .adopt_created_protected_stage_attempt(
            &db.pool,
            &resumed,
            created.id(),
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "namespace:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    let promoted = store
        .acknowledge_protected_stage_promotion(
            &db.pool,
            &resumed,
            &adopted,
            "child:1",
            "final-parent:1",
        )
        .await
        .unwrap();
    assert_eq!(promoted.state(), "promoted");
    assert_eq!(
        store
            .acknowledge_protected_stage_promotion(
                &db.pool,
                &resumed,
                &promoted,
                "child:1",
                "final-parent:1"
            )
            .await
            .unwrap()
            .state(),
        "promoted"
    );
    sqlx::query("UPDATE release_work_items SET state='normalizing',lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL WHERE id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .claim_normalizing(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    let normalized = store
        .claim_normalizing_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap();
    assert_eq!(normalized.len(), 1);
    let cross: Uuid = sqlx::query_scalar(
        "SELECT protected_attempt_lease_epoch FROM release_work_items WHERE id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_ne!(after, cross);
    assert!(matches!(
        store
            .renew_protected_claim_epoch(&db.pool, &replacement, Duration::minutes(1))
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT attempts FROM inbound_protected_attempt_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
    db.drop().await;
}

async fn wait_for_item_lock(pool: &sqlx::PgPool) {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop{let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT lease_owner,protected_attempt_lease_epoch%')").fetch_one(pool).await.unwrap();if waiting{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
}
#[tokio::test]
async fn protected_heartbeat_cancellation_and_blocked_expiry_do_not_renew() {
    let db = common::TestDb::create("protected_heartbeat").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let config = WorkEngineConfig {
        lease_duration: Duration::seconds(2),
        ..WorkEngineConfig::default()
    };
    let (_, epoch) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let old = epoch.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .renew_protected_claim_epoch(&pool, &old, Duration::minutes(1))
            .await
    });
    wait_for_item_lock(&db.pool).await;
    task.abort();
    let _ = task.await;
    lock.commit().await.unwrap();
    let expiry: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(expiry, now + Duration::seconds(2));
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let old = epoch.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .renew_protected_claim_epoch(&pool, &old, Duration::minutes(1))
            .await
    });
    wait_for_item_lock(&db.pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    lock.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(WorkStoreError::LeaseExpired(_))
    ));
    let unchanged: OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(unchanged, expiry);
    db.drop().await;
}

#[tokio::test]
async fn migrated_active_lease_without_epoch_fails_closed_until_new_claim() {
    let db = common::TestDb::create("protected_upgrade").await;
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
    let (_, old) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    store
        .begin_staging_handoff(
            &db.pool,
            item.id,
            &worker,
            "releases/item/source",
            "src-gen",
            now,
        )
        .await
        .unwrap();
    // Models an active lease populated before migration 0071 introduced epochs.
    sqlx::query("UPDATE release_work_items SET protected_attempt_lease_epoch=NULL WHERE id=$1")
        .bind(item.id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .reserve_protected_stage_attempt(
                &db.pool,
                &old,
                "root",
                "src-gen",
                "releases/item/source"
            )
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    assert!(matches!(
        store
            .renew_protected_claim_epoch(&db.pool, &old, Duration::minutes(1))
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, new) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    // An existing ordinary custody intent is not silently converted into a
    // protected attempt after reclaim; its prior worker may still have handles.
    assert!(
        store
            .begin_protected_staging_handoff(
                &db.pool,
                &new,
                &worker,
                "releases/item/source",
                "src-gen",
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .reserve_protected_stage_attempt(
                &db.pool,
                &new,
                "root",
                "src-gen",
                "releases/item/source"
            )
            .await
            .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn populated_upgrade_preserves_active_legacy_lease_without_fabricating_epoch() {
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;
    let db = common::TestDb::create_unmigrated("protected_upgrade_real").await;
    let prior: Vec<_> = melodee_persistence::MIGRATOR
        .migrations
        .iter()
        .filter(|m| m.version <= 70)
        .cloned()
        .collect();
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .unwrap();
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    sqlx::query("INSERT INTO release_work_items(id,api_key,source_path,current_path,state,attempt_count,correlation_id,discovered_at,updated_at,created_at) VALUES($1,$2,'source','source','discovered',0,$3,$4,$4,$4)").bind(item.id).bind(item.api_key).bind(item.correlation_id).bind(now).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE release_work_items SET state='ready',lease_owner='existing-worker',lease_acquired_at=$2,lease_expires_at=$3 WHERE id=$1").bind(item.id).bind(now).bind(now+Duration::minutes(5)).execute(&db.pool).await.unwrap();
    melodee_persistence::migrate(&db.pool).await.unwrap();
    let row:(Option<String>,Option<OffsetDateTime>,Option<Uuid>)=sqlx::query_as("SELECT lease_owner,lease_expires_at,protected_attempt_lease_epoch FROM release_work_items WHERE id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        row,
        (
            Some("existing-worker".to_owned()),
            Some(now + Duration::minutes(5)),
            None
        )
    );
    melodee_persistence::migrate(&db.pool).await.unwrap();
    db.drop().await;
}
