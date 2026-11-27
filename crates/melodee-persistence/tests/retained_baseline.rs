//! Provisional baseline persistence; no fixture here establishes protected-stage proof.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    CustodyHashObservation, CustodyHashPurpose, RetainedBaselineChunk, WorkEngineConfig,
    WorkItemStore, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn observation(path: &str, length: i64, digest: char, directory: bool) -> CustodyHashObservation {
    CustodyHashObservation {
        path_key: path.to_owned(),
        parent_key: String::new(),
        name_key: if path == "file" {
            "66696c65"
        } else {
            "656d707479"
        }
        .to_owned(),
        legacy_path_hex: "6100".to_owned(),
        is_directory: directory,
        legacy_length: length,
        digest: digest.to_string().repeat(64),
    }
}
// Fixture only: these metadata records are synthetic and do not claim that
// an fs-verity or protected-namespace proof was performed.
async fn authorize_cursor_fixture(
    pool: &sqlx::PgPool,
    worker: &WorkerId,
    id: Uuid,
    seal: &melodee_persistence::SealedCustodyHash,
    item: Uuid,
) {
    let ordinals:Vec<i64>=sqlx::query_scalar("SELECT stage_ordinal FROM inbound_retained_baseline_files WHERE baseline_id=$1 ORDER BY stage_ordinal").bind(id).fetch_all(pool).await.unwrap();
    for ordinal in ordinals {
        let proof = melodee_persistence::RetainedFileVerity {
            device: 1,
            inode: u64::try_from(ordinal).unwrap(),
            digest: [42; 32],
        };
        WorkItemStore
            .record_retained_baseline_file_verity(pool, worker, id, seal, ordinal, &proof)
            .await
            .unwrap();
    }
    WorkItemStore
        .freeze_protected_retained_baseline(pool, worker, id, seal)
        .await
        .unwrap();
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at,retained_source) VALUES($1,'fixture','fixture','staged','verified_copy',repeat('a',64),clock_timestamp(),clock_timestamp(),true)").bind(item).execute(pool).await.unwrap();
    sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative',lifecycle_lock_name=('stage-' || replace(id::text,'-','') || '.lock'),lifecycle_root_generation='fixture-parent',lifecycle_lock_generation='fixture-lock' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}
#[tokio::test]
async fn baseline_requires_full_stage_coverage_and_replays_safely() {
    let db = common::TestDb::create("baseline_stage").await;
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
            "staging",
            now,
        )
        .await
        .unwrap();
    let entries = [
        observation("file", 65_537, 'a', false),
        observation("empty", 0, 'b', false),
    ];
    store
        .append_custody_hash_spool(&db.pool, stage, item.id, &worker, &entries, now)
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 2, now)
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
    assert_eq!(file.length, 65_537);
    assert!(
        store
            .freeze_retained_baseline(&db.pool, &worker, id, &seal)
            .await
            .is_err()
    );
    let chunk = RetainedBaselineChunk {
        index: 0,
        length: 65_536,
        digest: "c".repeat(64),
    };
    sqlx::query("UPDATE inbound_retained_baseline_budget SET max_rows=3 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    let oversized = [
        chunk.clone(),
        RetainedBaselineChunk {
            index: 1,
            length: 1,
            digest: "d".repeat(64),
        },
    ];
    assert!(
        store
            .append_retained_baseline_chunks(&db.pool, &worker, id, &seal, file.ordinal, &oversized)
            .await
            .is_err()
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT rows FROM inbound_retained_baseline_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows, 2);
    let inserted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbound_retained_baseline_chunks WHERE baseline_id=$1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(inserted, 0);
    sqlx::query("UPDATE inbound_retained_baseline_budget SET max_rows=1000000 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            id,
            &seal,
            file.ordinal,
            std::slice::from_ref(&chunk),
        )
        .await
        .unwrap();
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            id,
            &seal,
            file.ordinal,
            std::slice::from_ref(&chunk),
        )
        .await
        .unwrap();
    assert!(
        store
            .append_retained_baseline_chunks(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &[RetainedBaselineChunk {
                    digest: "d".repeat(64),
                    ..chunk
                }]
            )
            .await
            .is_err()
    );
    assert!(
        store
            .finish_retained_baseline_file(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &"a".repeat(64)
            )
            .await
            .is_err()
    );
    assert!(
        store
            .append_retained_baseline_chunks(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &[RetainedBaselineChunk {
                    index: 1,
                    length: 2,
                    digest: "d".repeat(64)
                }]
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
                index: 1,
                length: 1,
                digest: "d".repeat(64),
            }],
        )
        .await
        .unwrap();
    assert!(
        store
            .finish_retained_baseline_file(
                &db.pool,
                &worker,
                id,
                &seal,
                file.ordinal,
                &"f".repeat(64)
            )
            .await
            .is_err()
    );
    store
        .finish_retained_baseline_file(&db.pool, &worker, id, &seal, file.ordinal, &"a".repeat(64))
        .await
        .unwrap();
    let empty = store
        .begin_retained_baseline_file(&db.pool, &worker, id, &seal, "empty")
        .await
        .unwrap();
    assert_eq!(empty.length, 0);
    store
        .finish_retained_baseline_file(&db.pool, &worker, id, &seal, empty.ordinal, &"b".repeat(64))
        .await
        .unwrap();
    let frozen = store
        .freeze_retained_baseline(&db.pool, &worker, id, &seal)
        .await
        .unwrap();
    assert_eq!((frozen.file_count(), frozen.chunk_count()), (2, 2));
    assert_eq!(
        store
            .begin_retained_baseline(&db.pool, &worker, &seal)
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        store
            .freeze_retained_baseline(&db.pool, &worker, id, &seal)
            .await
            .unwrap()
            .id(),
        id
    );
    assert!(
        sqlx::query(
            "UPDATE inbound_retained_baseline_chunks SET digest=repeat('e',64) WHERE baseline_id=$1"
        )
        .bind(id)
        .execute(&db.pool)
        .await
        .is_err()
    );
    assert!(
        !store
            .cleanup_abandoned_retained_baseline_page(&db.pool)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE release_work_items SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE inbound_retained_baselines SET updated_at=clock_timestamp()-interval '2 hours' WHERE id=$1").bind(id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .cleanup_abandoned_retained_baseline_page(&db.pool)
            .await
            .unwrap()
    );
    let budget: (i64, i64, i64) = sqlx::query_as(
        "SELECT captures,rows,bytes FROM inbound_retained_baseline_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0, 0));
    db.drop().await;
}

#[tokio::test]
async fn retained_cursor_is_bounded_fenced_and_preserves_sticky_signal() {
    use melodee_persistence::RetainedBaselineRead;
    use time::Duration;
    let db = common::TestDb::create("baseline_cursor").await;
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
            "staging",
            now,
        )
        .await
        .unwrap();
    store
        .append_custody_hash_spool(
            &db.pool,
            stage,
            item.id,
            &worker,
            &[
                observation("file", 17 * 65_536, 'a', false),
                observation("empty", 0, 'b', false),
            ],
            now,
        )
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 2, now)
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
    let chunks: Vec<_> = (0..17)
        .map(|index| RetainedBaselineChunk {
            index,
            length: 65_536,
            digest: "c".repeat(64),
        })
        .collect();
    store
        .append_retained_baseline_chunks(&db.pool, &worker, id, &seal, file.ordinal, &chunks)
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(&db.pool, &worker, id, &seal, file.ordinal, &"a".repeat(64))
        .await
        .unwrap();
    let empty = store
        .begin_retained_baseline_file(&db.pool, &worker, id, &seal, "empty")
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(&db.pool, &worker, id, &seal, empty.ordinal, &"b".repeat(64))
        .await
        .unwrap();
    authorize_cursor_fixture(&db.pool, &worker, id, &seal, item.id).await;
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'source','generation',$3,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())").bind(item.id).bind(id).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let lease = store
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(lease.baseline_id(), Some(id));
    let mut seen = 0;
    let mut saw_empty = false;
    let mut stale_page = None;
    loop {
        match store
            .retained_baseline_next_page(&db.pool, &lease)
            .await
            .unwrap()
        {
            RetainedBaselineRead::Page(page) => {
                assert!(page.chunks().len() <= 16);
                if page.path() == "file" {
                    seen += page.chunks().len();
                } else {
                    assert_eq!(page.file_length(), 0);
                    saw_empty = true;
                }
                let mismatch = page.path() == "file" && page.chunks().iter().any(|c| c.index == 16);
                store
                    .acknowledge_retained_baseline_page(&db.pool, &lease, &page, mismatch)
                    .await
                    .unwrap();
                assert!(
                    store
                        .acknowledge_retained_baseline_page(&db.pool, &lease, &page, false)
                        .await
                        .is_err()
                );
                stale_page = Some(page);
            }
            RetainedBaselineRead::PassComplete(pass) => {
                store
                    .complete_retained_baseline_pass(
                        &db.pool,
                        &lease,
                        &pass,
                        false,
                        Duration::minutes(5),
                    )
                    .await
                    .unwrap();
                assert!(
                    store
                        .complete_retained_baseline_pass(
                            &db.pool,
                            &lease,
                            &pass,
                            false,
                            Duration::minutes(5)
                        )
                        .await
                        .is_err()
                );
                break;
            }
        }
    }
    assert_eq!(seen, 17);
    assert!(saw_empty);
    let state:(i64,bool)=sqlx::query_as("SELECT baseline_completed_passes,baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(state, (1, true));
    sqlx::query("UPDATE inbound_retained_sources SET lease_expires_at=clock_timestamp()-interval '1 second',next_reobserve_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let replacement = store
        .claim_due_retained_sources(&db.pool, &WorkerId::new(), 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .acknowledge_retained_baseline_page(&db.pool, &lease, &stale_page.unwrap(), false)
            .await
            .is_err()
    );
    assert!(replacement.baseline_directory_turn());
    assert!(
        store
            .retained_baseline_next_page(&db.pool, &replacement)
            .await
            .is_err()
    );
    assert!(
        store
            .next_retained_baseline_directory(&db.pool, &replacement)
            .await
            .is_ok()
    );
    assert!(
        store
            .yield_retained_source(&db.pool, &replacement, Duration::ZERO)
            .await
            .is_err()
    );
    store
        .yield_retained_source(&db.pool, &replacement, Duration::seconds(1))
        .await
        .unwrap();
    let yielded: (bool, i32, Option<String>, Option<Uuid>) = sqlx::query_as("SELECT baseline_change_pending,retry_count,last_retry_reason,lease_token FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(yielded, (true, 0, None, None));
    assert!(
        store
            .yield_retained_source(&db.pool, &replacement, Duration::seconds(1))
            .await
            .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn empty_baseline_pass_persists_generation_mismatch_as_sticky_signal() {
    use melodee_persistence::RetainedBaselineRead;
    use time::Duration;
    let db = common::TestDb::create("baseline_empty").await;
    let now = OffsetDateTime::now_utc();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let mut item =
        ReleaseWorkItem::discovered("empty-source".to_owned(), None, Uuid::now_v7(), now);
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
            "staging",
            now,
        )
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 0, now)
        .await
        .unwrap();
    let seal = store
        .seal_custody_hash_spool(&db.pool, stage, item.id, &worker)
        .await
        .unwrap();
    let baseline = store
        .begin_retained_baseline(&db.pool, &worker, &seal)
        .await
        .unwrap();
    authorize_cursor_fixture(&db.pool, &worker, baseline, &seal, item.id).await;
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'empty-source','generation',$3,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())").bind(item.id).bind(baseline).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let lease = store
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(lease.baseline_id(), Some(baseline));
    let RetainedBaselineRead::PassComplete(pass) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("empty baseline must complete without pages")
    };
    store
        .complete_retained_baseline_pass(&db.pool, &lease, &pass, true, Duration::minutes(5))
        .await
        .unwrap();
    assert!(
        store
            .complete_retained_baseline_pass(&db.pool, &lease, &pass, false, Duration::minutes(5))
            .await
            .is_err()
    );
    assert!(
        store
            .retained_baseline_next_page(&db.pool, &lease)
            .await
            .is_err()
    );
    sqlx::query("UPDATE inbound_retained_sources SET baseline_file_due_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let RetainedBaselineRead::PassComplete(next) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("empty baseline must wrap")
    };
    store
        .complete_retained_baseline_pass(&db.pool, &lease, &next, false, Duration::minutes(5))
        .await
        .unwrap();
    let state:(i64,bool)=sqlx::query_as("SELECT baseline_completed_passes,baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(state, (2, true));
    db.drop().await;
}

#[tokio::test]
async fn unreadable_page_yields_fairly_and_records_inconclusive_sticky_reason() {
    use melodee_persistence::{
        RetainedBaselineRead, RetainedBaselineUnreadableReason, RetainedRetryReason,
    };
    use time::Duration;
    let db = common::TestDb::create("baseline_unreadable").await;
    let now = OffsetDateTime::now_utc();
    let store = WorkItemStore;
    let worker = WorkerId::new();
    let mut item = ReleaseWorkItem::discovered("first".to_owned(), None, Uuid::now_v7(), now);
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
            "staging",
            now,
        )
        .await
        .unwrap();
    store
        .append_custody_hash_spool(
            &db.pool,
            stage,
            item.id,
            &worker,
            &[
                observation("file", 1, 'a', false),
                observation("empty", 0, 'b', false),
            ],
            now,
        )
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 2, now)
        .await
        .unwrap();
    let seal = store
        .seal_custody_hash_spool(&db.pool, stage, item.id, &worker)
        .await
        .unwrap();
    let baseline = store
        .begin_retained_baseline(&db.pool, &worker, &seal)
        .await
        .unwrap();
    let file = store
        .begin_retained_baseline_file(&db.pool, &worker, baseline, &seal, "file")
        .await
        .unwrap();
    store
        .append_retained_baseline_chunks(
            &db.pool,
            &worker,
            baseline,
            &seal,
            file.ordinal,
            &[RetainedBaselineChunk {
                index: 0,
                length: 1,
                digest: "c".repeat(64),
            }],
        )
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(
            &db.pool,
            &worker,
            baseline,
            &seal,
            file.ordinal,
            &"a".repeat(64),
        )
        .await
        .unwrap();
    let empty = store
        .begin_retained_baseline_file(&db.pool, &worker, baseline, &seal, "empty")
        .await
        .unwrap();
    store
        .finish_retained_baseline_file(
            &db.pool,
            &worker,
            baseline,
            &seal,
            empty.ordinal,
            &"b".repeat(64),
        )
        .await
        .unwrap();
    authorize_cursor_fixture(&db.pool, &worker, baseline, &seal, item.id).await;
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'first','generation',$3,clock_timestamp()-interval '2 seconds',clock_timestamp(),clock_timestamp())").bind(item.id).bind(baseline).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let other = ReleaseWorkItem::discovered("other".to_owned(), None, Uuid::now_v7(), now);
    store.create(&db.pool, &other).await.unwrap();
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'other','generation',$2,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())").bind(other.id).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let lease = store
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(lease.work_item_id(), item.id);
    let RetainedBaselineRead::Page(page) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("expected first page")
    };
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT work_item_id FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE",
    )
    .bind(item.id)
    .execute(&mut *lock)
    .await
    .unwrap();
    let pool = db.pool.clone();
    let cancel_lease = lease.clone();
    let cancel_page = page.clone();
    let cancelled = tokio::spawn(async move {
        WorkItemStore
            .unreadable_retained_baseline_page(
                &pool,
                &cancel_lease,
                &cancel_page,
                RetainedBaselineUnreadableReason::Missing,
                Duration::minutes(1),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT source_library_id,source_path,source_path_format%')").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.unwrap();
    cancelled.abort();
    let _ = cancelled.await;
    lock.commit().await.unwrap();
    let unmodified:(i64,bool,i32)=sqlx::query_as("SELECT baseline_cursor_revision,baseline_change_pending,baseline_unreadable_count FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(unmodified, (0, false, 0));
    assert!(
        store
            .unreadable_retained_baseline_page(
                &db.pool,
                &lease,
                &page,
                RetainedBaselineUnreadableReason::Unreadable,
                Duration::ZERO
            )
            .await
            .is_err()
    );
    store
        .unreadable_retained_baseline_page(
            &db.pool,
            &lease,
            &page,
            RetainedBaselineUnreadableReason::Unreadable,
            Duration::minutes(1),
        )
        .await
        .unwrap();
    assert!(
        store
            .unreadable_retained_baseline_page(
                &db.pool,
                &lease,
                &page,
                RetainedBaselineUnreadableReason::Unreadable,
                Duration::minutes(1)
            )
            .await
            .is_err()
    );
    assert!(
        store
            .acknowledge_retained_baseline_page(&db.pool, &lease, &page, false)
            .await
            .is_err()
    );
    let state:(String,bool,Option<String>,i32,i32,Option<Uuid>)=sqlx::query_as("SELECT status,baseline_change_pending,baseline_unreadable_reason,baseline_unreadable_count,retry_count,lease_token FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        state,
        (
            "active".to_owned(),
            true,
            Some("unreadable".to_owned()),
            1,
            0,
            None
        )
    );
    let peer = store
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(peer.work_item_id(), other.id);
    store
        .defer_retained_source(
            &db.pool,
            &peer,
            Duration::minutes(10),
            RetainedRetryReason::Other,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()-interval '1 second',baseline_unreadable_count=2147483647 WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let resumed = store
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    let RetainedBaselineRead::Page(next) = store
        .retained_baseline_next_page(&db.pool, &resumed)
        .await
        .unwrap()
    else {
        panic!("expected next page")
    };
    assert_ne!(next.path(), page.path());
    store
        .unreadable_retained_baseline_page(
            &db.pool,
            &resumed,
            &next,
            RetainedBaselineUnreadableReason::Missing,
            Duration::minutes(1),
        )
        .await
        .unwrap();
    let state:(String,bool,Option<String>,i32,Option<Uuid>)=sqlx::query_as("SELECT status,baseline_change_pending,baseline_unreadable_reason,baseline_unreadable_count,lease_token FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        state,
        (
            "active".to_owned(),
            true,
            Some("missing".to_owned()),
            i32::MAX,
            None
        )
    );
    let active: bool =
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(active);
    db.drop().await;
}
