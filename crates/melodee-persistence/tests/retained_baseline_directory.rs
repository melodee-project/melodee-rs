//! Metadata-only retained directory cursor and membership evidence.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    CustodyHashObservation, CustodyHashPurpose, RetainedBaselineChunk, RetainedBaselineRead,
    RetainedDirectoryAdvance, RetainedDirectoryContinuation, RetainedDirectoryMember,
    WorkEngineConfig, WorkItemStore, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
fn entry(
    path: &str,
    parent: &str,
    digest: char,
    is_dir: bool,
    length: i64,
) -> CustodyHashObservation {
    CustodyHashObservation {
        path_key: path.to_owned(),
        parent_key: parent.to_owned(),
        name_key: "6161".to_owned(),
        legacy_path_hex: "6100".to_owned(),
        is_directory: is_dir,
        legacy_length: length,
        digest: digest.to_string().repeat(64),
    }
}
async fn make_due(pool: &sqlx::PgPool, id: Uuid) {
    sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(id).execute(pool).await.unwrap();
}
async fn claim(pool: &sqlx::PgPool, worker: &WorkerId) -> melodee_persistence::RetainedSourceLease {
    WorkItemStore
        .claim_due_retained_sources(pool, worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0)
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
async fn verified_directories_exact_membership_cursors_and_fair_turns() {
    let db = common::TestDb::create("baseline_dirs").await;
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
            "stage-gen",
            "stage",
            now,
        )
        .await
        .unwrap();
    let entries = [
        entry("rootfile", "", 'a', false, 1),
        entry("dir", "", 'b', true, 0),
        entry("dir/x", "dir", 'c', false, 1),
        entry("dir/empty", "dir", 'd', true, 0),
    ];
    store
        .append_custody_hash_spool(&db.pool, stage, item.id, &worker, &entries, now)
        .await
        .unwrap();
    store
        .freeze_custody_hash_spool(&db.pool, stage, item.id, &worker, 4, now)
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
    for (path, digest) in [("rootfile", 'a'), ("dir/x", 'c')] {
        let file = store
            .begin_retained_baseline_file(&db.pool, &worker, baseline, &seal, path)
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
                    digest: "e".repeat(64),
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
                &digest.to_string().repeat(64),
            )
            .await
            .unwrap();
    }
    assert!(
        store
            .freeze_retained_baseline(&db.pool, &worker, baseline, &seal)
            .await
            .is_err()
    );
    assert!(
        store
            .record_retained_baseline_directory(
                &db.pool,
                &worker,
                baseline,
                &seal,
                "dir",
                &"f".repeat(64)
            )
            .await
            .is_err()
    );
    for (path, digest) in [("dir", 'b'), ("dir/empty", 'd')] {
        store
            .record_retained_baseline_directory(
                &db.pool,
                &worker,
                baseline,
                &seal,
                path,
                &digest.to_string().repeat(64),
            )
            .await
            .unwrap();
        store
            .record_retained_baseline_directory(
                &db.pool,
                &worker,
                baseline,
                &seal,
                path,
                &digest.to_string().repeat(64),
            )
            .await
            .unwrap();
    }
    authorize_cursor_fixture(&db.pool, &worker, baseline, &seal, item.id).await;
    assert!(sqlx::query("UPDATE inbound_retained_baseline_directories SET path_key='renamed' WHERE baseline_id=$1 AND stage_ordinal>0").bind(baseline).execute(&db.pool).await.is_err());
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'source','generation',$3,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())").bind(item.id).bind(baseline).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let lease = claim(&db.pool, &worker).await;
    assert!(!lease.baseline_directory_turn());
    let RetainedBaselineRead::Page(file_page) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("expected file page")
    };
    store
        .acknowledge_retained_baseline_page(&db.pool, &lease, &file_page, false)
        .await
        .unwrap();
    store
        .yield_retained_source(&db.pool, &lease, Duration::milliseconds(1))
        .await
        .unwrap();
    make_due(&db.pool, item.id).await;
    let lease = claim(&db.pool, &worker).await;
    assert!(lease.baseline_directory_turn());
    let root = store
        .next_retained_baseline_directory(&db.pool, &lease)
        .await
        .unwrap();
    assert_eq!(root.path(), "");
    assert_eq!(root.cookie(), None);
    let members = [
        RetainedDirectoryMember {
            path_key: "rootfile".to_owned(),
            is_directory: false,
        },
        RetainedDirectoryMember {
            path_key: "dir".to_owned(),
            is_directory: true,
        },
        RetainedDirectoryMember {
            path_key: "dir".to_owned(),
            is_directory: false,
        },
        RetainedDirectoryMember {
            path_key: "dir/x".to_owned(),
            is_directory: false,
        },
        RetainedDirectoryMember {
            path_key: "unknown".to_owned(),
            is_directory: false,
        },
    ];
    assert_eq!(
        store
            .retained_directory_membership(&db.pool, &lease, &root, &members)
            .await
            .unwrap(),
        vec![true, true, false, false, false]
    );
    assert!(
        store
            .advance_retained_baseline_directory(
                &db.pool,
                &lease,
                &root,
                RetainedDirectoryAdvance {
                    continuation: RetainedDirectoryContinuation::More("invalid"),
                    generation: "gen1",
                    mismatch: false,
                    yield_after: Duration::milliseconds(1),
                    recheck_after: Duration::minutes(5)
                }
            )
            .await
            .is_err()
    );
    store
        .advance_retained_baseline_directory(
            &db.pool,
            &lease,
            &root,
            RetainedDirectoryAdvance {
                continuation: RetainedDirectoryContinuation::More("0000000000000010"),
                generation: "gen1",
                mismatch: false,
                yield_after: Duration::milliseconds(1),
                recheck_after: Duration::minutes(5),
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .advance_retained_baseline_directory(
                &db.pool,
                &lease,
                &root,
                RetainedDirectoryAdvance {
                    continuation: RetainedDirectoryContinuation::Eof,
                    generation: "gen1",
                    mismatch: false,
                    yield_after: Duration::milliseconds(1),
                    recheck_after: Duration::minutes(5)
                }
            )
            .await
            .is_err()
    );
    let mut seen = Vec::new();
    for _ in 0..3 {
        make_due(&db.pool, item.id).await;
        let lease = claim(&db.pool, &worker).await;
        assert!(!lease.baseline_directory_turn());
        let page = store
            .next_retained_baseline_directory(&db.pool, &lease)
            .await
            .unwrap();
        seen.push(page.path().to_owned());
        if page.path() == "" {
            assert_eq!(page.cookie(), Some("0000000000000010"));
            store
                .advance_retained_baseline_directory(
                    &db.pool,
                    &lease,
                    &page,
                    RetainedDirectoryAdvance {
                        continuation: RetainedDirectoryContinuation::More("0000000000000020"),
                        generation: "gen2",
                        mismatch: false,
                        yield_after: Duration::milliseconds(1),
                        recheck_after: Duration::minutes(5),
                    },
                )
                .await
                .unwrap();
        } else {
            store
                .advance_retained_baseline_directory(
                    &db.pool,
                    &lease,
                    &page,
                    RetainedDirectoryAdvance {
                        continuation: RetainedDirectoryContinuation::Eof,
                        generation: "gen1",
                        mismatch: false,
                        yield_after: Duration::milliseconds(1),
                        recheck_after: Duration::minutes(5),
                    },
                )
                .await
                .unwrap();
        }
    }
    assert!(seen.contains(&"dir".to_owned()));
    assert!(seen.contains(&"dir/empty".to_owned()));
    assert!(seen.contains(&String::new()));
    let root_state:(Option<String>,Option<String>)=sqlx::query_as("SELECT scan_cookie,scan_generation FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND stage_ordinal=0").bind(baseline).fetch_one(&db.pool).await.unwrap();
    assert_eq!(root_state, (None, Some("gen2".to_owned())));
    let signal: bool = sqlx::query_scalar(
        "SELECT baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(signal);
    make_due(&db.pool, item.id).await;
    let lease = claim(&db.pool, &worker).await;
    let page = store
        .next_retained_baseline_directory(&db.pool, &lease)
        .await
        .unwrap();
    assert!(
        store
            .unreadable_retained_baseline_directory(
                &db.pool,
                &lease,
                &page,
                melodee_persistence::RetainedBaselineUnreadableReason::Missing,
                Duration::ZERO
            )
            .await
            .is_err()
    );
    store
        .unreadable_retained_baseline_directory(
            &db.pool,
            &lease,
            &page,
            melodee_persistence::RetainedBaselineUnreadableReason::Missing,
            Duration::milliseconds(1),
        )
        .await
        .unwrap();
    assert!(
        store
            .unreadable_retained_baseline_directory(
                &db.pool,
                &lease,
                &page,
                melodee_persistence::RetainedBaselineUnreadableReason::Missing,
                Duration::milliseconds(1)
            )
            .await
            .is_err()
    );
    let row:(bool,Option<String>,i32,Option<Uuid>,bool)=sqlx::query_as("SELECT baseline_change_pending,baseline_unreadable_reason,baseline_unreadable_count,lease_token,baseline_directory_turn FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(row, (true, Some("missing".to_owned()), 1, None, false));
    let cursor:(Option<String>,Option<String>)=sqlx::query_as("SELECT scan_cookie,scan_generation FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND path_key=$2").bind(baseline).bind(page.path()).fetch_one(&db.pool).await.unwrap();
    assert_eq!(cursor, (None, None));
    let active: bool =
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(active);
    db.drop().await;
}

#[tokio::test]
async fn file_completion_and_directory_eof_have_independent_due_times() {
    let db = common::TestDb::create("baseline_due").await;
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
            "stage-gen",
            "stage",
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
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,'source','generation',$3,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())").bind(item.id).bind(baseline).bind(seal.digest()).execute(&db.pool).await.unwrap();
    let lease = claim(&db.pool, &worker).await;
    assert!(!lease.baseline_directory_turn());
    let RetainedBaselineRead::PassComplete(pass) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("expected empty pass")
    };
    store
        .complete_retained_baseline_pass(&db.pool, &lease, &pass, false, Duration::minutes(5))
        .await
        .unwrap();
    assert!(
        store
            .retained_baseline_next_page(&db.pool, &lease)
            .await
            .is_err()
    );
    store
        .yield_retained_baseline_source(&db.pool, &lease, Duration::milliseconds(1))
        .await
        .unwrap();
    let promptly_due:bool=sqlx::query_scalar("SELECT next_reobserve_at<clock_timestamp()+interval '2 seconds' AND baseline_file_due_at>clock_timestamp()+interval '4 minutes' FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert!(promptly_due);
    make_due(&db.pool, item.id).await;
    let lease = claim(&db.pool, &worker).await;
    assert!(lease.baseline_directory_turn());
    let root = store
        .next_retained_baseline_directory(&db.pool, &lease)
        .await
        .unwrap();
    store
        .advance_retained_baseline_directory(
            &db.pool,
            &lease,
            &root,
            RetainedDirectoryAdvance {
                continuation: RetainedDirectoryContinuation::Eof,
                generation: "gen1",
                mismatch: false,
                yield_after: Duration::milliseconds(1),
                recheck_after: Duration::minutes(5),
            },
        )
        .await
        .unwrap();
    let both_deferred:bool=sqlx::query_scalar("SELECT r.next_reobserve_at>clock_timestamp()+interval '4 minutes' AND r.baseline_file_due_at>clock_timestamp()+interval '4 minutes' AND d.next_scan_at>clock_timestamp()+interval '4 minutes' FROM inbound_retained_sources r JOIN inbound_retained_baseline_directories d ON d.baseline_id=r.baseline_id AND d.stage_ordinal=0 WHERE r.work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert!(both_deferred);
    assert!(
        store
            .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE inbound_retained_baseline_directories SET next_scan_at=clock_timestamp()-interval '1 second' WHERE baseline_id=$1 AND stage_ordinal=0").bind(baseline).execute(&db.pool).await.unwrap();
    make_due(&db.pool, item.id).await;
    let lease = claim(&db.pool, &worker).await;
    assert!(lease.baseline_directory_turn());
    let root = store
        .next_retained_baseline_directory(&db.pool, &lease)
        .await
        .unwrap();
    store
        .advance_retained_baseline_directory(
            &db.pool,
            &lease,
            &root,
            RetainedDirectoryAdvance {
                continuation: RetainedDirectoryContinuation::Invalidated,
                generation: "gen1",
                mismatch: false,
                yield_after: Duration::milliseconds(1),
                recheck_after: Duration::minutes(5),
            },
        )
        .await
        .unwrap();
    let invalidated:(bool,bool)=sqlx::query_as("SELECT r.baseline_change_pending,d.next_scan_at<=clock_timestamp() FROM inbound_retained_sources r JOIN inbound_retained_baseline_directories d ON d.baseline_id=r.baseline_id AND d.stage_ordinal=0 WHERE r.work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(invalidated, (true, true));
    sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()-interval '1 second',baseline_file_due_at=clock_timestamp()-interval '1 second',baseline_directory_turn=true WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let lease = claim(&db.pool, &worker).await;
    assert!(lease.baseline_directory_turn());
    assert!(
        store
            .defer_unsupported_retained_directories(
                &db.pool,
                &lease,
                Duration::minutes(30),
                Duration::ZERO
            )
            .await
            .is_err()
    );
    store
        .defer_unsupported_retained_directories(
            &db.pool,
            &lease,
            Duration::minutes(30),
            Duration::milliseconds(1),
        )
        .await
        .unwrap();
    assert!(
        store
            .defer_unsupported_retained_directories(
                &db.pool,
                &lease,
                Duration::minutes(30),
                Duration::milliseconds(1)
            )
            .await
            .is_err()
    );
    let unsupported:(Option<String>,bool,bool)=sqlx::query_as("SELECT baseline_directory_unavailable_reason,baseline_directory_blocked_until>clock_timestamp()+interval '29 minutes',next_reobserve_at<clock_timestamp()+interval '2 seconds' FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(unsupported, (Some("unsupported".to_owned()), true, true));
    make_due(&db.pool, item.id).await;
    let lease = claim(&db.pool, &worker).await;
    assert!(!lease.baseline_directory_turn());
    assert!(
        store
            .next_retained_baseline_directory(&db.pool, &lease)
            .await
            .is_err()
    );
    let RetainedBaselineRead::PassComplete(pass) = store
        .retained_baseline_next_page(&db.pool, &lease)
        .await
        .unwrap()
    else {
        panic!("file class must remain available")
    };
    store
        .complete_retained_baseline_pass(&db.pool, &lease, &pass, false, Duration::minutes(5))
        .await
        .unwrap();
    store
        .yield_retained_baseline_source(&db.pool, &lease, Duration::milliseconds(1))
        .await
        .unwrap();
    let due_file_first:bool=sqlx::query_scalar("SELECT next_reobserve_at>clock_timestamp()+interval '4 minutes' AND next_reobserve_at<clock_timestamp()+interval '6 minutes' FROM inbound_retained_sources WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert!(due_file_first);
    db.drop().await;
}
