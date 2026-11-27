//! Provisional hash spool bounds, retry safety, lease fencing and cleanup.
#![allow(clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    CustodyHashCursor, CustodyHashObservation, CustodyHashPurpose, WorkEngineConfig, WorkItemStore,
    WorkerId,
};
use time::Duration;
use uuid::Uuid;

fn entry(ordinal: i64) -> CustodyHashObservation {
    CustodyHashObservation {
        path_key: format!("track-{ordinal}"),
        parent_key: String::new(),
        name_key: format!("{ordinal:016x}"),
        legacy_path_hex: "6100".to_owned(),
        is_directory: false,
        legacy_length: 6,
        digest: format!("{:064x}", ordinal % 7),
    }
}

#[tokio::test]
async fn source_and_staging_have_distinct_provisional_capture_identities() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("custody_hash_purposes").await;
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("claim");
    let source = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("source");
    let staging = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Staging,
            "unix:2:3",
            "stage",
            now,
        )
        .await
        .expect("staging");
    assert_ne!(source, staging);
    assert_eq!(
        store
            .custody_hash_spool_for_item(&db.pool, item.id, CustodyHashPurpose::Source)
            .await
            .expect("source lookup")
            .expect("source row")
            .id,
        source
    );
    assert_eq!(
        store
            .custody_hash_spool_for_item(&db.pool, item.id, CustodyHashPurpose::Staging)
            .await
            .expect("staging lookup")
            .expect("staging row")
            .id,
        staging
    );
    assert!(
        store
            .begin_custody_hash_spool(
                &db.pool,
                item.id,
                &worker,
                CustodyHashPurpose::Source,
                "unix:1:2",
                "source",
                now
            )
            .await
            .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn spool_pages_are_bounded_idempotent_fenced_and_recoverable() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("custody_hash_spool").await;
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("claim");
    sqlx::query("UPDATE custody_hash_spool_budget SET max_entries=129 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("quota");
    let capture = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("capture");
    let first: Vec<_> = (0..128).map(entry).collect();
    let mut reversed = first.clone();
    reversed.reverse();
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &reversed, now)
        .await
        .expect("append");
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &first, now)
        .await
        .expect("retry");
    let expected_bytes: i64 = 1542
        + first
            .iter()
            .map(|row| {
                (row.path_key.len()
                    + row.parent_key.len()
                    + row.name_key.len()
                    + row.legacy_path_hex.len()) as i64
                    + 384
            })
            .sum::<i64>();
    let accounted: (i64, i64) =
        sqlx::query_as("SELECT entries,bytes FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("page accounting");
    assert_eq!(accounted, (128, expected_bytes));
    assert!(sqlx::query("UPDATE custody_hash_spool_entries SET path_key='renamed' WHERE capture_id=$1 AND path_key='track-0'")
        .bind(capture).execute(&db.pool).await.is_err(), "path evidence must be immutable");
    assert!(sqlx::query("UPDATE custody_hash_spool_entries SET digest=repeat('f',64) WHERE capture_id=$1 AND path_key='track-0'")
        .bind(capture).execute(&db.pool).await.is_err(), "digest evidence must be immutable");
    let after_rejected_updates: (i64, i64) =
        sqlx::query_as("SELECT entries,bytes FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("accounting after rejected updates");
    assert_eq!(after_rejected_updates, accounted);
    let altered = [
        CustodyHashObservation {
            digest: "f".repeat(64),
            ..entry(0)
        },
        entry(128),
    ];
    assert!(
        store
            .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &altered, now)
            .await
            .is_err()
    );
    let counts: (i64, i64) =
        sqlx::query_as("SELECT captures,entries FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("counts");
    assert_eq!(
        counts,
        (1, 128),
        "a conflicting path rolls back the entire page"
    );
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &[entry(128)], now)
        .await
        .expect("last");
    sqlx::query(
        "UPDATE custody_hash_spool_budget SET max_entries=130,max_bytes=bytes+384 WHERE singleton",
    )
    .execute(&db.pool)
    .await
    .expect("byte quota");
    assert!(matches!(
        store
            .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &[entry(129)], now)
            .await,
        Err(melodee_persistence::WorkStoreError::CustodyHashBudgetExhausted)
    ));
    assert!(
        store
            .freeze_custody_hash_spool(&db.pool, capture, item.id, &worker, 128, now)
            .await
            .is_err(),
        "a partial traversal cannot freeze the capture"
    );
    store
        .freeze_custody_hash_spool(&db.pool, capture, item.id, &worker, 129, now)
        .await
        .expect("freeze");
    let seal = store
        .seal_custody_hash_spool(&db.pool, capture, item.id, &worker)
        .await
        .expect("seal frozen spool");
    assert!(seal.digest().starts_with("v3:"));
    assert_eq!(seal.entries(), 129);
    let siblings = store
        .custody_legacy_siblings_page(
            &db.pool, capture, item.id, &worker, "", None, false, false, now,
        )
        .await
        .expect("siblings");
    assert_eq!(siblings.len(), 16);
    assert!(
        siblings
            .windows(2)
            .all(|pair| pair[0].name_key < pair[1].name_key)
    );
    let last_sibling = siblings.last().expect("last sibling");
    let sibling_cursor = melodee_persistence::CustodyLegacyCursor {
        name_key: last_sibling.name_key.clone(),
        ordinal: last_sibling.ordinal,
    };
    let next_siblings = store
        .custody_legacy_siblings_page(
            &db.pool,
            capture,
            item.id,
            &worker,
            "",
            Some(&sibling_cursor),
            false,
            false,
            now,
        )
        .await
        .expect("next siblings");
    assert_eq!(next_siblings.len(), 16);
    assert!(next_siblings[0].name_key > last_sibling.name_key);
    assert!(
        store
            .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &[entry(129)], now)
            .await
            .is_err()
    );
    let page = store
        .custody_hash_spool_page(&db.pool, capture, item.id, &worker, None, now)
        .await
        .expect("first page");
    assert_eq!(page.len(), 128);
    let last = page.last().expect("last");
    let cursor = CustodyHashCursor {
        digest: last.digest.clone(),
        ordinal: last.ordinal,
    };
    let tail = store
        .custody_hash_spool_page(&db.pool, capture, item.id, &worker, Some(&cursor), now)
        .await
        .expect("tail");
    assert_eq!(tail.len(), 1);
    assert!(
        page.windows(2)
            .all(|pair| (&pair[0].digest, pair[0].ordinal) <= (&pair[1].digest, pair[1].ordinal))
    );
    assert!(
        !store
            .cleanup_abandoned_custody_hash_spool_page(&db.pool, now + Duration::days(1))
            .await
            .expect("active lease uses database clock")
    );
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now + Duration::milliseconds(50))
        .execute(&db.pool)
        .await
        .expect("expire lease");
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert!(
        matches!(store.custody_hash_spool_page(&db.pool, capture, item.id, &worker, None, now).await,
        Err(melodee_persistence::WorkStoreError::LeaseExpired(id)) if id == item.id)
    );
    assert!(
        matches!(store.renew_custody_hash_lease(&db.pool, item.id, &worker, Duration::seconds(30)).await,
        Err(melodee_persistence::WorkStoreError::LeaseExpired(id)) if id == item.id)
    );
    assert!(
        store
            .cleanup_abandoned_custody_hash_spool_page(&db.pool, now + Duration::days(1))
            .await
            .expect("cleanup page")
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT entries FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("remaining");
    assert_eq!(remaining, 1);
    assert!(
        store
            .cleanup_abandoned_custody_hash_spool_page(&db.pool, now + Duration::days(1))
            .await
            .expect("cleanup tail")
    );
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT captures,entries,bytes FROM custody_hash_spool_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .expect("final counts");
    assert_eq!(counts, (0, 0, 0));
    db.drop().await;
}

#[tokio::test]
async fn exact_path_equality_survives_a_forced_lookup_hash_collision() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("custody_hash_path_collision").await;
    sqlx::query("CREATE OR REPLACE FUNCTION ingest_occurrence_hash(path text) RETURNS bytea LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE RETURN decode(repeat('00',32),'hex')")
        .execute(&db.pool).await.expect("force collision");
    sqlx::query("REINDEX INDEX custody_hash_spool_path_idx")
        .execute(&db.pool)
        .await
        .expect("reindex");
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("claim");
    let capture = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("capture");
    let inputs = [
        entry(1),
        entry(2),
        CustodyHashObservation {
            path_key: "x".repeat(8192),
            digest: "f".repeat(64),
            ..entry(3)
        },
    ];
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &inputs, now)
        .await
        .expect("collision-safe append");
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &inputs, now)
        .await
        .expect("idempotent retry");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM custody_hash_spool_entries WHERE capture_id=$1")
            .bind(capture)
            .fetch_one(&db.pool)
            .await
            .expect("entries");
    assert_eq!(count, 3);
    db.drop().await;
}

#[tokio::test]
async fn replacement_lease_must_reobserve_every_existing_path_before_freeze() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("custody_hash_resume").await;
    let store = WorkItemStore;
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let original = WorkerId::new();
    store
        .claim(&db.pool, &original, &WorkEngineConfig::default(), now)
        .await
        .expect("first lease");
    let capture = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &original,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("capture");
    let inputs = [entry(1), entry(2), entry(3)];
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &original, &inputs, now)
        .await
        .expect("initial page");
    let next = WorkerId::new();
    sqlx::query("UPDATE release_work_items SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(item.id).execute(&db.pool).await.expect("expire lease");
    let resumed = time::OffsetDateTime::now_utc();
    assert_eq!(
        store
            .claim(&db.pool, &next, &WorkEngineConfig::default(), resumed)
            .await
            .expect("new lease")
            .len(),
        1
    );
    let epoch = Uuid::now_v7();
    store
        .revalidate_custody_hash_spool(
            &db.pool, capture, item.id, &next, "unix:1:2", epoch, resumed,
        )
        .await
        .expect("adopt for revalidation");
    assert!(
        store
            .freeze_custody_hash_spool(&db.pool, capture, item.id, &next, 3, resumed)
            .await
            .is_err()
    );
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &next, &inputs[..2], resumed)
        .await
        .expect("partial");
    store
        .revalidate_custody_hash_spool(
            &db.pool, capture, item.id, &next, "unix:1:2", epoch, resumed,
        )
        .await
        .expect("idempotent epoch retry");
    assert!(
        store
            .freeze_custody_hash_spool(&db.pool, capture, item.id, &next, 3, resumed)
            .await
            .is_err()
    );
    assert!(
        store
            .append_custody_hash_spool(&db.pool, capture, item.id, &original, &inputs, resumed)
            .await
            .is_err()
    );
    store
        .append_custody_hash_spool(&db.pool, capture, item.id, &next, &inputs[2..], resumed)
        .await
        .expect("remaining path");
    store
        .freeze_custody_hash_spool(&db.pool, capture, item.id, &next, 3, resumed)
        .await
        .expect("complete reobservation");
    db.drop().await;
}

#[tokio::test]
async fn concurrent_captures_cannot_exceed_global_entry_quota() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("custody_hash_quota").await;
    let store = WorkItemStore;
    let mut first = ReleaseWorkItem::discovered("first".to_owned(), None, Uuid::now_v7(), now);
    let mut second = ReleaseWorkItem::discovered("second".to_owned(), None, Uuid::now_v7(), now);
    first.state = IngestionState::Ready;
    second.state = IngestionState::Ready;
    store.create(&db.pool, &first).await.expect("first");
    store.create(&db.pool, &second).await.expect("second");
    let worker = WorkerId::new();
    store
        .claim(&db.pool, &worker, &WorkEngineConfig::default(), now)
        .await
        .expect("claim");
    sqlx::query("UPDATE custody_hash_spool_budget SET max_entries=1 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("quota");
    let a = store
        .begin_custody_hash_spool(
            &db.pool,
            first.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:1",
            "source",
            now,
        )
        .await
        .expect("a");
    let b = store
        .begin_custody_hash_spool(
            &db.pool,
            second.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("b");
    let row = [entry(0)];
    let (left, right) = tokio::join!(
        store.append_custody_hash_spool(&db.pool, a, first.id, &worker, &row, now),
        store.append_custody_hash_spool(&db.pool, b, second.id, &worker, &row, now),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let entries: i64 =
        sqlx::query_scalar("SELECT entries FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(entries, 1);
    db.drop().await;
}

#[tokio::test]
async fn page_rolls_back_if_lease_expires_while_insert_is_stalled() {
    let db = common::TestDb::create("custody_hash_late_commit").await;
    let store = WorkItemStore;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let config = WorkEngineConfig {
        lease_duration: Duration::seconds(3),
        ..WorkEngineConfig::default()
    };
    let claimed = store
        .claim(&db.pool, &worker, &config, now)
        .await
        .expect("claim");
    let expiry = claimed[0].lease_expires_at.expect("expiry");
    let capture = store
        .begin_custody_hash_spool(
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Source,
            "unix:1:2",
            "source",
            now,
        )
        .await
        .expect("capture");
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .expect("lock");
    let trigger = format!(
        "CREATE FUNCTION test_custody_spool_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_custody_spool_stall BEFORE INSERT ON custody_hash_spool_entries FOR EACH ROW EXECUTE FUNCTION test_custody_spool_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .expect("test trigger");
    let worker_task = worker.clone();
    let pool = db.pool.clone();
    let item_id = item.id;
    let task = tokio::spawn(async move {
        WorkItemStore
            .append_custody_hash_spool(&pool, capture, item_id, &worker_task, &[entry(1)], now)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'INSERT INTO custody_hash_spool_entries%')")
                .fetch_one(&db.pool).await.expect("wait status");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("insert reached deterministic stall");
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>$1")
                .bind(expiry)
                .fetch_one(&db.pool)
                .await
                .expect("clock");
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("lease expired while stalled");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .expect("unlock");
    assert!(
        matches!(task.await.expect("join"), Err(melodee_persistence::WorkStoreError::LeaseExpired(id)) if id == item.id)
    );
    let count: i64 =
        sqlx::query_scalar("SELECT entries FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(count, 0, "late page and quota accounting must roll back");
    drop(blocker);
    db.drop().await;
}

#[tokio::test]
async fn retained_spool_uses_independent_token_fence_and_allows_safe_takeover_cleanup() {
    let now = time::OffsetDateTime::now_utc();
    let db = common::TestDb::create("retained_spool_fence").await;
    let store = WorkItemStore;
    let item = ReleaseWorkItem::discovered("retained-path".to_owned(), None, Uuid::now_v7(), now);
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let first_token = Uuid::now_v7();
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at,lease_owner,lease_token,lease_expires_at) VALUES ($1,'retained-path','gen',$2,$3,$3,$3,$4,$5,clock_timestamp()+interval '5 minutes')")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).bind(now).bind(worker.as_str()).bind(first_token)
        .execute(&db.pool).await.expect("retained lease");
    assert!(
        store
            .begin_custody_hash_spool_retained(
                first_token,
                &db.pool,
                item.id,
                &worker,
                CustodyHashPurpose::Retained,
                "gen",
                "wrong-path",
                now
            )
            .await
            .is_err()
    );
    let capture = store
        .begin_custody_hash_spool_retained(
            first_token,
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Retained,
            "gen",
            "retained-path",
            now,
        )
        .await
        .expect("independent capture");
    store
        .append_custody_hash_spool_retained(
            first_token,
            &db.pool,
            capture,
            item.id,
            &worker,
            &[entry(1)],
            now,
        )
        .await
        .expect("append");
    store
        .freeze_custody_hash_spool_retained(
            first_token,
            &db.pool,
            capture,
            item.id,
            &worker,
            1,
            now,
        )
        .await
        .expect("freeze");
    let seal = store
        .seal_custody_hash_spool_retained(first_token, &db.pool, capture, item.id, &worker)
        .await
        .expect("seal");
    assert_eq!(seal.retained_lease_token(), Some(first_token));
    assert!(
        !store
            .cleanup_abandoned_custody_hash_spool_page(&db.pool, now)
            .await
            .expect("active cleanup")
    );
    assert!(
        store
            .append_custody_hash_spool(&db.pool, capture, item.id, &worker, &[entry(2)], now)
            .await
            .is_err()
    );
    let next_token = Uuid::now_v7();
    sqlx::query("UPDATE inbound_retained_sources SET lease_token=$2 WHERE work_item_id=$1")
        .bind(item.id)
        .bind(next_token)
        .execute(&db.pool)
        .await
        .expect("takeover");
    assert!(
        store
            .revalidate_custody_hash_spool_retained(
                first_token,
                &db.pool,
                capture,
                item.id,
                &worker,
                "gen",
                Uuid::now_v7(),
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .revalidate_custody_hash_spool_retained(
                next_token,
                &db.pool,
                capture,
                item.id,
                &worker,
                "gen",
                Uuid::now_v7(),
                now
            )
            .await
            .is_err(),
        "new token cannot adopt old evidence"
    );
    assert!(
        store
            .discard_custody_hash_spool_page_retained(
                next_token, &db.pool, capture, item.id, &worker, now
            )
            .await
            .expect("takeover discard")
    );
    let replacement = store
        .begin_custody_hash_spool_retained(
            next_token,
            &db.pool,
            item.id,
            &worker,
            CustodyHashPurpose::Retained,
            "gen",
            "retained-path",
            now,
        )
        .await
        .expect("fresh capture");
    assert_ne!(capture, replacement);
    assert!(
        !store
            .cleanup_abandoned_custody_hash_spool_page(&db.pool, now)
            .await
            .expect("new lease cleanup")
    );
    db.drop().await;
}

#[tokio::test]
async fn stalled_renewal_cannot_revive_an_expired_processing_lease() {
    let db = common::TestDb::create("custody_stalled_renewal").await;
    let store = WorkItemStore;
    let now = time::OffsetDateTime::now_utc();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.expect("item");
    let worker = WorkerId::new();
    let config = WorkEngineConfig {
        lease_duration: Duration::seconds(3),
        ..WorkEngineConfig::default()
    };
    let claimed = store
        .claim(&db.pool, &worker, &config, now)
        .await
        .expect("claim");
    let expiry = claimed[0].lease_expires_at.expect("expiry");
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .expect("lock");
    let trigger = format!(
        "CREATE FUNCTION test_custody_renewal_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.lease_expires_at > OLD.lease_expires_at THEN PERFORM pg_advisory_xact_lock({key}); END IF; RETURN NEW; END $$; CREATE TRIGGER test_custody_renewal_stall BEFORE UPDATE ON release_work_items FOR EACH ROW EXECUTE FUNCTION test_custody_renewal_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .expect("trigger");
    let worker_task = worker.clone();
    let pool = db.pool.clone();
    let item_id = item.id;
    let task = tokio::spawn(async move {
        WorkItemStore
            .renew_custody_hash_lease(&pool, item_id, &worker_task, Duration::minutes(5))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'UPDATE release_work_items SET lease_expires_at=%')")
                .fetch_one(&db.pool).await.expect("wait status");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("renewal reached stall");
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>$1")
                .bind(expiry)
                .fetch_one(&db.pool)
                .await
                .expect("clock");
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("old lease expired");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .expect("unlock");
    assert!(
        matches!(task.await.expect("join"), Err(melodee_persistence::WorkStoreError::LeaseExpired(id)) if id == item.id)
    );
    let persisted: time::OffsetDateTime =
        sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&db.pool)
            .await
            .expect("expiry");
    assert_eq!(persisted, expiry, "stalled renewal must roll back");
    drop(blocker);
    db.drop().await;
}
