//! Phase 9 WP-05 worker-pool durable pause/resume acceptance (ADR-0134 §8,
//! PRD-WORK-005): migration shape, constraint integrity, audited
//! pause/resume transitions, and the worker-facing pause check.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY, WorkerPoolStore, WorkerPoolStoreError,
    is_pool_paused, is_valid_pool_name,
};
use time::OffsetDateTime;

#[tokio::test]
async fn migration_seeds_exactly_the_wired_pools_unpaused() {
    let db = TestDb::create("worker_pool_seed").await;
    let pools = WorkerPoolStore.list(&db.pool).await.expect("list pools");
    let names: Vec<&str> = pools.iter().map(|pool| pool.pool.as_str()).collect();
    assert_eq!(names, [POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY]);
    for pool in &pools {
        assert!(!pool.paused, "{} starts unpaused", pool.pool);
        assert_eq!(pool.paused_by, None);
        assert_eq!(pool.paused_at, None);
        assert_eq!(pool.resume_requested_at, None);
    }
    db.drop().await;
}

#[tokio::test]
async fn schema_rejects_invalid_pool_rows() {
    let db = TestDb::create("worker_pool_constraints").await;

    // Pool names are lowercase machine identifiers.
    for bad in ["Podcast", "1pool", "pod cast", "", "pool!"] {
        let result = sqlx::query("INSERT INTO worker_pool_state (pool) VALUES ($1)")
            .bind(bad)
            .execute(&db.pool)
            .await;
        assert!(result.is_err(), "pool name '{bad}' must be rejected");
    }

    // A paused row must carry its operator audit.
    let result =
        sqlx::query("INSERT INTO worker_pool_state (pool, paused) VALUES ('search', true)")
            .execute(&db.pool)
            .await;
    assert!(result.is_err(), "paused without audit must be rejected");

    // Audit fields are set (or cleared) together.
    let result =
        sqlx::query("INSERT INTO worker_pool_state (pool, paused_by) VALUES ('search', 'ops')")
            .execute(&db.pool)
            .await;
    assert!(
        result.is_err(),
        "paused_by without paused_at must be rejected"
    );

    db.drop().await;
}

#[tokio::test]
async fn pause_resume_transitions_preserve_durable_audit_history() {
    let db = TestDb::create("worker_pool_transitions").await;
    let store = WorkerPoolStore;
    let t1 = OffsetDateTime::now_utc();

    let paused = store
        .set_paused(&db.pool, POOL_POLICY, true, "ops-alice", t1)
        .await
        .expect("pause");
    assert!(paused.paused);
    assert_eq!(paused.paused_by.as_deref(), Some("ops-alice"));
    assert!(paused.paused_at.is_some());
    assert_eq!(paused.resume_requested_at, None);
    assert!(is_pool_paused(&db.pool, POOL_POLICY).await.expect("paused"));

    // Resume keeps the last-pause audit and stamps the resume request.
    let t2 = t1 + time::Duration::seconds(5);
    let resumed = store
        .set_paused(&db.pool, POOL_POLICY, false, "ops-bob", t2)
        .await
        .expect("resume");
    assert!(!resumed.paused);
    assert_eq!(
        resumed.paused_by.as_deref(),
        Some("ops-alice"),
        "last-pause audit survives resume"
    );
    assert_eq!(resumed.paused_at, paused.paused_at);
    assert!(resumed.resume_requested_at.is_some());
    assert!(resumed.updated_at >= paused.updated_at);
    assert!(
        !is_pool_paused(&db.pool, POOL_POLICY)
            .await
            .expect("resumed")
    );

    // Re-pausing refreshes the audit idempotently.
    let t3 = t2 + time::Duration::seconds(5);
    let repaused = store
        .set_paused(&db.pool, POOL_POLICY, true, "ops-bob", t3)
        .await
        .expect("re-pause");
    assert!(repaused.paused);
    assert_eq!(repaused.paused_by.as_deref(), Some("ops-bob"));
    assert_eq!(repaused.resume_requested_at, resumed.resume_requested_at);

    db.drop().await;
}

#[tokio::test]
async fn unknown_pool_is_never_paused_and_registers_lazily_on_pause() {
    let db = TestDb::create("worker_pool_lazy").await;
    let store = WorkerPoolStore;

    assert!(
        !is_pool_paused(&db.pool, "search")
            .await
            .expect("missing row")
    );
    assert!(store.get(&db.pool, "search").await.expect("get").is_none());

    // Pausing an unregistered pool upserts its row so a pool wired in a
    // later binary is paused from its first check.
    let now = OffsetDateTime::now_utc();
    let state = store
        .set_paused(&db.pool, "search", true, "ops-carol", now)
        .await
        .expect("lazy pause registration");
    assert!(state.paused);
    assert!(is_pool_paused(&db.pool, "search").await.expect("paused"));

    // Validation happens before any SQL: bad names and blank actors fail.
    assert!(matches!(
        store
            .set_paused(&db.pool, "Bad Pool", true, "ops", now)
            .await,
        Err(WorkerPoolStoreError::InvalidPoolName)
    ));
    assert!(matches!(
        store.set_paused(&db.pool, "search", true, "   ", now).await,
        Err(WorkerPoolStoreError::MissingActor)
    ));
    assert!(matches!(
        store.get(&db.pool, "nope!").await,
        Err(WorkerPoolStoreError::InvalidPoolName)
    ));

    db.drop().await;
}

#[test]
fn pool_name_validation_matches_the_migration_check() {
    for valid in [
        "podcast",
        "policy",
        "discovery",
        "search",
        "a",
        "pool-1",
        "pool_2",
    ] {
        assert!(is_valid_pool_name(valid), "{valid}");
    }
    for invalid in [
        "",
        "Podcast",
        "1pool",
        " pool",
        "pool ",
        "pool.name",
        "pööl",
    ] {
        assert!(!is_valid_pool_name(invalid), "{invalid}");
    }
    let longest = format!("p{}", "o".repeat(63));
    assert!(is_valid_pool_name(&longest));
    assert!(!is_valid_pool_name(&format!("{longest}x")));
}
