//! Migration integration tests: from-empty and repeatability (work-package
//! parts XIV, XXXII).

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_persistence::migrate;

#[tokio::test]
async fn migrations_run_from_empty_database() {
    let db = TestDb::create("migrations_empty").await;
    // Tables exist and are queryable.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_items")
        .fetch_one(&db.pool)
        .await
        .expect("query work items");
    assert_eq!(count.0, 0);
    let events: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_item_events")
        .fetch_one(&db.pool)
        .await
        .expect("query events");
    assert_eq!(events.0, 0);
    db.drop().await;
}

#[tokio::test]
async fn migrations_are_repeatable() {
    let db = TestDb::create("migrations_repeat").await;
    migrate(&db.pool).await.expect("second migrate run");
    migrate(&db.pool).await.expect("third migrate run");
    db.drop().await;
}

#[tokio::test]
async fn schema_rejects_impossible_states() {
    let db = TestDb::create("migrations_constraints").await;

    // Unknown state is rejected by the CHECK constraint.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'bogus',
                  gen_random_uuid(), now(), now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "unknown state must be rejected");

    // Lease inconsistency is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at, lease_owner
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'ready',
                  gen_random_uuid(), now(), now(), now(), 'worker-without-timestamps')",
    )
    .execute(&db.pool)
    .await;
    assert!(
        result.is_err(),
        "lease owner without timestamps must be rejected"
    );

    // Negative attempt count is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at, attempt_count
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'ready',
                  gen_random_uuid(), now(), now(), now(), -1)",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "negative attempt_count must be rejected");

    // Terminal state without completed_at is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'indexed',
                  gen_random_uuid(), now(), now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(
        result.is_err(),
        "terminal state without completed_at must be rejected"
    );

    db.drop().await;
}
