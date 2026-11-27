//! Database atomicity only. These synthetic records are not filesystem or
//! fs-verity evidence and must never be used to claim production activation.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    ClaimedStageReceipt, CustodyHashObservation, CustodyHashPurpose, ProtectedPublication,
    ProtectedStageIdentity, RetainedBaselineChunk, RetainedFileVerity, WorkEngineConfig,
    WorkItemStore, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn synthetic_protected_publication_is_atomic_replayable_and_quota_retained() {
    let db = common::TestDb::create("protected_publish").await;
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
    assert!(matches!(
        store.claimed_stage_receipt(&db.pool, &claim).await.unwrap(),
        ClaimedStageReceipt::NoReceipt
    ));
    store
        .begin_protected_staging_handoff(
            &db.pool,
            &claim,
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
            &claim,
            "root-gen",
            "src-gen",
            "releases/item/source",
        )
        .await
        .unwrap();
    let created = store
        .acknowledge_protected_stage_created(
            &db.pool,
            &claim,
            &reserved,
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "attempt:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    let intent = store
        .begin_protected_stage_promotion(&db.pool, &claim, &created, "content:1", "parent:1")
        .await
        .unwrap();
    let promoted = store
        .acknowledge_protected_stage_promotion(&db.pool, &claim, &intent, "content:1", "parent:1")
        .await
        .unwrap();
    let source = store
        .begin_protected_custody_hash_spool(
            &db.pool,
            &claim,
            &worker,
            CustodyHashPurpose::Source,
            "src-gen",
            "source",
            now,
        )
        .await
        .unwrap();
    // Find two distinct paths with the same first 16 SHA-256 bits. The
    // full hash and exact path, not an index prefix, establish membership.
    let mut seen = std::collections::HashMap::new();
    let (known_path, collision_path) = (0..=65_536_u32)
        .find_map(|n| {
            use sha2::Digest;
            let path = format!("candidate-{n}");
            let digest = sha2::Sha256::digest(path.as_bytes());
            let prefix = [digest[0], digest[1]];
            seen.insert(prefix, path.clone()).map(|old| (old, path))
        })
        .expect("pigeonhole guarantees a truncated-hash collision");
    assert_ne!(known_path, collision_path);
    let entry = CustodyHashObservation {
        path_key: known_path.clone(),
        parent_key: String::new(),
        name_key: "66696c65".to_owned(),
        legacy_path_hex: "6100".to_owned(),
        is_directory: false,
        legacy_length: 1,
        digest: "c".repeat(64),
    };
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, source, &worker, 1, now)
        .await
        .unwrap();
    let before = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, source, &worker)
        .await
        .unwrap();
    let stage = store
        .begin_protected_custody_hash_spool(
            &db.pool,
            &claim,
            &worker,
            CustodyHashPurpose::Staging,
            "content:1",
            "releases/item/source",
            now,
        )
        .await
        .unwrap();
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            stage,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, stage, &worker, 1, now)
        .await
        .unwrap();
    let stage_seal = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, stage, &worker)
        .await
        .unwrap();
    let baseline_id = store
        .begin_retained_baseline(&db.pool, &worker, &stage_seal)
        .await
        .unwrap();
    let baseline_file = store
        .begin_retained_baseline_file(&db.pool, &worker, baseline_id, &stage_seal, &known_path)
        .await
        .unwrap();
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &[RetainedBaselineChunk {
                index: 0,
                length: 1,
                digest: "d".repeat(64),
            }],
        )
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &"c".repeat(64),
        )
        .await
        .unwrap();
    let verity = RetainedFileVerity {
        device: u64::MAX,
        inode: u64::MAX - 1,
        digest: [0x89; 32],
    };
    store
        .record_retained_baseline_file_verity(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &verity,
        )
        .await
        .unwrap();
    let baseline = store
        .freeze_protected_retained_baseline(&db.pool, &worker, baseline_id, &stage_seal)
        .await
        .unwrap();
    let legacy = "a".repeat(64);
    let protected_session = store
        .begin_protected_retention_session(&db.pool, &claim, &before)
        .await
        .unwrap();
    store
        .register_protected_retention_stage(
            &db.pool,
            &claim,
            &protected_session,
            &before,
            &stage_seal,
            &legacy,
        )
        .await
        .unwrap();
    let session = protected_session.id();
    store
        .revalidate_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            "src-gen",
            Uuid::now_v7(),
            now,
        )
        .await
        .unwrap();
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, source, &worker, 1, now)
        .await
        .unwrap();
    let after = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, source, &worker)
        .await
        .unwrap();
    let wrong = "b".repeat(64);
    macro_rules! publication {
        ($hash:expr) => {
            ProtectedPublication {
                claim: &claim,
                attempt: &promoted,
                baseline: &baseline,
                source_before: &before,
                stage: &stage_seal,
                source_after: &after,
                session_id: session,
                legacy_hash: $hash,
                lifecycle_root_generation: "parent:1",
                lifecycle_lock_generation: "lifecycle-lock:1",
            }
        };
    }
    assert!(
        store
            .publish_protected_retained(&db.pool, publication!(&wrong))
            .await
            .is_err()
    );
    let wrong_parent = ProtectedPublication {
        claim: &claim,
        attempt: &promoted,
        baseline: &baseline,
        source_before: &before,
        stage: &stage_seal,
        source_after: &after,
        session_id: session,
        legacy_hash: &legacy,
        lifecycle_root_generation: "replaced-parent",
        lifecycle_lock_generation: "lifecycle-lock:1",
    };
    assert!(
        store
            .publish_protected_retained(&db.pool, wrong_parent)
            .await
            .is_err()
    );
    let before_state:(String,String,i64)=sqlx::query_as("SELECT (SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1),(SELECT state FROM inbound_retained_baselines WHERE id=$2),(SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1)").bind(item.id).bind(baseline_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(before_state, ("intent".to_owned(), "frozen".to_owned(), 0));
    store
        .publish_protected_retained(&db.pool, publication!(&legacy))
        .await
        .unwrap();
    store
        .publish_protected_retained(&db.pool, publication!(&legacy))
        .await
        .unwrap();
    let wrong_lock = ProtectedPublication {
        claim: &claim,
        attempt: &promoted,
        baseline: &baseline,
        source_before: &before,
        stage: &stage_seal,
        source_after: &after,
        session_id: session,
        legacy_hash: &legacy,
        lifecycle_root_generation: "parent:1",
        lifecycle_lock_generation: "replaced-lock",
    };
    assert!(
        store
            .publish_protected_retained(&db.pool, wrong_lock)
            .await
            .is_err()
    );
    let state:(String,String,Uuid,String,i64)=sqlx::query_as("SELECT (SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1),(SELECT state FROM inbound_retained_baselines WHERE id=$2),(SELECT baseline_id FROM inbound_retained_sources WHERE work_item_id=$1),(SELECT state FROM inbound_protected_stage_attempts WHERE id=$3),(SELECT attempts FROM inbound_protected_attempt_budget WHERE singleton)").bind(item.id).bind(baseline_id).bind(promoted.id()).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        state,
        (
            "staged".to_owned(),
            "authoritative".to_owned(),
            baseline_id,
            "consumed".to_owned(),
            1
        )
    );
    match store.claimed_stage_receipt(&db.pool, &claim).await.unwrap() {
        ClaimedStageReceipt::ProtectedPending(receipt) => {
            assert_eq!(receipt.baseline_id, baseline_id)
        }
        other => panic!("unexpected receipt: {other:?}"),
    }
    let found = store
        .find_committed_protected_receipt(&db.pool, &claim, promoted.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.baseline_id, baseline_id);
    assert_eq!(found.attempt_id, promoted.id());
    assert_eq!(found.source_path, "source");
    assert_eq!(found.stage_binding, "releases/item/source");
    assert_eq!(found.stage_generation, "content:1");
    assert_eq!(found.root_generation, "root-gen");
    assert_eq!(found.lock_generation, "lock:1");
    assert_eq!(found.legacy_hash, legacy);
    sqlx::query("UPDATE release_work_item_staging_handoffs SET tree_hash=repeat('b',64) WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .find_committed_protected_receipt(&db.pool, &claim, promoted.id())
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET tree_hash=$2 WHERE work_item_id=$1")
        .bind(item.id)
        .bind(&legacy)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - time::Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, new_claim) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(matches!(
        store
            .find_committed_protected_receipt(&db.pool, &claim, promoted.id())
            .await,
        Err(melodee_persistence::WorkStoreError::LeaseExpired(_))
    ));
    assert!(store.claimed_stage_receipt(&db.pool, &claim).await.is_err());
    assert!(matches!(
        store
            .claimed_stage_receipt(&db.pool, &new_claim)
            .await
            .unwrap(),
        ClaimedStageReceipt::ProtectedPending(_)
    ));
    let recovered = store
        .find_committed_protected_receipt(&db.pool, &new_claim, promoted.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.baseline_id, baseline_id);
    assert_eq!(recovered.marker_generation, "marker:1");
    assert_eq!(recovered.attempt_state, "consumed");
    // Synthetic database-only cleanup protocol; real callers must hold the
    // exclusive producer lock and independently fsync private namespaces.
    let cleanup = store
        .claim_protected_stage_cleanup(&db.pool, &worker, 1, time::Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(cleanup.attempt_id(), promoted.id());
    assert_eq!(cleanup.baseline_id(), baseline_id);
    let old_expiry: OffsetDateTime = sqlx::query_scalar(
        "SELECT cleanup_expires_at FROM inbound_protected_stage_attempts WHERE id=$1",
    )
    .bind(promoted.id())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    store
        .renew_protected_stage_cleanup(&db.pool, &cleanup, time::Duration::minutes(2))
        .await
        .unwrap();
    let renewed: OffsetDateTime = sqlx::query_scalar(
        "SELECT cleanup_expires_at FROM inbound_protected_stage_attempts WHERE id=$1",
    )
    .bind(promoted.id())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(renewed > old_expiry);
    assert!(
        store
            .complete_protected_stage_cleanup(&db.pool, &cleanup)
            .await
            .is_err()
    );
    assert!(
        store
            .defer_protected_stage_cleanup(
                &db.pool,
                &cleanup,
                melodee_persistence::ProtectedCleanupRetryReason::Busy,
                time::Duration::ZERO
            )
            .await
            .is_err()
    );
    store
        .defer_protected_stage_cleanup(
            &db.pool,
            &cleanup,
            melodee_persistence::ProtectedCleanupRetryReason::Busy,
            time::Duration::minutes(1),
        )
        .await
        .unwrap();
    let deferred:(String,Option<String>,i32,i64)=sqlx::query_as("SELECT state,cleanup_last_reason,cleanup_retry_count,(SELECT attempts FROM inbound_protected_attempt_budget WHERE singleton) FROM inbound_protected_stage_attempts WHERE id=$1").bind(promoted.id()).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        deferred,
        ("consumed".to_owned(), Some("busy".to_owned()), 1, 1)
    );
    assert!(
        store
            .begin_protected_stage_cleanup(&db.pool, &cleanup)
            .await
            .is_err()
    );
    sqlx::query("UPDATE inbound_protected_stage_attempts SET cleanup_due_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(promoted.id()).execute(&db.pool).await.unwrap();
    let cleanup = store
        .claim_protected_stage_cleanup(&db.pool, &worker, 1, time::Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM inbound_protected_stage_attempts WHERE id=$1 FOR UPDATE")
        .bind(promoted.id())
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let cancelled = cleanup.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .begin_protected_stage_cleanup(&pool, &cancelled)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT state,cleanup_owner%')").fetch_one(&db.pool).await.unwrap();if waiting{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    task.abort();
    let _ = task.await;
    lock.commit().await.unwrap();
    let still_consumed: String =
        sqlx::query_scalar("SELECT state FROM inbound_protected_stage_attempts WHERE id=$1")
            .bind(promoted.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(still_consumed, "consumed");
    sqlx::query("UPDATE inbound_protected_stage_attempts SET cleanup_expires_at=clock_timestamp()+interval '1 second',cleanup_due_at=clock_timestamp()+interval '1 second' WHERE id=$1").bind(promoted.id()).execute(&db.pool).await.unwrap();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM inbound_protected_stage_attempts WHERE id=$1 FOR UPDATE")
        .bind(promoted.id())
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let old = cleanup.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .renew_protected_stage_cleanup(&pool, &old, time::Duration::minutes(2))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT state,cleanup_owner%')").fetch_one(&db.pool).await.unwrap();if waiting{break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    lock.commit().await.unwrap();
    assert!(task.await.unwrap().is_err());
    sqlx::query("UPDATE inbound_protected_stage_attempts SET cleanup_due_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(promoted.id()).execute(&db.pool).await.unwrap();
    let replacement = store
        .claim_protected_stage_cleanup(&db.pool, &worker, 1, time::Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .renew_protected_stage_cleanup(&db.pool, &cleanup, time::Duration::minutes(1))
            .await
            .is_err()
    );
    let cleanup = replacement;
    store
        .begin_protected_stage_cleanup(&db.pool, &cleanup)
        .await
        .unwrap();
    store
        .begin_protected_stage_cleanup(&db.pool, &cleanup)
        .await
        .unwrap();
    let retained_count: i64 =
        sqlx::query_scalar("SELECT attempts FROM inbound_protected_attempt_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(retained_count, 1);
    sqlx::query("UPDATE inbound_protected_stage_attempts SET cleanup_expires_at=clock_timestamp()-interval '1 second',cleanup_due_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(promoted.id()).execute(&db.pool).await.unwrap();
    let new = store
        .claim_protected_stage_cleanup(&db.pool, &WorkerId::new(), 1, time::Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .complete_protected_stage_cleanup(&db.pool, &cleanup)
            .await
            .is_err()
    );
    store
        .begin_protected_stage_cleanup(&db.pool, &new)
        .await
        .unwrap();
    store
        .complete_protected_stage_cleanup(&db.pool, &new)
        .await
        .unwrap();
    assert!(
        store
            .protected_stage_cleanup_completed(&db.pool, baseline_id, promoted.id())
            .await
            .unwrap()
    );
    assert!(
        !store
            .protected_stage_cleanup_completed(&db.pool, baseline_id, Uuid::now_v7())
            .await
            .unwrap()
    );
    assert!(
        store
            .find_cleaned_protected_receipt(&db.pool, &claim, promoted.id())
            .await
            .is_err()
    );
    assert!(matches!(
        store
            .claimed_stage_receipt(&db.pool, &new_claim)
            .await
            .unwrap(),
        ClaimedStageReceipt::ProtectedCleaned(_)
    ));
    let cleaned = store
        .find_cleaned_protected_receipt(&db.pool, &new_claim, promoted.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleaned.baseline_id, baseline_id);
    assert_eq!(cleaned.attempt_id, promoted.id());
    assert_eq!(cleaned.session_id, session);
    assert_eq!(cleaned.root_generation, "root-gen");
    assert_eq!(cleaned.final_parent_generation, "parent:1");
    assert_eq!(cleaned.stage_binding, "releases/item/source");
    assert_eq!(
        cleaned.lifecycle_lock_name,
        format!("stage-{}.lock", baseline_id.simple())
    );
    assert_eq!(cleaned.lifecycle_root_generation, "parent:1");
    assert_eq!(cleaned.lifecycle_lock_generation, "lifecycle-lock:1");
    assert_eq!(cleaned.legacy_hash, legacy);
    assert!(
        store
            .cleaned_protected_verity_page(&db.pool, &new_claim, promoted.id(), None, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .cleaned_protected_verity_page(&db.pool, &new_claim, promoted.id(), None, 17)
            .await
            .is_err()
    );
    assert!(
        store
            .cleaned_protected_directory_page(&db.pool, &new_claim, promoted.id(), None, 17)
            .await
            .is_err()
    );
    let members = [
        melodee_persistence::RetainedDirectoryMember {
            path_key: known_path.clone(),
            is_directory: false,
        },
        melodee_persistence::RetainedDirectoryMember {
            path_key: collision_path.clone(),
            is_directory: false,
        },
        melodee_persistence::RetainedDirectoryMember {
            path_key: known_path.clone(),
            is_directory: true,
        },
        melodee_persistence::RetainedDirectoryMember {
            path_key: "new-child".to_owned(),
            is_directory: false,
        },
    ];
    assert_eq!(
        store
            .cleaned_protected_directory_membership(
                &db.pool,
                &new_claim,
                promoted.id(),
                "",
                &members
            )
            .await
            .unwrap(),
        vec![true, false, false, false]
    );
    assert!(
        store
            .cleaned_protected_directory_membership(&db.pool, &claim, promoted.id(), "", &members)
            .await
            .is_err()
    );
    assert!(
        store
            .cleaned_protected_directory_membership(
                &db.pool,
                &new_claim,
                promoted.id(),
                "missing-parent",
                &members
            )
            .await
            .is_err()
    );
    let too_many = vec![
        melodee_persistence::RetainedDirectoryMember {
            path_key: "x".to_owned(),
            is_directory: false
        };
        65
    ];
    assert!(
        store
            .cleaned_protected_directory_membership(
                &db.pool,
                &new_claim,
                promoted.id(),
                "",
                &too_many
            )
            .await
            .is_err()
    );
    let directories = store
        .cleaned_protected_directory_page(&db.pool, &new_claim, promoted.id(), None, 16)
        .await
        .unwrap();
    assert_eq!(directories.len(), 1);
    assert_eq!(directories[0].ordinal, 0);
    assert_eq!(directories[0].path_key, "");
    assert_eq!(
        directories[0].stage_digest,
        stage_seal.digest().strip_prefix("v3:").unwrap()
    );
    assert!(
        store
            .cleaned_protected_directory_page(&db.pool, &new_claim, promoted.id(), Some(0), 16)
            .await
            .unwrap()
            .is_empty()
    );
    let page = store
        .cleaned_protected_verity_page(&db.pool, &new_claim, promoted.id(), None, 16)
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].path_key, known_path);
    assert_eq!(page[0].evidence, verity);
    assert!(
        store
            .cleaned_protected_verity_page(
                &db.pool,
                &new_claim,
                promoted.id(),
                Some(page[0].ordinal),
                16
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .find_committed_protected_receipt(&db.pool, &new_claim, promoted.id())
            .await
            .unwrap()
            .is_none()
    );
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT attempts,bytes FROM inbound_protected_attempt_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0));
    let active: bool = sqlx::query_scalar(
        "SELECT status='active' FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(active);
    db.drop().await;
}

#[tokio::test]
async fn same_worker_reclaim_cannot_reuse_prior_protected_observations() {
    let db = common::TestDb::create("protected_aba").await;
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
    store
        .begin_protected_staging_handoff(
            &db.pool,
            &claim,
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
            &claim,
            "root-gen",
            "src-gen",
            "releases/item/source",
        )
        .await
        .unwrap();
    let created = store
        .acknowledge_protected_stage_created(
            &db.pool,
            &claim,
            &reserved,
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "attempt:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    let intent = store
        .begin_protected_stage_promotion(&db.pool, &claim, &created, "content:1", "parent:1")
        .await
        .unwrap();
    let promoted = store
        .acknowledge_protected_stage_promotion(&db.pool, &claim, &intent, "content:1", "parent:1")
        .await
        .unwrap();
    let source = store
        .begin_protected_custody_hash_spool(
            &db.pool,
            &claim,
            &worker,
            CustodyHashPurpose::Source,
            "src-gen",
            "source",
            now,
        )
        .await
        .unwrap();
    // Find two distinct paths with the same first 16 SHA-256 bits. The
    // full hash and exact path, not an index prefix, establish membership.
    let mut seen = std::collections::HashMap::new();
    let (known_path, collision_path) = (0..=65_536_u32)
        .find_map(|n| {
            use sha2::Digest;
            let path = format!("candidate-{n}");
            let digest = sha2::Sha256::digest(path.as_bytes());
            let prefix = [digest[0], digest[1]];
            seen.insert(prefix, path.clone()).map(|old| (old, path))
        })
        .expect("pigeonhole guarantees a truncated-hash collision");
    assert_ne!(known_path, collision_path);
    let entry = CustodyHashObservation {
        path_key: known_path.clone(),
        parent_key: String::new(),
        name_key: "66696c65".to_owned(),
        legacy_path_hex: "6100".to_owned(),
        is_directory: false,
        legacy_length: 1,
        digest: "c".repeat(64),
    };
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, source, &worker, 1, now)
        .await
        .unwrap();
    let before = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, source, &worker)
        .await
        .unwrap();
    let stage = store
        .begin_protected_custody_hash_spool(
            &db.pool,
            &claim,
            &worker,
            CustodyHashPurpose::Staging,
            "content:1",
            "releases/item/source",
            now,
        )
        .await
        .unwrap();
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            stage,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, stage, &worker, 1, now)
        .await
        .unwrap();
    let stage_seal = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, stage, &worker)
        .await
        .unwrap();
    let baseline_id = store
        .begin_retained_baseline(&db.pool, &worker, &stage_seal)
        .await
        .unwrap();
    let baseline_file = store
        .begin_retained_baseline_file(&db.pool, &worker, baseline_id, &stage_seal, &known_path)
        .await
        .unwrap();
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &[RetainedBaselineChunk {
                index: 0,
                length: 1,
                digest: "d".repeat(64),
            }],
        )
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &"c".repeat(64),
        )
        .await
        .unwrap();
    let verity = RetainedFileVerity {
        device: u64::MAX,
        inode: u64::MAX - 1,
        digest: [0x89; 32],
    };
    store
        .record_retained_baseline_file_verity(
            &db.pool,
            &worker,
            baseline_id,
            &stage_seal,
            baseline_file.ordinal,
            &verity,
        )
        .await
        .unwrap();
    let baseline = store
        .freeze_protected_retained_baseline(&db.pool, &worker, baseline_id, &stage_seal)
        .await
        .unwrap();
    let legacy = "a".repeat(64);
    let protected_session = store
        .begin_protected_retention_session(&db.pool, &claim, &before)
        .await
        .unwrap();
    store
        .register_protected_retention_stage(
            &db.pool,
            &claim,
            &protected_session,
            &before,
            &stage_seal,
            &legacy,
        )
        .await
        .unwrap();
    let session = protected_session.id();
    store
        .revalidate_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            "src-gen",
            Uuid::now_v7(),
            now,
        )
        .await
        .unwrap();
    store
        .append_protected_custody_hash_spool(
            &db.pool,
            &claim,
            source,
            &worker,
            std::slice::from_ref(&entry),
            now,
        )
        .await
        .unwrap();
    store
        .freeze_protected_custody_hash_spool(&db.pool, &claim, source, &worker, 1, now)
        .await
        .unwrap();
    let after = store
        .seal_protected_custody_hash_spool(&db.pool, &claim, source, &worker)
        .await
        .unwrap();
    // Reclaim with the same worker and caller-supplied timestamp. The attempt
    // can be adopted, but observations from the old claim may never follow it.
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - time::Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .claim(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty(),
        "ordinary claim must skip protected custody"
    );
    let (_, new_claim) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(
        store
            .begin_staging_handoff(
                &db.pool,
                item.id,
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
            .record_staging_receipt(
                &db.pool,
                item.id,
                &worker,
                melodee_persistence::StagingHandoffMethod::VerifiedCopy,
                &legacy,
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .record_inbound_removed(&db.pool, item.id, &worker, now)
            .await
            .is_err()
    );
    assert!(
        store
            .record_inbound_replaced(&db.pool, item.id, &worker, now)
            .await
            .is_err()
    );
    assert!(
        store
            .renew_lease(&db.pool, item.id, &worker, time::Duration::minutes(1), now)
            .await
            .is_err()
    );
    assert!(
        store
            .renew_custody_hash_lease(&db.pool, item.id, &worker, time::Duration::minutes(1))
            .await
            .is_err()
    );
    assert!(
        store
            .release_lease(&db.pool, item.id, &worker, now)
            .await
            .is_err()
    );
    assert!(
        store
            .release_protected_lease(&db.pool, &claim, &worker, now)
            .await
            .is_err()
    );
    let adopted = store
        .adopt_created_protected_stage_attempt(
            &db.pool,
            &new_claim,
            promoted.id(),
            &ProtectedStageIdentity {
                root_generation: "root-gen",
                namespace_generation: "attempt:1",
                marker_generation: "marker:1",
                lock_generation: "lock:1",
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .append_protected_custody_hash_spool(
                &db.pool,
                &claim,
                source,
                &worker,
                std::slice::from_ref(&entry),
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .revalidate_protected_custody_hash_spool(
                &db.pool,
                &new_claim,
                source,
                &worker,
                "src-gen",
                Uuid::now_v7(),
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .revalidate_custody_hash_spool(
                &db.pool,
                source,
                item.id,
                &worker,
                "src-gen",
                Uuid::now_v7(),
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .begin_protected_retention_session(&db.pool, &new_claim, &after)
            .await
            .is_err()
    );
    assert!(
        store
            .freeze_protected_retained_baseline(&db.pool, &worker, baseline_id, &stage_seal)
            .await
            .is_err()
    );
    let publication = ProtectedPublication {
        claim: &new_claim,
        attempt: &adopted,
        baseline: &baseline,
        source_before: &before,
        stage: &stage_seal,
        source_after: &after,
        session_id: session,
        legacy_hash: &legacy,
        lifecycle_root_generation: "parent:1",
        lifecycle_lock_generation: "lifecycle-lock:1",
    };
    assert!(
        store
            .publish_protected_retained(&db.pool, publication)
            .await
            .is_err()
    );
    let state:(String,String,i64)=sqlx::query_as("SELECT (SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1),(SELECT state FROM inbound_retained_baselines WHERE id=$2),(SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1)").bind(item.id).bind(baseline_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(state, ("intent".to_owned(), "frozen".to_owned(), 0));
    assert!(
        store
            .transition_protected(
                &db.pool,
                &claim,
                &worker,
                IngestionState::Inspecting,
                None,
                now
            )
            .await
            .is_err()
    );
    store
        .transition_protected(
            &db.pool,
            &new_claim,
            &worker,
            IngestionState::Inspecting,
            None,
            now,
        )
        .await
        .unwrap();
    assert!(
        store
            .release_lease(&db.pool, item.id, &worker, now)
            .await
            .is_err()
    );
    store
        .release_protected_lease(&db.pool, &new_claim, &worker, now)
        .await
        .unwrap();
    let owner: Option<String> =
        sqlx::query_scalar("SELECT lease_owner FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(owner, None);
    db.drop().await;
}

#[tokio::test]
async fn claimed_receipt_distinguishes_legacy_and_fails_closed_on_ambiguous_attempt() {
    let db = common::TestDb::create("claimed_receipt_legacy").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let (_, claim) = store
        .claim_with_protected_epoch(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(matches!(
        store.claimed_stage_receipt(&db.pool, &claim).await.unwrap(),
        ClaimedStageReceipt::NoReceipt
    ));
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
    assert!(matches!(
        store.claimed_stage_receipt(&db.pool, &claim).await.unwrap(),
        ClaimedStageReceipt::NoReceipt
    ));
    sqlx::query("UPDATE release_work_item_staging_handoffs SET status='staged',method='verified_copy',tree_hash=repeat('a',64),staged_at=$2 WHERE work_item_id=$1").bind(item.id).bind(now).execute(&db.pool).await.unwrap();
    assert!(matches!(
        store.claimed_stage_receipt(&db.pool, &claim).await.unwrap(),
        ClaimedStageReceipt::Legacy
    ));
    db.drop().await;
}
