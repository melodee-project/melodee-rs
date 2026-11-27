//! Independent retained-source lease, quota, and atomic supersession tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    CustodyHashObservation, CustodyHashPurpose, RetainedRecheckOutcome, RetainedRetryReason,
    RetainedSourceLease, WorkItemStore, WorkStoreError, WorkerId,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn observation(byte: char) -> CustodyHashObservation {
    CustodyHashObservation {
        path_key: "file".to_owned(),
        parent_key: String::new(),
        name_key: "66696c65".to_owned(),
        legacy_path_hex: "66696c6500".to_owned(),
        is_directory: false,
        legacy_length: 3,
        digest: byte.to_string().repeat(64),
    }
}

fn sealed_digest(entry: &CustodyHashObservation) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"melodee-custody-tree-v3\0");
    hasher.update(entry.digest.as_bytes());
    hasher.update(&1_u64.to_le_bytes());
    format!("v3:{}", hasher.finalize().to_hex())
}

async fn seed_format(pool: &PgPool, path: &str, baseline: &str, format: i16) -> ReleaseWorkItem {
    let now = OffsetDateTime::now_utc();
    let item = ReleaseWorkItem::discovered(path.to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore
        .create(pool, &item)
        .await
        .expect("create item");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_library_id,source_path,source_generation,content_digest,source_path_format,next_reobserve_at,created_at,updated_at) VALUES($1,NULL,$2,'unix:1:1',$3,$4,clock_timestamp()-interval '1 second',clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(path).bind(baseline).bind(format).execute(pool).await.expect("retain source");
    item
}

async fn seed(pool: &PgPool, path: &str, baseline: &str) -> ReleaseWorkItem {
    seed_format(pool, path, baseline, 1).await
}

async fn claim(pool: &PgPool, worker: &WorkerId) -> RetainedSourceLease {
    let leases = WorkItemStore
        .claim_due_retained_sources(pool, worker, 1, Duration::minutes(5))
        .await
        .expect("claim retained");
    assert_eq!(leases.len(), 1);
    leases.into_iter().next().expect("lease")
}

async fn seal_pair(
    pool: &PgPool,
    worker: &WorkerId,
    lease: &RetainedSourceLease,
    generation: &str,
    entry: &CustodyHashObservation,
) -> (
    melodee_persistence::RetainedRecheckSession,
    melodee_persistence::SealedCustodyHash,
) {
    let store = WorkItemStore;
    let now = OffsetDateTime::now_utc();
    let capture = store
        .begin_custody_hash_spool_retained(
            lease.token(),
            pool,
            lease.work_item_id(),
            worker,
            CustodyHashPurpose::Retained,
            generation,
            lease.source_path(),
            now,
        )
        .await
        .expect("begin capture");
    store
        .append_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
            std::slice::from_ref(entry),
            now,
        )
        .await
        .expect("append first");
    store
        .freeze_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
            1,
            now,
        )
        .await
        .expect("freeze first");
    let before = store
        .seal_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
        )
        .await
        .expect("seal first");
    let session = store
        .begin_retained_recheck(pool, lease, &before, "root:1")
        .await
        .expect("begin recheck");
    store
        .revalidate_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
            generation,
            Uuid::now_v7(),
            now,
        )
        .await
        .expect("new epoch");
    store
        .append_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
            std::slice::from_ref(entry),
            now,
        )
        .await
        .expect("append second");
    store
        .freeze_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
            1,
            now,
        )
        .await
        .expect("freeze second");
    let after = store
        .seal_custody_hash_spool_retained(
            lease.token(),
            pool,
            capture,
            lease.work_item_id(),
            worker,
        )
        .await
        .expect("seal second");
    (session, after)
}

#[tokio::test]
async fn unchanged_replays_and_changed_or_inactive_sources_are_held_safely() {
    let db = common::TestDb::create("retained_changes").await;
    let initial = observation('a');
    let unchanged = seed(&db.pool, "unchanged", &sealed_digest(&initial)).await;
    let worker = WorkerId::new();
    let lease = claim(&db.pool, &worker).await;
    assert_eq!(lease.work_item_id(), unchanged.id);
    let (session, after) = seal_pair(&db.pool, &worker, &lease, "unix:1:1", &initial).await;
    let store = WorkItemStore;
    assert!(
        store
            .finish_retained_recheck(
                &db.pool,
                &lease,
                &session,
                &after,
                "wrong-root",
                Duration::minutes(30)
            )
            .await
            .is_err()
    );
    let still_pending: bool = sqlx::query_scalar(
        "SELECT lease_token IS NOT NULL FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(unchanged.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(still_pending);
    let first = store
        .finish_retained_recheck(
            &db.pool,
            &lease,
            &session,
            &after,
            "root:1",
            Duration::minutes(30),
        )
        .await
        .expect("unchanged");
    assert_eq!(first, RetainedRecheckOutcome::Unchanged);
    assert_eq!(
        store
            .finish_retained_recheck(
                &db.pool,
                &lease,
                &session,
                &after,
                "root:1",
                Duration::minutes(30)
            )
            .await
            .expect("replay unchanged"),
        first
    );
    let active: bool =
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id=$1")
            .bind(unchanged.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(active);

    let old = seed(&db.pool, "changed", &sealed_digest(&initial)).await;
    let lease = claim(&db.pool, &worker).await;
    assert_eq!(lease.work_item_id(), old.id);
    let changed = observation('b');
    let (session, after) = seal_pair(&db.pool, &worker, &lease, "unix:1:1", &changed).await;
    assert!(matches!(
        store
            .finish_retained_recheck(
                &db.pool,
                &lease,
                &session,
                &after,
                "root:1",
                Duration::minutes(30)
            )
            .await,
        Err(WorkStoreError::RetainedFreshIntakeRequired)
    ));
    let active: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM release_work_items WHERE source_path='changed' AND inbound_active",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(active, vec![old.id]);
    let audit:(String,Option<Uuid>,bool)=sqlx::query_as("SELECT status,superseded_by,baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1").bind(old.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(audit, ("active".to_owned(), None, true));
    let all: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE source_path='changed'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(all, 1);
    store
        .defer_retained_source(
            &db.pool,
            &lease,
            Duration::minutes(10),
            RetainedRetryReason::AwaitingIntake,
        )
        .await
        .unwrap();
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT last_retry_reason FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(old.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(reason.as_deref(), Some("awaiting_intake"));

    let inactive = seed(&db.pool, "inactive", &sealed_digest(&initial)).await;
    sqlx::query("UPDATE release_work_items SET inbound_active=false WHERE id=$1")
        .bind(inactive.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let lease = claim(&db.pool, &worker).await;
    assert_eq!(lease.work_item_id(), inactive.id);
    let (session, after) = seal_pair(&db.pool, &worker, &lease, "unix:1:1", &initial).await;
    assert!(matches!(
        store
            .finish_retained_recheck(
                &db.pool,
                &lease,
                &session,
                &after,
                "root:1",
                Duration::minutes(30)
            )
            .await,
        Err(WorkStoreError::RetainedFreshIntakeRequired)
    ));
    let state:(String,Option<Uuid>,bool)=sqlx::query_as("SELECT status,superseded_by,baseline_change_pending FROM inbound_retained_sources WHERE work_item_id=$1").bind(inactive.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(state, ("active".to_owned(), None, true));
    let all: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE source_path='inactive'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(all, 1);
    db.drop().await;
}

#[tokio::test]
async fn claims_are_fair_bounded_and_stale_owners_cannot_acknowledge() {
    let db = common::TestDb::create("retained_claims").await;
    let digest = sealed_digest(&observation('a'));
    let oldest = seed(&db.pool, "oldest", &digest).await;
    let later = seed(&db.pool, "later", &digest).await;
    sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()-interval '1 hour' WHERE work_item_id=$1").bind(oldest.id).execute(&db.pool).await.unwrap();
    let first_worker = WorkerId::new();
    let lease = claim(&db.pool, &first_worker).await;
    assert_eq!(lease.work_item_id(), oldest.id);
    assert_eq!(lease.baseline_id(), None);
    let other_worker = WorkerId::new();
    let other = claim(&db.pool, &other_worker).await;
    assert_eq!(other.work_item_id(), later.id);
    assert!(
        WorkItemStore
            .claim_due_retained_sources(&db.pool, &first_worker, 0, Duration::seconds(10))
            .await
            .is_err()
    );
    sqlx::query("UPDATE inbound_retained_sources SET lease_expires_at=clock_timestamp()-interval '1 second',next_reobserve_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(oldest.id).execute(&db.pool).await.unwrap();
    let replacement = claim(&db.pool, &first_worker).await;
    assert_ne!(lease.token(), replacement.token());
    assert!(matches!(
        WorkItemStore
            .renew_retained_source_lease(&db.pool, &lease, Duration::seconds(10))
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    assert!(matches!(
        WorkItemStore
            .defer_retained_source(
                &db.pool,
                &lease,
                Duration::seconds(1),
                RetainedRetryReason::Missing
            )
            .await,
        Err(WorkStoreError::LeaseExpired(_))
    ));
    WorkItemStore
        .defer_retained_source(
            &db.pool,
            &replacement,
            Duration::minutes(10),
            RetainedRetryReason::Ambiguous,
        )
        .await
        .expect("defer");
    let row: (String,Option<String>,Option<String>) = sqlx::query_as("SELECT status,last_retry_reason,lease_owner FROM inbound_retained_sources WHERE work_item_id=$1").bind(oldest.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        row,
        ("active".to_owned(), Some("ambiguous".to_owned()), None)
    );
    db.drop().await;
}

#[tokio::test]
async fn ambiguous_legacy_names_are_rejected_and_quota_is_exact() {
    let db = common::TestDb::create("retained_quota").await;
    sqlx::query("UPDATE inbound_retained_source_budget SET max_active=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    let digest = sealed_digest(&observation('a'));
    let first = seed(&db.pool, "first", &digest).await;
    let ambiguous = ReleaseWorkItem::discovered(
        "ambiguous-\u{fffd}".to_owned(),
        None,
        Uuid::now_v7(),
        OffsetDateTime::now_utc(),
    );
    WorkItemStore.create(&db.pool, &ambiguous).await.unwrap();
    let rejected=sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_path_format,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,0,'generation',$3,clock_timestamp(),clock_timestamp(),clock_timestamp())").bind(ambiguous.id).bind(&ambiguous.source_path).bind(&digest).execute(&db.pool).await;
    assert!(
        matches!(rejected,Err(sqlx::Error::Database(ref error)) if error.code().as_deref()==Some("55006"))
    );
    let third = ReleaseWorkItem::discovered(
        "third".to_owned(),
        None,
        Uuid::now_v7(),
        OffsetDateTime::now_utc(),
    );
    WorkItemStore.create(&db.pool, &third).await.unwrap();
    let result=sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'third','unix:1:1',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())").bind(third.id).bind(&digest).execute(&db.pool).await;
    assert!(
        matches!(result,Err(sqlx::Error::Database(ref error)) if error.code().as_deref()==Some("53400"))
    );
    let leases = WorkItemStore
        .claim_due_retained_sources(&db.pool, &WorkerId::new(), 32, Duration::minutes(5))
        .await
        .unwrap();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].work_item_id(), first.id);
    let count: i64 = sqlx::query_scalar(
        "SELECT active_count FROM inbound_retained_source_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    db.drop().await;
}

#[tokio::test]
async fn blocked_expired_renewal_and_cancelled_defer_do_not_acknowledge() {
    let db = common::TestDb::create("retained_fence").await;
    let digest = sealed_digest(&observation('a'));
    let first = seed(&db.pool, "blocked-renewal", &digest).await;
    let worker = WorkerId::new();
    let lease = WorkItemStore
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::seconds(1))
        .await
        .unwrap()
        .remove(0);
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT work_item_id FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE",
    )
    .bind(first.id)
    .execute(&mut *lock)
    .await
    .unwrap();
    let pool = db.pool.clone();
    let old = lease.clone();
    let renewal = tokio::spawn(async move {
        WorkItemStore
            .renew_retained_source_lease(&pool, &old, Duration::minutes(1))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    lock.commit().await.unwrap();
    assert!(matches!(
        renewal.await.unwrap(),
        Err(WorkStoreError::LeaseExpired(_))
    ));
    let expiry_unchanged: bool = sqlx::query_scalar("SELECT lease_expires_at <= clock_timestamp() FROM inbound_retained_sources WHERE work_item_id=$1").bind(first.id).fetch_one(&db.pool).await.unwrap();
    assert!(expiry_unchanged);

    let next = WorkItemStore
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::minutes(1))
        .await
        .unwrap()
        .remove(0);
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT work_item_id FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE",
    )
    .bind(first.id)
    .execute(&mut *lock)
    .await
    .unwrap();
    let pool = db.pool.clone();
    let cancelled = next.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .defer_retained_source(
                &pool,
                &cancelled,
                Duration::minutes(1),
                RetainedRetryReason::Missing,
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    task.abort();
    let _ = task.await;
    lock.commit().await.unwrap();
    let state: (Option<Uuid>, Option<String>, i32) = sqlx::query_as("SELECT lease_token,last_retry_reason,retry_count FROM inbound_retained_sources WHERE work_item_id=$1").bind(first.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(state, (Some(next.token()), None, 0));
    db.drop().await;
}

#[tokio::test]
async fn chunk_checkpoint_resumes_under_fence_and_rejects_invalid_shapes() {
    let db = common::TestDb::create("retained_chunks").await;
    let digest = sealed_digest(&observation('a'));
    let item = seed(&db.pool, "chunked", &digest).await;
    let worker = WorkerId::new();
    let lease = claim(&db.pool, &worker).await;
    let store = WorkItemStore;
    let first = store
        .begin_or_adopt_retained_chunk_checkpoint(
            &db.pool,
            &lease,
            "disc/file",
            "root:1",
            "file:1",
            65_537,
        )
        .await
        .unwrap();
    assert_eq!(first.next_index(), 0);
    assert!(
        store
            .finish_retained_chunk_checkpoint(&db.pool, &lease, &first)
            .await
            .is_err()
    );
    assert!(
        store
            .advance_retained_chunk_checkpoint(&db.pool, &lease, &first, 0, 10, &"a".repeat(64))
            .await
            .is_err()
    );
    let advanced = store
        .advance_retained_chunk_checkpoint(&db.pool, &lease, &first, 0, 65_536, &"a".repeat(64))
        .await
        .unwrap();
    assert!(
        store
            .advance_retained_chunk_checkpoint(&db.pool, &lease, &first, 0, 65_536, &"a".repeat(64))
            .await
            .is_err()
    );
    assert_eq!(advanced.peaks(), &[(0, "a".repeat(64))]);
    sqlx::query("UPDATE inbound_retained_sources SET lease_expires_at=clock_timestamp()-interval '1 second',next_reobserve_at=clock_timestamp()-interval '1 second' WHERE work_item_id=$1").bind(item.id).execute(&db.pool).await.unwrap();
    let replacement = claim(&db.pool, &WorkerId::new()).await;
    assert!(
        store
            .advance_retained_chunk_checkpoint(&db.pool, &lease, &advanced, 1, 1, &"b".repeat(64))
            .await
            .is_err()
    );
    let adopted = store
        .begin_or_adopt_retained_chunk_checkpoint(
            &db.pool,
            &replacement,
            "disc/file",
            "root:1",
            "file:1",
            65_537,
        )
        .await
        .unwrap();
    assert_eq!(adopted.capture_id(), first.capture_id());
    assert_eq!(adopted.next_index(), 1);
    assert!(
        store
            .begin_or_adopt_retained_chunk_checkpoint(
                &db.pool,
                &replacement,
                "disc/other",
                "root:1",
                "file:1",
                65_537
            )
            .await
            .is_err()
    );
    assert!(
        store
            .advance_retained_chunk_checkpoint(
                &db.pool,
                &replacement,
                &adopted,
                1,
                2,
                &"b".repeat(64)
            )
            .await
            .is_err()
    );
    let second = store
        .advance_retained_chunk_checkpoint(&db.pool, &replacement, &adopted, 1, 1, &"b".repeat(64))
        .await
        .unwrap();
    let mut expected = blake3::Hasher::new();
    expected.update(b"melodee-retained-file-node-v4\0");
    expected.update(&0_u64.to_le_bytes());
    expected.update(&[0xaa; 32]);
    expected.update(&[0xbb; 32]);
    assert_eq!(
        second.peaks(),
        &[(1, expected.finalize().to_hex().to_string())]
    );
    let completed = store
        .finish_retained_chunk_checkpoint(&db.pool, &replacement, &second)
        .await
        .unwrap();
    assert!(completed.is_complete());
    store
        .discard_retained_chunk_checkpoint(&db.pool, &replacement, completed.capture_id())
        .await
        .unwrap();
    let budget: (i64, i64) =
        sqlx::query_as("SELECT captures,bytes FROM inbound_retained_chunk_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(budget, (0, 0));
    db.drop().await;
}

#[tokio::test]
async fn chunk_checkpoint_quota_tampering_and_gc_are_bounded() {
    let db = common::TestDb::create("retained_chunks_gc").await;
    let digest = sealed_digest(&observation('a'));
    let first = seed(&db.pool, "gc-first", &digest).await;
    let _second = seed(&db.pool, "gc-second", &digest).await;
    sqlx::query("UPDATE inbound_retained_chunk_budget SET max_captures=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .unwrap();
    let worker = WorkerId::new();
    let lease = claim(&db.pool, &worker).await;
    assert_eq!(lease.work_item_id(), first.id);
    let store = WorkItemStore;
    let cp = store
        .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &lease, "file", "root", "file", 1)
        .await
        .unwrap();
    let other = claim(&db.pool, &worker).await;
    assert!(matches!(
        store
            .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &other, "file", "root", "file", 1)
            .await,
        Err(WorkStoreError::RetainedSourceBudgetExhausted)
    ));
    sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET peaks='[[0,\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"]]'::jsonb WHERE work_item_id=$1").bind(first.id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &lease, "file", "root", "file", 1)
            .await
            .is_err()
    );
    sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET peaks='[]'::jsonb,updated_at=clock_timestamp()-interval '2 hours' WHERE work_item_id=$1").bind(first.id).execute(&db.pool).await.unwrap();
    assert!(
        !store
            .cleanup_retained_chunk_checkpoint(&db.pool)
            .await
            .unwrap()
    );
    store
        .defer_retained_source(
            &db.pool,
            &lease,
            Duration::minutes(1),
            RetainedRetryReason::Missing,
        )
        .await
        .unwrap();
    assert!(
        store
            .cleanup_retained_chunk_checkpoint(&db.pool)
            .await
            .unwrap()
    );
    let budget: (i64, i64) =
        sqlx::query_as("SELECT captures,bytes FROM inbound_retained_chunk_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(budget, (0, 0));
    assert!(
        store
            .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &other, "file", "root", "file", 1)
            .await
            .is_ok()
    );
    assert!(
        store
            .discard_retained_chunk_checkpoint(&db.pool, &lease, cp.capture_id())
            .await
            .is_err()
    );
    db.drop().await;
}

async fn await_chunk_budget_waiter(pool: &PgPool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT singleton FROM inbound_retained_chunk_budget%')").fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("checkpoint operation should wait for budget lock");
}

#[tokio::test]
async fn blocked_chunk_checkpoint_expiry_and_cancellation_leave_no_progress() {
    let db = common::TestDb::create("retained_chunk_blocked").await;
    let digest = sealed_digest(&observation('a'));
    let item = seed(&db.pool, "blocked-chunk", &digest).await;
    let worker = WorkerId::new();
    let lease = WorkItemStore
        .claim_due_retained_sources(&db.pool, &worker, 1, Duration::seconds(1))
        .await
        .unwrap()
        .remove(0);
    let checkpoint = WorkItemStore
        .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &lease, "file", "root", "file", 1)
        .await
        .unwrap();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT singleton FROM inbound_retained_chunk_budget WHERE singleton FOR UPDATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let old = lease.clone();
    let prev = checkpoint.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .advance_retained_chunk_checkpoint(&pool, &old, &prev, 0, 1, &"a".repeat(64))
            .await
    });
    await_chunk_budget_waiter(&db.pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    lock.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(WorkStoreError::LeaseExpired(_))
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT next_index FROM inbound_retained_chunk_checkpoints WHERE work_item_id=$1",
    )
    .bind(item.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);

    let new = claim(&db.pool, &worker).await;
    let adopted = WorkItemStore
        .begin_or_adopt_retained_chunk_checkpoint(&db.pool, &new, "file", "root", "file", 1)
        .await
        .unwrap();
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("SELECT singleton FROM inbound_retained_chunk_budget WHERE singleton FOR UPDATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let cancel = new.clone();
    let task = tokio::spawn(async move {
        WorkItemStore
            .advance_retained_chunk_checkpoint(&pool, &cancel, &adopted, 0, 1, &"a".repeat(64))
            .await
    });
    await_chunk_budget_waiter(&db.pool).await;
    task.abort();
    let _ = task.await;
    lock.commit().await.unwrap();
    let row:(i64,Uuid)=sqlx::query_as("SELECT next_index,lease_token FROM inbound_retained_chunk_checkpoints WHERE work_item_id=$1").bind(item.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(row, (0, new.token()));
    db.drop().await;
}

#[tokio::test]
async fn operator_listing_is_read_only_bounded_and_preserves_encoded_paths() {
    let db = common::TestDb::create("retained_listing").await;
    let digest = sealed_digest(&observation('a'));
    let first = seed(&db.pool, "//melodee-unix-hex:ff61", &digest).await;
    let second = seed(&db.pool, "ordinary", &digest).await;
    sqlx::query("UPDATE inbound_retained_sources SET baseline_change_pending=true,last_retry_reason='missing',retry_count=3,baseline_unreadable_reason='unreadable',baseline_unreadable_count=2 WHERE work_item_id=$1").bind(first.id).execute(&db.pool).await.unwrap();
    let store = WorkItemStore;
    assert!(
        store
            .list_retained_obligations(&db.pool, None, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .list_retained_obligations(&db.pool, None, 129)
            .await
            .is_err()
    );
    let page1 = store
        .list_retained_obligations(&db.pool, None, 1)
        .await
        .unwrap();
    assert_eq!(page1.len(), 1);
    let page2 = store
        .list_retained_obligations(&db.pool, Some(page1[0].work_item_id), 1)
        .await
        .unwrap();
    assert_eq!(page2.len(), 1);
    assert!(
        store
            .list_retained_obligations(&db.pool, Some(page2[0].work_item_id), 1)
            .await
            .unwrap()
            .is_empty()
    );
    let rows = [&page1[0], &page2[0]];
    let encoded = rows.iter().find(|r| r.work_item_id == first.id).unwrap();
    assert_eq!(encoded.source_path, "//melodee-unix-hex:ff61");
    assert_eq!(encoded.source_path_format, 1);
    assert_eq!(encoded.source_library_id, None);
    assert!(encoded.baseline_change_pending);
    assert_eq!(encoded.last_retry_reason.as_deref(), Some("missing"));
    assert_eq!(
        encoded.baseline_unreadable_reason.as_deref(),
        Some("unreadable")
    );
    assert_eq!(encoded.baseline_unreadable_count, 2);
    assert!(!encoded.lease_active);
    let json = serde_json::to_value(encoded).unwrap();
    assert!(
        json["next_reobserve_at"]
            .as_str()
            .is_some_and(|v| v.contains('T'))
    );
    assert!(json.get("lease_token").is_none());
    assert!(rows.iter().any(|r| r.work_item_id == second.id));
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM inbound_retained_sources WHERE status='active' AND lease_token IS NULL").fetch_one(&db.pool).await.unwrap();
    assert_eq!(active, 2);
    db.drop().await;
}
