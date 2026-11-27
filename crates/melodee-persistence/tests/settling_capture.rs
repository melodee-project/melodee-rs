//! Fenced incremental snapshot evidence and interrupted-capture recovery.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb};
use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    DiscoveryScanFence, SettlingObservation, SettlingSnapshotFile, StagedSnapshotCompletion,
    VerifiedStagedSnapshot, WorkItemStore,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn fixture(db: &TestDb) -> (Uuid, DiscoveryScanFence) {
    let item = ReleaseWorkItem::discovered("release".into(), None, Uuid::now_v7(), T0);
    WorkItemStore.create(&db.pool, &item).await.unwrap();
    let fence = DiscoveryScanFence {
        scope_key: "default".into(),
        generation: Uuid::now_v7(),
        owner: Uuid::now_v7(),
    };
    sqlx::query("INSERT INTO ingestion_discovery_scan(scope_key,generation,owner,lease_expires_at,birth_cutoff) VALUES($1,$2,$3,clock_timestamp()+interval '5 minutes',clock_timestamp())")
        .bind(&fence.scope_key).bind(fence.generation).bind(fence.owner).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2 WHERE id=$1")
        .bind(item.id)
        .bind(fence.generation)
        .execute(&db.pool)
        .await
        .unwrap();
    (item.id, fence)
}

fn row(path: String, n: i64) -> SettlingSnapshotFile {
    SettlingSnapshotFile {
        relative_path: path,
        file_type: "file".into(),
        file_size: n,
        modified_at: OffsetDateTime::UNIX_EPOCH + Duration::nanoseconds(123_456_789 + n),
    }
}

fn signature(rows: &[SettlingSnapshotFile]) -> String {
    let mut h = blake3::Hasher::new();
    for file in rows {
        h.update(
            format!(
                "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                file.relative_path,
                file.file_type,
                file.file_size,
                file.modified_at.unix_timestamp_nanos()
            )
            .as_bytes(),
        );
    }
    format!("melodee-snapshot-v2-blake3:{}", h.finalize().to_hex())
}

async fn publish(
    pool: &sqlx::PgPool,
    item: Uuid,
    verified: &VerifiedStagedSnapshot,
    fence: &DiscoveryScanFence,
) {
    let expected = WorkItemStore.get(pool, item).await.unwrap();
    let observation = SettlingObservation {
        expected: &expected,
        staged: Some(StagedSnapshotCompletion {
            verified,
            ready_marker_seen: false,
            bypassed_settling: false,
        }),
        next_state: None,
        transition_reason: "test",
        state_reason: "test",
        stable_since: None,
        latest_snapshot_id: Some(verified.snapshot_id()),
        observed_at: T0,
    };
    WorkItemStore
        .apply_settling_observation_fenced(pool, &observation, fence)
        .await
        .unwrap();
}

#[tokio::test]
async fn chunked_capture_is_hidden_revalidated_and_recovers_after_owner_loss() {
    let db = TestDb::create("snapshot_staging").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    assert!(staged.newly_created);
    let rows: Vec<_> = (0..260).map(|n| row(format!("{n:04}.flac"), n)).collect();
    for page in rows.chunks(128) {
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, page, &fence)
            .await
            .unwrap();
    }
    store
        .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &rows[..128], &fence)
        .await
        .unwrap();
    let mut conflict = rows[0].clone();
    conflict.file_size += 1;
    assert!(
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &[conflict], &fence)
            .await
            .is_err()
    );
    assert!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .is_none()
    );
    let first = store
        .staged_snapshot_files_page(&db.pool, staged.snapshot_id, item, None, &fence)
        .await
        .unwrap();
    assert_eq!(first.len(), 128);
    assert_eq!(first[0].modified_at.unix_timestamp_nanos(), 123_456_789);
    let second = store
        .staged_snapshot_files_page(
            &db.pool,
            staged.snapshot_id,
            item,
            Some(&first[127].relative_path),
            &fence,
        )
        .await
        .unwrap();
    assert_eq!(second[0].relative_path, "0128.flac");
    let sealed = store
        .seal_staged_snapshot(&db.pool, staged.snapshot_id, item, &fence)
        .await
        .unwrap();
    assert_eq!(sealed.signature(), signature(&rows));
    assert_eq!((sealed.file_count(), sealed.total_bytes()), (260, 33670));
    assert!(
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &rows[..1], &fence)
            .await
            .is_err()
    );
    publish(&db.pool, item, &sealed, &fence).await;
    let budget: (i64, i64) = sqlx::query_as(
        "SELECT active_rows,active_bytes FROM ingestion_settling_capture_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0), "publication releases staged quota");
    assert_eq!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .unwrap()
            .signature,
        signature(&rows)
    );

    let interrupted = store
        .begin_staged_snapshot(&db.pool, item, T0 + Duration::seconds(1), &fence)
        .await
        .unwrap();
    for page in rows.chunks(128) {
        store
            .append_staged_snapshot_files(&db.pool, interrupted.snapshot_id, item, page, &fence)
            .await
            .unwrap();
    }
    let replacement = DiscoveryScanFence {
        owner: Uuid::now_v7(),
        ..fence.clone()
    };
    sqlx::query("UPDATE ingestion_discovery_scan SET owner=$1 WHERE scope_key=$2")
        .bind(replacement.owner)
        .bind(&replacement.scope_key)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .append_staged_snapshot_files(
                &db.pool,
                interrupted.snapshot_id,
                item,
                &rows[..1],
                &fence
            )
            .await
            .is_err()
    );
    let resume = store
        .begin_staged_snapshot(&db.pool, item, T0, &replacement)
        .await
        .unwrap();
    assert!(!resume.newly_created);
    assert_eq!(resume.snapshot_id, interrupted.snapshot_id);
    let mut pages = 0;
    loop {
        pages += 1;
        if store
            .discard_staged_snapshot_page(&db.pool, resume.snapshot_id, item, &replacement)
            .await
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(pages, 4);
    assert_eq!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .unwrap()
            .id,
        staged.snapshot_id
    );
    assert!(
        store
            .begin_staged_snapshot(&db.pool, item, T0, &replacement)
            .await
            .unwrap()
            .newly_created
    );
    db.drop().await;
}

#[tokio::test]
async fn source_or_work_item_change_fences_capture() {
    let db = TestDb::create("snapshot_binding").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    sqlx::query("UPDATE release_work_items SET source_path='changed' WHERE id=$1")
        .bind(item)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .append_staged_snapshot_files(
                &db.pool,
                staged.snapshot_id,
                item,
                &[row("a".into(), 1)],
                &fence
            )
            .await
            .is_err()
    );
    assert!(
        store
            .seal_staged_snapshot(&db.pool, staged.snapshot_id, item, &fence)
            .await
            .is_err()
    );
    assert!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .discard_staged_snapshot_page(&db.pool, staged.snapshot_id, item, &fence)
            .await
            .unwrap()
    );
    db.drop().await;
}

#[tokio::test]
async fn expired_scope_cleanup_is_bounded_and_fences_reacquisition() {
    let db = TestDb::create("snapshot_expired").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    let rows: Vec<_> = (0..129).map(|n| row(format!("{n:04}"), n)).collect();
    assert!(
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &rows, &fence)
            .await
            .is_err()
    );
    for page in rows.chunks(128) {
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, page, &fence)
            .await
            .unwrap();
    }
    store
        .cleanup_expired_staged_snapshot_page(&db.pool, true)
        .await
        .unwrap();
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1",
    )
    .bind(staged.snapshot_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, 129, "unexpired capture remains untouched");
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key=$1")
        .bind(&fence.scope_key).execute(&db.pool).await.unwrap();
    store
        .cleanup_expired_staged_snapshot_page(&db.pool, true)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1",
    )
    .bind(staged.snapshot_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1, "cleanup removes at most one page");
    let successor = DiscoveryScanFence {
        owner: Uuid::now_v7(),
        ..fence.clone()
    };
    sqlx::query("UPDATE ingestion_discovery_scan SET owner=$1, lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE scope_key=$2")
        .bind(successor.owner).bind(&successor.scope_key).execute(&db.pool).await.unwrap();
    let found = store
        .begin_staged_snapshot(&db.pool, item, T0, &successor)
        .await
        .unwrap();
    assert!(
        found.newly_created,
        "discarding capture does not block a new active capture"
    );
    assert_ne!(found.snapshot_id, staged.snapshot_id);
    assert!(
        store
            .append_staged_snapshot_files(
                &db.pool,
                staged.snapshot_id,
                item,
                &rows[..1],
                &successor
            )
            .await
            .is_err()
    );
    assert!(
        !store
            .discard_staged_snapshot_page(&db.pool, staged.snapshot_id, item, &successor)
            .await
            .unwrap()
    );
    assert!(
        store
            .discard_staged_snapshot_page(&db.pool, staged.snapshot_id, item, &successor)
            .await
            .unwrap()
    );
    db.drop().await;
}

#[tokio::test]
async fn populated_upgrade_preserves_existing_snapshot_evidence() {
    use melodee_persistence::{MIGRATOR, migrate};
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("snapshot_upgrade").await;
    let prior = MIGRATOR
        .migrations
        .iter()
        .filter(|m| m.version <= 67)
        .cloned()
        .collect();
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .unwrap();
    let item = ReleaseWorkItem::discovered("historical".into(), None, Uuid::now_v7(), T0);
    // Old-engine-faithful raw insert: the current guarded create requires the
    // 0071 guard schema the subject upgrade creates.
    common::seed_raw_work_item(&db.pool, &item).await;
    let id: i64 = sqlx::query_scalar("INSERT INTO release_work_item_settling_snapshots(work_item_id,observed_at,signature,file_count,total_bytes,ready_marker_seen,bypassed_settling) VALUES($1,$2,'legacy',1,7,false,false) RETURNING id")
        .bind(item.id).bind(T0).fetch_one(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO release_work_item_settling_snapshot_files(snapshot_id,relative_path,file_type,file_size,modified_at) VALUES($1,'legacy.flac','file',7,$2)")
        .bind(id).bind(T0).execute(&db.pool).await.unwrap();
    migrate(&db.pool).await.unwrap();
    let latest = WorkItemStore
        .latest_snapshot(&db.pool, item.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (latest.id, latest.signature.as_str(), latest.total_bytes),
        (id, "legacy", 7)
    );
    let row: (String, i64, Option<String>) = sqlx::query_as("SELECT relative_path,file_size,modified_at_nanos FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1")
        .bind(id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(row, ("legacy.flac".into(), 7, None));
    migrate(&db.pool).await.unwrap();
    db.drop().await;
}

#[tokio::test]
async fn failed_page_rolls_back_without_hiding_previous_snapshot() {
    let db = TestDb::create("snapshot_disk_full").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    let old = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    let original = [row("old".into(), 5)];
    store
        .append_staged_snapshot_files(&db.pool, old.snapshot_id, item, &original, &fence)
        .await
        .unwrap();
    let sealed = store
        .seal_staged_snapshot(&db.pool, old.snapshot_id, item, &fence)
        .await
        .unwrap();
    publish(&db.pool, item, &sealed, &fence).await;
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0 + Duration::seconds(1), &fence)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION test_snapshot_disk_full() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.relative_path = 'fail' THEN RAISE EXCEPTION 'simulated disk full' USING ERRCODE='53100'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_snapshot_disk_full BEFORE INSERT ON release_work_item_settling_snapshot_files FOR EACH ROW EXECUTE FUNCTION test_snapshot_disk_full();")
        .execute(&db.pool).await.unwrap();
    let page = [row("first".into(), 1), row("fail".into(), 2)];
    assert!(
        store
            .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &page, &fence)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1",
    )
    .bind(staged.snapshot_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "a failed chunk cannot partially commit");
    assert_eq!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .unwrap()
            .id,
        old.snapshot_id
    );
    sqlx::query(
        "DROP TRIGGER test_snapshot_disk_full ON release_work_item_settling_snapshot_files",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    store
        .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &page, &fence)
        .await
        .unwrap();
    assert!(
        !store
            .discard_staged_snapshot_page(&db.pool, staged.snapshot_id, item, &fence)
            .await
            .unwrap()
    );
    assert!(
        store
            .discard_staged_snapshot_page(&db.pool, staged.snapshot_id, item, &fence)
            .await
            .unwrap()
    );
    db.drop().await;
}

#[tokio::test]
async fn worker_claim_prevents_snapshot_publication_and_progress_update() {
    use melodee_domain::IngestionState;
    use melodee_persistence::{WorkEngineConfig, WorkerId};

    let db = TestDb::create("snapshot_claim_race").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    store
        .system_transition(&db.pool, item, IngestionState::Settling, Some("test"), T0)
        .await
        .unwrap();
    store
        .system_transition(&db.pool, item, IngestionState::Ready, Some("test"), T0)
        .await
        .unwrap();
    let expected = store.get(&db.pool, item).await.unwrap();
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    let files = [row("one".into(), 1)];
    store
        .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &files, &fence)
        .await
        .unwrap();
    let sealed = store
        .seal_staged_snapshot(&db.pool, staged.snapshot_id, item, &fence)
        .await
        .unwrap();
    let claimed = store
        .claim(&db.pool, &WorkerId::new(), &WorkEngineConfig::default(), T0)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let observation = SettlingObservation {
        expected: &expected,
        staged: Some(StagedSnapshotCompletion {
            verified: &sealed,
            ready_marker_seen: false,
            bypassed_settling: false,
        }),
        next_state: Some(IngestionState::Settling),
        transition_reason: "snapshot-changed",
        state_reason: "snapshot-changed",
        stable_since: Some(T0),
        latest_snapshot_id: Some(staged.snapshot_id),
        observed_at: T0,
    };
    assert!(
        store
            .apply_settling_observation_fenced(&db.pool, &observation, &fence)
            .await
            .is_err()
    );
    assert!(
        store
            .upsert_settling_progress_fenced(
                &db.pool,
                item,
                Some(T0),
                Some(staged.snapshot_id),
                T0,
                Some("snapshot-error"),
                Some(&fence)
            )
            .await
            .is_err()
    );
    assert!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get(&db.pool, item)
            .await
            .unwrap()
            .latest_settling_snapshot_id
            .is_none()
    );
    db.drop().await;
}

#[tokio::test]
async fn concurrent_scope_quota_backpressure_can_be_extended_and_released() {
    let db = TestDb::create("snapshot_quota").await;
    let (first_item, first_fence) = fixture(&db).await;
    let source = Uuid::now_v7();
    let second = ReleaseWorkItem::discovered("second".into(), Some(source), Uuid::now_v7(), T0);
    WorkItemStore.create(&db.pool, &second).await.unwrap();
    let second_fence = DiscoveryScanFence {
        scope_key: source.to_string(),
        generation: Uuid::now_v7(),
        owner: Uuid::now_v7(),
    };
    sqlx::query("INSERT INTO ingestion_discovery_scan(scope_key,generation,owner,lease_expires_at,birth_cutoff) VALUES($1,$2,$3,clock_timestamp()+interval '5 minutes',clock_timestamp())")
        .bind(&second_fence.scope_key).bind(second_fence.generation).bind(second_fence.owner).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2 WHERE id=$1")
        .bind(second.id)
        .bind(second_fence.generation)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ingestion_settling_capture_budget SET max_rows=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let first = store
        .begin_staged_snapshot(&db.pool, first_item, T0, &first_fence)
        .await
        .unwrap();
    let second_capture = store
        .begin_staged_snapshot(&db.pool, second.id, T0, &second_fence)
        .await
        .unwrap();
    let first_row = [row("one".into(), 1)];
    let second_row = [row("two".into(), 2)];
    let (a, b) = tokio::join!(
        store.append_staged_snapshot_files(
            &db.pool,
            first.snapshot_id,
            first_item,
            &first_row,
            &first_fence
        ),
        store.append_staged_snapshot_files(
            &db.pool,
            second_capture.snapshot_id,
            second.id,
            &second_row,
            &second_fence
        )
    );
    assert_ne!(a.is_ok(), b.is_ok(), "only one page fits the global budget");
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(melodee_persistence::WorkStoreError::SettlingCaptureBudgetExhausted)
    ));
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT active_rows,active_bytes FROM ingestion_settling_capture_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(counts.0, 1);
    assert!(counts.1 > 0);
    sqlx::query("UPDATE ingestion_settling_capture_budget SET max_rows=2 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    if a.is_err() {
        store
            .append_staged_snapshot_files(
                &db.pool,
                first.snapshot_id,
                first_item,
                &first_row,
                &first_fence,
            )
            .await
            .unwrap();
    }
    if b.is_err() {
        store
            .append_staged_snapshot_files(
                &db.pool,
                second_capture.snapshot_id,
                second.id,
                &second_row,
                &second_fence,
            )
            .await
            .unwrap();
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT active_rows FROM ingestion_settling_capture_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    sqlx::query("DELETE FROM release_work_item_settling_snapshots WHERE id=$1")
        .bind(first.snapshot_id)
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .mark_staged_snapshot_discarding(
            &db.pool,
            second_capture.snapshot_id,
            second.id,
            &second_fence,
        )
        .await
        .unwrap();
    store
        .cleanup_expired_staged_snapshot_page(&db.pool, true)
        .await
        .unwrap();
    store
        .cleanup_expired_staged_snapshot_page(&db.pool, true)
        .await
        .unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT active_rows,active_bytes FROM ingestion_settling_capture_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "deletion, cascade, and active-scope GC release quota"
    );
    sqlx::query("UPDATE ingestion_settling_capture_budget SET max_bytes=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .begin_staged_snapshot(&db.pool, first_item, T0, &first_fence)
            .await,
        Err(melodee_persistence::WorkStoreError::SettlingCaptureBudgetExhausted)
    ));
    sqlx::query(
        "UPDATE ingestion_settling_capture_budget SET max_bytes=512,max_captures=1 WHERE singleton",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let large = store
        .begin_staged_snapshot(&db.pool, first_item, T0, &first_fence)
        .await
        .unwrap();
    assert!(
        store
            .append_staged_snapshot_files(
                &db.pool,
                large.snapshot_id,
                first_item,
                &first_row,
                &first_fence
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE ingestion_settling_capture_budget SET max_bytes=4096 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .append_staged_snapshot_files(
            &db.pool,
            large.snapshot_id,
            first_item,
            &first_row,
            &first_fence,
        )
        .await
        .unwrap();
    assert!(
        matches!(
            store
                .begin_staged_snapshot(&db.pool, second.id, T0, &second_fence)
                .await,
            Err(melodee_persistence::WorkStoreError::SettlingCaptureBudgetExhausted)
        ),
        "empty captures count toward the global budget"
    );
    sqlx::query("UPDATE ingestion_settling_capture_budget SET max_captures=2 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .begin_staged_snapshot(&db.pool, second.id, T0, &second_fence)
            .await
            .unwrap()
            .newly_created
    );
    db.drop().await;
}

#[tokio::test]
async fn successor_cleans_other_items_capture_and_orphaned_discard_after_scan_finishes() {
    let db = TestDb::create("snapshot_cross_item").await;
    let (first_item, fence) = fixture(&db).await;
    let second = ReleaseWorkItem::discovered("second".into(), None, Uuid::now_v7(), T0);
    WorkItemStore.create(&db.pool, &second).await.unwrap();
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2 WHERE id=$1")
        .bind(second.id)
        .bind(fence.generation)
        .execute(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let first = store
        .begin_staged_snapshot(&db.pool, first_item, T0, &fence)
        .await
        .unwrap();
    let rows: Vec<_> = (0..129).map(|n| row(format!("{n:04}"), n)).collect();
    for page in rows.chunks(128) {
        store
            .append_staged_snapshot_files(&db.pool, first.snapshot_id, first_item, page, &fence)
            .await
            .unwrap();
    }
    let recovered = store
        .begin_staged_snapshot(&db.pool, second.id, T0, &fence)
        .await
        .unwrap();
    assert!(!recovered.newly_created);
    assert_eq!(recovered.work_item_id, first_item);
    assert!(
        store
            .append_staged_snapshot_files(
                &db.pool,
                recovered.snapshot_id,
                second.id,
                &rows[..1],
                &fence
            )
            .await
            .is_err()
    );
    store
        .mark_staged_snapshot_discarding(
            &db.pool,
            recovered.snapshot_id,
            recovered.work_item_id,
            &fence,
        )
        .await
        .unwrap();
    let next = store
        .begin_staged_snapshot(&db.pool, second.id, T0, &fence)
        .await
        .unwrap();
    assert!(next.newly_created);
    for page in rows.chunks(128) {
        store
            .append_staged_snapshot_files(&db.pool, next.snapshot_id, second.id, page, &fence)
            .await
            .unwrap();
    }
    store
        .mark_staged_snapshot_discarding(&db.pool, next.snapshot_id, second.id, &fence)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ingestion_discovery_scan WHERE scope_key=$1")
        .bind(&fence.scope_key)
        .execute(&db.pool)
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(
            store
                .cleanup_expired_staged_snapshot_page(&db.pool, false)
                .await
                .unwrap()
        );
    }
    for id in [first.snapshot_id, next.snapshot_id] {
        let rows_left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1",
        )
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            rows_left, 1,
            "cleanup rotates after each page so neither capture starves"
        );
    }
    for _ in 0..5 {
        store
            .cleanup_expired_staged_snapshot_page(&db.pool, true)
            .await
            .unwrap();
    }
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_settling_snapshots WHERE NOT capture_complete",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        left, 0,
        "queued captures are reclaimable even after successful scan removes its scope row"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT active_rows FROM ingestion_settling_capture_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    db.drop().await;
}

#[tokio::test]
async fn completion_publishes_progress_and_transition_together() {
    use melodee_domain::IngestionState;
    let db = TestDb::create("snapshot_atomic").await;
    let (item, fence) = fixture(&db).await;
    let store = WorkItemStore;
    let expected = store.get(&db.pool, item).await.unwrap();
    let staged = store
        .begin_staged_snapshot(&db.pool, item, T0, &fence)
        .await
        .unwrap();
    let rows = [row("one".into(), 1)];
    store
        .append_staged_snapshot_files(&db.pool, staged.snapshot_id, item, &rows, &fence)
        .await
        .unwrap();
    let sealed = store
        .seal_staged_snapshot(&db.pool, staged.snapshot_id, item, &fence)
        .await
        .unwrap();
    let observation = SettlingObservation {
        expected: &expected,
        staged: Some(StagedSnapshotCompletion {
            verified: &sealed,
            ready_marker_seen: true,
            bypassed_settling: false,
        }),
        next_state: Some(IngestionState::Settling),
        transition_reason: "ready-marker-reset",
        state_reason: "ready-marker",
        stable_since: Some(T0),
        latest_snapshot_id: Some(staged.snapshot_id),
        observed_at: T0,
    };
    let updated = store
        .apply_settling_observation_fenced(&db.pool, &observation, &fence)
        .await
        .unwrap();
    assert_eq!(updated.state, IngestionState::Settling);
    assert_eq!(updated.state_reason.as_deref(), Some("ready-marker"));
    assert_eq!(
        updated.latest_settling_snapshot_id,
        Some(staged.snapshot_id)
    );
    assert_eq!(
        store
            .latest_snapshot(&db.pool, item)
            .await
            .unwrap()
            .unwrap()
            .signature,
        signature(&rows)
    );
    let reason: String = sqlx::query_scalar("SELECT reason_code FROM release_work_item_events WHERE work_item_id=$1 AND to_state='settling' ORDER BY id DESC LIMIT 1").bind(item).fetch_one(&db.pool).await.unwrap();
    assert_eq!(reason, "ready-marker-reset");
    db.drop().await;
}
