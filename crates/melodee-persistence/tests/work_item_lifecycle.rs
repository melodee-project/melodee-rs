//! DB metadata tests only; they do not prove filesystem quiescence or locks.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{LifecycleQuarantineReason, WorkEngineConfig, WorkItemStore, WorkerId};
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
async fn maintenance_cursor_replay_quarantine_and_expired_fence_are_durable() {
    let db = common::TestDb::create("lifecycle_repair").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let mut items = Vec::new();
    for path in ["source-a", "source-b", "source-c"] {
        let item = ReleaseWorkItem::discovered(path.to_owned(), None, Uuid::now_v7(), now);
        store.create(&db.pool, &item).await.unwrap();
        items.push(item.id);
    }
    items.sort_unstable();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", Duration::minutes(1))
        .await
        .unwrap();
    let blocked = ReleaseWorkItem::discovered("source-d".to_owned(), None, Uuid::now_v7(), now);
    assert!(store.create(&db.pool, &blocked).await.is_err());
    assert!(
        store
            .claim(
                &db.pool,
                &WorkerId::new(),
                &WorkEngineConfig::default(),
                now
            )
            .await
            .is_err()
    );
    let false_attestation = melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "reason",
        backup_reference: "backup",
        stop_all_writers: false,
        inspection_complete: true,
    };
    assert!(
        store
            .confirm_lifecycle_quiescence(&db.pool, &fence, &false_attestation)
            .await
            .is_err()
    );
    let missing_backup = melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "reason",
        backup_reference: " ",
        stop_all_writers: true,
        inspection_complete: true,
    };
    assert!(
        store
            .confirm_lifecycle_quiescence(&db.pool, &fence, &missing_backup)
            .await
            .is_err()
    );
    store
        .confirm_lifecycle_quiescence(&db.pool, &fence, &attestation())
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(&db.pool, &fence, &attestation())
        .await
        .unwrap();
    let changed = melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "different",
        ..attestation()
    };
    assert!(
        store
            .confirm_lifecycle_quiescence(&db.pool, &fence, &changed)
            .await
            .is_err()
    );
    let audit:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM work_item_lifecycle_attestations),entries FROM work_item_lifecycle_attestation_budget WHERE singleton").fetch_one(&db.pool).await.unwrap();
    assert_eq!(audit, (1, 1));
    assert!(
        sqlx::query("DELETE FROM work_item_lifecycle_attestations")
            .execute(&db.pool)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &fence)
            .await
            .unwrap(),
        Some(items[0])
    );
    assert!(
        store
            .reserve_work_item_lifecycle(&db.pool, &fence, items[1])
            .await
            .is_err()
    );
    let reserved = store
        .reserve_work_item_lifecycle(&db.pool, &fence, items[0])
        .await
        .unwrap();
    assert_eq!(reserved.state, "reserved");
    assert_eq!(
        reserved.lock_name,
        format!("work-{}.lock", items[0].simple())
    );
    assert_eq!(
        store
            .reserve_work_item_lifecycle(&db.pool, &fence, items[0])
            .await
            .unwrap(),
        reserved
    );
    let ready = store
        .acknowledge_work_item_lifecycle(
            &db.pool,
            &fence,
            items[0],
            "lock-root",
            "lock-inode",
            "stage-root",
        )
        .await
        .unwrap();
    assert_eq!(ready.state, "ready");
    let mut wrong_identity = ready.clone();
    wrong_identity.lock_generation = Some("replacement".to_owned());
    assert!(
        store
            .invalidate_work_item_lifecycle(
                &db.pool,
                &fence,
                &wrong_identity,
                LifecycleQuarantineReason::Changed
            )
            .await
            .is_err()
    );
    store
        .invalidate_work_item_lifecycle(
            &db.pool,
            &fence,
            &ready,
            LifecycleQuarantineReason::Changed,
        )
        .await
        .unwrap();
    let quarantined: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM work_item_lifecycle_quarantine WHERE work_item_id=$1)",
    )
    .bind(items[0])
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(quarantined);

    assert_eq!(
        store
            .acknowledge_work_item_lifecycle(
                &db.pool,
                &fence,
                items[0],
                "lock-root",
                "lock-inode",
                "stage-root"
            )
            .await
            .unwrap(),
        ready
    );
    assert!(
        store
            .acknowledge_work_item_lifecycle(
                &db.pool,
                &fence,
                items[0],
                "lock-root",
                "wrong",
                "stage-root"
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &fence)
            .await
            .unwrap(),
        Some(items[1])
    );
    store
        .quarantine_lifecycle_provision_item(
            &db.pool,
            &fence,
            items[1],
            LifecycleQuarantineReason::Unreadable,
        )
        .await
        .unwrap();
    assert!(
        store
            .finish_lifecycle_maintenance(&db.pool, &fence)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &fence)
            .await
            .unwrap(),
        Some(items[2])
    );
    store
        .reserve_work_item_lifecycle(&db.pool, &fence, items[2])
        .await
        .unwrap();
    // Simulate crash of an externally quiesced maintenance process: token
    // expiry never releases the fence or permits old-token writes.
    sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE singleton").execute(&db.pool).await.unwrap();
    assert!(
        store
            .next_lifecycle_provision_item(&db.pool, &fence)
            .await
            .is_err()
    );
    assert!(
        store
            .claim(
                &db.pool,
                &WorkerId::new(),
                &WorkEngineConfig::default(),
                now
            )
            .await
            .is_err()
    );
    let replacement = store
        .reclaim_lifecycle_maintenance(&db.pool, "operator", Duration::minutes(1))
        .await
        .unwrap();
    assert!(
        store
            .reserve_work_item_lifecycle(&db.pool, &replacement, items[2])
            .await
            .is_err()
    );
    store
        .confirm_lifecycle_quiescence(&db.pool, &replacement, &attestation())
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_work_item_lifecycle(&db.pool, &replacement, items[2])
            .await
            .unwrap()
            .state,
        "reserved"
    );
    store
        .acknowledge_work_item_lifecycle(
            &db.pool,
            &replacement,
            items[2],
            "lock-root",
            "inode-c",
            "stage-root",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &replacement)
            .await
            .unwrap(),
        None
    );
    assert!(
        store
            .finish_lifecycle_maintenance(&db.pool, &replacement)
            .await
            .is_err()
    );
    for (id, inode) in [
        (items[0], "lock-inode"),
        (items[1], "inode-b"),
        (items[2], "inode-c"),
    ] {
        assert_eq!(
            store
                .next_lifecycle_provision_item(&db.pool, &replacement)
                .await
                .unwrap(),
            Some(id)
        );
        store
            .reserve_work_item_lifecycle(&db.pool, &replacement, id)
            .await
            .unwrap();
        store
            .acknowledge_work_item_lifecycle(
                &db.pool,
                &replacement,
                id,
                "lock-root",
                inode,
                "stage-root",
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .next_lifecycle_provision_item(&db.pool, &replacement)
            .await
            .unwrap(),
        None
    );
    store
        .finish_lifecycle_maintenance(&db.pool, &replacement)
        .await
        .unwrap();
    let enforcement: bool = sqlx::query_scalar(
        "SELECT enforcement_enabled FROM work_item_lifecycle_maintenance WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !enforcement,
        "metadata sweep must not activate incomplete runtime participation"
    );
    assert!(
        sqlx::query(
            "UPDATE work_item_lifecycle_locks SET lock_generation='changed' WHERE work_item_id=$1"
        )
        .bind(items[0])
        .execute(&db.pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM work_item_lifecycle_locks WHERE work_item_id=$1")
            .bind(items[0])
            .execute(&db.pool)
            .await
            .is_err()
    );
    let reservation = store
        .begin_admin_media_reservation(&db.pool, &[items[0]], &[], "operator", Duration::minutes(1))
        .await
        .unwrap();
    let page = store
        .admin_reservation_lifecycle_page(&db.pool, &reservation, None)
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    store
        .revalidate_admin_reservation_lifecycles(&db.pool, &reservation, &page)
        .await
        .unwrap();
    let mut changed = page;
    changed[0].lock_generation = "wrong".to_owned();
    assert!(
        store
            .revalidate_admin_reservation_lifecycles(&db.pool, &reservation, &changed)
            .await
            .is_err()
    );
    store
        .finish_admin_media_reservation(&db.pool, &reservation)
        .await
        .unwrap();
    db.drop().await;
}

#[tokio::test]
async fn new_item_provisioning_fences_claims_and_rejects_old_job_tokens() {
    let db = common::TestDb::create("lifecycle_new_item").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    // Test-only setup: no production API can enable this flag while lifecycle
    // participation and the external old-writer fence remain incomplete.
    sqlx::query(
        "UPDATE work_item_lifecycle_maintenance SET enforcement_enabled=true WHERE singleton",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let config = WorkEngineConfig::default();
    let mut item = ReleaseWorkItem::discovered("new-source".to_owned(), None, Uuid::now_v7(), now);
    item.state = melodee_domain::IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    assert!(
        store
            .claim(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .claim_lifecycle_provision_jobs(&db.pool, "provisioner", 17, Duration::minutes(1))
            .await
            .is_err()
    );
    let old = store
        .claim_lifecycle_provision_jobs(&db.pool, "provisioner", 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(old.work_item_id(), item.id);
    sqlx::query("UPDATE work_item_lifecycle_provision_jobs SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let current = store
        .claim_lifecycle_provision_jobs(&db.pool, "provisioner", 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .acknowledge_lifecycle_provision(&db.pool, &old, "lock-root", "inode", "stage-root")
            .await
            .is_err()
    );
    store
        .acknowledge_lifecycle_provision(&db.pool, &current, "lock-root", "inode", "stage-root")
        .await
        .unwrap();
    store
        .acknowledge_lifecycle_provision(&db.pool, &current, "lock-root", "inode", "stage-root")
        .await
        .unwrap();
    assert!(
        store
            .acknowledge_lifecycle_provision(&db.pool, &current, "lock-root", "wrong", "stage-root")
            .await
            .is_err()
    );
    let (_, claim) = store
        .claim_ordinary_with_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    let identity = store
        .claimed_work_item_lifecycle(&db.pool, &claim)
        .await
        .unwrap();
    assert_eq!(identity.lock_generation.as_deref(), Some("inode"));
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, replacement) = store
        .claim_ordinary_with_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(
        store
            .claimed_work_item_lifecycle(&db.pool, &claim)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .claimed_work_item_lifecycle(&db.pool, &replacement)
            .await
            .unwrap(),
        identity
    );
    store
        .release_lease(&db.pool, item.id, &worker, now)
        .await
        .unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(&db.pool, &fence, &attestation())
        .await
        .unwrap();
    store
        .invalidate_work_item_lifecycle(
            &db.pool,
            &fence,
            &identity,
            LifecycleQuarantineReason::Changed,
        )
        .await
        .unwrap();
    assert!(
        store
            .finish_lifecycle_maintenance(&db.pool, &fence)
            .await
            .is_err()
    );
    // Test-only simulation of a stopped maintenance process: runtime lookups
    // must reject the quarantined row even if an operator bypasses the fence.
    sqlx::query("UPDATE work_item_lifecycle_maintenance SET owner=NULL,token=NULL,lease_expires_at=NULL,quiesced=false WHERE singleton").execute(&db.pool).await.unwrap();
    assert!(
        store
            .claim_ordinary_with_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    let reservation = store
        .begin_admin_media_reservation(&db.pool, &[item.id], &[], "operator", Duration::minutes(1))
        .await
        .unwrap();
    assert!(
        store
            .admin_reservation_lifecycle_page(&db.pool, &reservation, None)
            .await
            .is_err()
    );
    store
        .finish_admin_media_reservation(&db.pool, &reservation)
        .await
        .unwrap();
    db.drop().await;
}

async fn blocked_attestation(cancel: bool) {
    let db = common::TestDb::create("lifecycle_attestation_stall").await;
    let store = WorkItemStore;
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", Duration::minutes(1))
        .await
        .unwrap();
    let expiry:OffsetDateTime=sqlx::query_scalar("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()+interval '2 seconds' WHERE singleton RETURNING lease_expires_at").fetch_one(&db.pool).await.unwrap();
    let key = (Uuid::now_v7().as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_lifecycle_audit_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_lifecycle_audit_stall BEFORE INSERT ON work_item_lifecycle_attestations FOR EACH ROW EXECUTE FUNCTION test_lifecycle_audit_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .confirm_lifecycle_quiescence(&pool, &fence, &attestation())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(4),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'INSERT INTO work_item_lifecycle_attestations%')").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.expect("attestation reached audit stall");
    if cancel {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>$1")
                    .bind(expiry)
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
                if expired {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    let mut verify = db.pool.begin().await.unwrap();
    let (quiesced,audits,budget):(bool,i64,i64)=sqlx::query_as("SELECT quiesced,(SELECT count(*) FROM work_item_lifecycle_attestations),(SELECT entries FROM work_item_lifecycle_attestation_budget WHERE singleton) FROM work_item_lifecycle_maintenance WHERE singleton FOR UPDATE").fetch_one(&mut *verify).await.unwrap();
    assert_eq!((quiesced, audits, budget), (false, 0, 0));
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}
#[tokio::test]
async fn expired_attestation_rolls_back() {
    blocked_attestation(false).await;
}
#[tokio::test]
async fn cancelled_attestation_rolls_back() {
    blocked_attestation(true).await;
}
