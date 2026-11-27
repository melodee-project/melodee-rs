//! The protected claim must fence a statement that outlives its lease, even
//! when a task is cancelled while PostgreSQL is still executing the insert.
#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{
    CustodyHashObservation, CustodyHashPurpose, WorkEngineConfig, WorkItemStore, WorkStoreError,
    WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn run_blocked(cancel: bool) {
    let db = common::TestDb::create(if cancel {
        "protected_cancel"
    } else {
        "protected_expiry"
    })
    .await;
    let store = WorkItemStore;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let worker = WorkerId::new();
    let config = WorkEngineConfig {
        lease_duration: Duration::seconds(3),
        ..WorkEngineConfig::default()
    };
    let (claimed, claim) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    let expiry = claimed.lease_expires_at.unwrap();
    let capture = store
        .begin_protected_custody_hash_spool(
            &db.pool,
            &claim,
            &worker,
            CustodyHashPurpose::Source,
            "source-generation",
            "source",
            now,
        )
        .await
        .unwrap();
    let initial: (i64, i64) =
        sqlx::query_as("SELECT entries,bytes FROM custody_hash_spool_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let key = (item.id.as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_protected_spool_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_protected_spool_stall BEFORE INSERT ON custody_hash_spool_entries FOR EACH ROW EXECUTE FUNCTION test_protected_spool_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let worker_task = worker.clone();
    let task = tokio::spawn(async move {
        let entry = CustodyHashObservation {
            path_key: "file".to_owned(),
            parent_key: String::new(),
            name_key: "66696c65".to_owned(),
            legacy_path_hex: "6100".to_owned(),
            is_directory: false,
            legacy_length: 1,
            digest: "a".repeat(64),
        };
        WorkItemStore
            .append_protected_custody_hash_spool(
                &pool,
                &claim,
                capture,
                &worker_task,
                &[entry],
                now,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'INSERT INTO custody_hash_spool_entries%')").fetch_one(&db.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("insert reached the advisory lock");
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
        .expect("database clock reached lease expiry");
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(
            matches!(task.await.unwrap(), Err(WorkStoreError::LeaseExpired(id)) if id==item.id)
        );
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    // Acquiring the budget row waits for any cancelled transaction's rollback.
    let mut verify = db.pool.begin().await.unwrap();
    let after: (i64, i64) = sqlx::query_as(
        "SELECT entries,bytes FROM custody_hash_spool_budget WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut *verify)
    .await
    .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM custody_hash_spool_entries WHERE capture_id=$1")
            .bind(capture)
            .fetch_one(&mut *verify)
            .await
            .unwrap();
    let captured: i64 =
        sqlx::query_scalar("SELECT entry_count FROM custody_hash_spool_captures WHERE id=$1")
            .bind(capture)
            .fetch_one(&mut *verify)
            .await
            .unwrap();
    assert_eq!(after, initial, "entry and byte quota must roll back");
    assert_eq!(
        (count, captured),
        (0, 0),
        "cancelled/expired work must not commit evidence"
    );
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}

#[tokio::test]
async fn protected_append_expiry_rolls_back() {
    run_blocked(false).await;
}
#[tokio::test]
async fn protected_append_cancellation_rolls_back() {
    run_blocked(true).await;
}
