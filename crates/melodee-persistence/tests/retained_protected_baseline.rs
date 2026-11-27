//! Provisional protected-baseline metadata; actual fs-verity and namespace
//! verification are performed independently by the ingestion capability.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    CustodyHashObservation, CustodyHashPurpose, RetainedBaselineChunk, RetainedFileVerity,
    WorkEngineConfig, WorkItemStore, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn protected_freeze_requires_complete_immutable_verity_records_and_replays() {
    let db = common::TestDb::create("baseline_verity").await;
    let now = OffsetDateTime::now_utc();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .unwrap();
    let stage = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Staging,
            "stage-generation",
            "stage",
            now,
        )
        .await
        .unwrap();
    let entry = CustodyHashObservation {
        path_key: "file".to_owned(),
        parent_key: String::new(),
        name_key: "66696c65".to_owned(),
        legacy_path_hex: "6100".to_owned(),
        is_directory: false,
        legacy_length: 1,
        digest: "a".repeat(64),
    };
    store
        .append_custody_hash_spool(&db.pool, stage, item.id, &worker, &[entry], now)
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 1, now)
        .await
        .unwrap();
    let seal = store
        .seal_custody_hash_spool(&db.pool, stage, item.id, &worker)
        .await
        .unwrap();
    let id = store
        .begin_retained_baseline(&db.pool, &worker, &seal)
        .await
        .unwrap();
    let file = store
        .begin_retained_baseline_file(&db.pool, &worker, id, &seal, "file")
        .await
        .unwrap();
    let evidence = RetainedFileVerity {
        device: u64::MAX,
        inode: u64::MAX - 1,
        digest: [0x89; 32],
    };
    assert!(
        store
            .record_retained_baseline_file_verity(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &evidence
            )
            .await
            .is_err()
    );
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            id,
            &seal,
            file.ordinal,
            &[RetainedBaselineChunk {
                index: 0,
                length: 1,
                digest: "b".repeat(64),
            }],
        )
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(&db.pool, &worker, id, &seal, file.ordinal, &"a".repeat(64))
        .await
        .unwrap();
    assert!(
        store
            .freeze_protected_retained_baseline(&db.pool, &worker, id, &seal)
            .await
            .is_err()
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE id=$1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(state, "capturing");
    store
        .record_retained_baseline_file_verity(&db.pool, &worker, id, &seal, file.ordinal, &evidence)
        .await
        .unwrap();
    store
        .record_retained_baseline_file_verity(&db.pool, &worker, id, &seal, file.ordinal, &evidence)
        .await
        .unwrap();
    assert_eq!(
        store
            .retained_baseline_file_verity(&db.pool, &worker, id, &seal, file.ordinal)
            .await
            .unwrap(),
        evidence
    );
    let conflicting = RetainedFileVerity {
        inode: 1,
        ..evidence.clone()
    };
    assert!(
        store
            .record_retained_baseline_file_verity(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &conflicting
            )
            .await
            .is_err()
    );
    let protected = store
        .freeze_protected_retained_baseline(&db.pool, &worker, id, &seal)
        .await
        .unwrap();
    assert_eq!(protected.baseline().id(), id);
    assert_eq!(
        store
            .freeze_protected_retained_baseline(&db.pool, &worker, id, &seal)
            .await
            .unwrap()
            .baseline()
            .id(),
        id
    );
    assert_eq!(
        store
            .retained_baseline_file_verity(&db.pool, &worker, id, &seal, file.ordinal)
            .await
            .unwrap(),
        evidence
    );
    assert!(sqlx::query("UPDATE inbound_retained_baseline_files SET stage_verity_inode=repeat('0',16) WHERE baseline_id=$1").bind(id).execute(&db.pool).await.is_err());
    assert!(
        sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative' WHERE id=$1")
            .bind(id)
            .execute(&db.pool)
            .await
            .is_err()
    );
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at,retained_source) VALUES($1,'fixture','fixture','staged','verified_copy',repeat('a',64),clock_timestamp(),clock_timestamp(),true)").bind(item.id).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative',lifecycle_lock_name=('stage-' || replace(id::text,'-','') || '.lock'),lifecycle_root_generation='fixture-parent',lifecycle_lock_generation='fixture-lock' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let rejected=sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'source','fixture',$3,clock_timestamp(),clock_timestamp(),clock_timestamp())").bind(item.id).bind(id).bind(format!("v3:{}","f".repeat(64))).execute(&mut *tx).await;
    assert!(rejected.is_err());
    tx.rollback().await.unwrap();
    let rolled_back:(String,i64)=sqlx::query_as("SELECT state,(SELECT count(*) FROM release_work_item_staging_handoffs WHERE work_item_id=$2) FROM inbound_retained_baselines WHERE id=$1").bind(id).bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(rolled_back, ("frozen".to_owned(), 0));
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbound_retained_sources WHERE work_item_id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(receipts, 0);
    // Model a crashed worker after provisional freeze. Cleanup is page-bounded
    // and cannot remove the independent work item or source obligation.
    sqlx::query("UPDATE release_work_items SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE inbound_retained_baselines SET updated_at=clock_timestamp()-interval '2 hours' WHERE id=$1").bind(id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .retained_baseline_file_verity(&db.pool, &worker, id, &seal, file.ordinal)
            .await
            .is_err()
    );
    assert!(
        store
            .cleanup_abandoned_retained_baseline_page(&db.pool)
            .await
            .unwrap()
    );
    let work:(bool,i64)=sqlx::query_as("SELECT inbound_active,(SELECT count(*) FROM inbound_retained_baselines WHERE id=$2) FROM release_work_items WHERE id=$1").bind(item.id).bind(id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(work, (true, 0));
    let budget: (i64, i64, i64) = sqlx::query_as(
        "SELECT captures,rows,bytes FROM inbound_retained_baseline_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0, 0));
    db.drop().await;
}
