//! Protected mutations cannot commit after expiry or cancellation.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    ClaimedStageReceipt, CustodyHashObservation, CustodyHashPurpose, ProtectedPublication,
    ProtectedStageIdentity, RetainedBaselineChunk, RetainedFileVerity, WorkEngineConfig,
    WorkItemStore, WorkStoreError, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

async fn blocked_baseline(cancel: bool) {
    let db = common::TestDb::create("protected_block_baseline").await;
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
    let _promoted = store
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
    let _before = store
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
    let baseline_quota: (i64, i64) =
        sqlx::query_as("SELECT rows,bytes FROM inbound_retained_baseline_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let expiry:OffsetDateTime=sqlx::query_scalar("UPDATE release_work_items SET lease_expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1 RETURNING lease_expires_at").bind(item.id).fetch_one(&db.pool).await.unwrap();
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_protected_mutation_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_protected_mutation_stall BEFORE INSERT ON inbound_retained_baseline_files FOR EACH ROW EXECUTE FUNCTION test_protected_mutation_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let seal = stage_seal.clone();
    let worker_task = worker.clone();
    let path = known_path.clone();
    let task = tokio::spawn(async move {
        matches!(
            WorkItemStore
                .begin_retained_baseline_file(&pool, &worker_task, baseline_id, &seal, &path)
                .await,
            Err(WorkStoreError::LeaseExpired(_))
        )
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE $1)").bind("INSERT INTO inbound_retained_baseline_files%").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    } }).await.expect("mutation reached advisory stall");
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
        .expect("database lease expiry");
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(task.await.unwrap(), "expired transaction must fail");
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    let mut verify = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .fetch_one(&mut *verify)
        .await
        .unwrap();
    let (files,rows,bytes):(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM inbound_retained_baseline_files WHERE baseline_id=$1),rows,bytes FROM inbound_retained_baseline_budget WHERE singleton").bind(baseline_id).fetch_one(&mut *verify).await.unwrap();
    assert_eq!(files, 0);
    assert_eq!((rows, bytes), baseline_quota);
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}
#[tokio::test]
async fn baseline_expiry_rolls_back() {
    blocked_baseline(false).await;
}
#[tokio::test]
async fn baseline_cancellation_rolls_back() {
    blocked_baseline(true).await;
}
async fn blocked_session(cancel: bool) {
    let db = common::TestDb::create("protected_block_session").await;
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
    let _promoted = store
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
    let _baseline = store
        .freeze_protected_retained_baseline(&db.pool, &worker, baseline_id, &stage_seal)
        .await
        .unwrap();
    let _legacy = "a".repeat(64);
    let expiry:OffsetDateTime=sqlx::query_scalar("UPDATE release_work_items SET lease_expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1 RETURNING lease_expires_at").bind(item.id).fetch_one(&db.pool).await.unwrap();
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_protected_mutation_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_protected_mutation_stall BEFORE INSERT ON inbound_retained_proof_sessions FOR EACH ROW EXECUTE FUNCTION test_protected_mutation_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let claim_task = claim.clone();
    let before_task = before.clone();
    let task = tokio::spawn(async move {
        matches!(
            WorkItemStore
                .begin_protected_retention_session(&pool, &claim_task, &before_task)
                .await,
            Err(WorkStoreError::LeaseExpired(_))
        )
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE $1)").bind("INSERT INTO inbound_retained_proof_sessions%").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    } }).await.expect("mutation reached advisory stall");
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
        .expect("database lease expiry");
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(task.await.unwrap(), "expired transaction must fail");
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    let mut verify = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .fetch_one(&mut *verify)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_retained_proof_sessions WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&mut *verify)
    .await
    .unwrap();
    assert_eq!(count, 0);
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}
#[tokio::test]
async fn session_expiry_rolls_back() {
    blocked_session(false).await;
}
#[tokio::test]
async fn session_cancellation_rolls_back() {
    blocked_session(true).await;
}
async fn blocked_publication(cancel: bool) {
    let db = common::TestDb::create("protected_block_publication").await;
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
    let expiry:OffsetDateTime=sqlx::query_scalar("UPDATE release_work_items SET lease_expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1 RETURNING lease_expires_at").bind(item.id).fetch_one(&db.pool).await.unwrap();
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_protected_mutation_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_protected_mutation_stall BEFORE UPDATE ON release_work_item_staging_handoffs FOR EACH ROW EXECUTE FUNCTION test_protected_mutation_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let claim_task = claim.clone();
    let promoted_task = promoted.clone();
    let baseline_task = baseline.clone();
    let before_task = before.clone();
    let stage_task = stage_seal.clone();
    let after_task = after.clone();
    let legacy_task = legacy.clone();
    let task = tokio::spawn(async move {
        let publication = ProtectedPublication {
            claim: &claim_task,
            attempt: &promoted_task,
            baseline: &baseline_task,
            source_before: &before_task,
            stage: &stage_task,
            source_after: &after_task,
            session_id: session,
            legacy_hash: &legacy_task,
            lifecycle_root_generation: "parent:1",
            lifecycle_lock_generation: "lifecycle-lock:1",
        };
        matches!(
            WorkItemStore
                .publish_protected_retained(&pool, publication)
                .await,
            Err(WorkStoreError::LeaseExpired(_))
        )
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE $1)").bind("UPDATE release_work_item_staging_handoffs SET status=%").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    } }).await.expect("mutation reached advisory stall");
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
        .expect("database lease expiry");
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(task.await.unwrap(), "expired transaction must fail");
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    let mut verify = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM release_work_items WHERE id=$1 FOR UPDATE")
        .bind(item.id)
        .fetch_one(&mut *verify)
        .await
        .unwrap();
    let state:(String,String,i64)=sqlx::query_as("SELECT (SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1),(SELECT state FROM inbound_retained_baselines WHERE id=$2),(SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1)").bind(item.id).bind(baseline_id).fetch_one(&mut *verify).await.unwrap();
    assert_eq!(state, ("intent".to_owned(), "frozen".to_owned(), 0));
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}
#[tokio::test]
async fn publication_expiry_rolls_back() {
    blocked_publication(false).await;
}
#[tokio::test]
async fn publication_cancellation_rolls_back() {
    blocked_publication(true).await;
}
