//! Performance evidence for work claiming:
//! a nontrivial queue, many workers, zero duplicate ownership, query plan
//! capture. Set `MELODEE_WRITE_PERF_EVIDENCE=1` to refresh
//! `design/evidence/phase-2/durable-work-execution/work-claim-benchmark.json`;
//! normal correctness gates never rewrite tracked evidence.
//!
//! These measurements are evidence, not user-facing performance claims; no
//! thresholds are asserted (PRD-PERF-001).

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, test_config};
use melodee_persistence::{WorkItemStore, WorkerId};
use std::collections::HashSet;
use std::time::Instant;

const QUEUE_SIZE: i64 = 10_000;
const WORKERS: usize = 32;

#[tokio::test]
async fn claim_benchmark_10k_queue_32_workers() {
    let db = TestDb::create("perf_claims").await;
    let store = WorkItemStore;
    let config = test_config();

    // Bulk-seed a ready queue (fixture setup only; the domain transition path
    // is covered by the behavioral tests).
    sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at
        )
        SELECT gen_random_uuid(), gen_random_uuid(),
               '/bench/release-' || g, '/bench/release-' || g, 'ready',
               gen_random_uuid(), now(), now(), now()
        FROM generate_series(1, $1) AS g",
    )
    .bind(QUEUE_SIZE)
    .execute(&db.pool)
    .await
    .expect("seed queue");

    // Query plan for the claim selection (evidence for index justification).
    let plan: Vec<(String,)> = sqlx::query_as(
        "EXPLAIN (FORMAT TEXT)
         SELECT * FROM release_work_items
         WHERE (
                 (state = 'ready' AND (next_attempt_at IS NULL OR next_attempt_at <= now()))
                 OR (state = 'failed' AND next_attempt_at IS NOT NULL
                     AND next_attempt_at <= now() AND attempt_count < 5)
               )
           AND (lease_expires_at IS NULL OR lease_expires_at <= now())
         ORDER BY next_attempt_at NULLS FIRST, created_at, id
         LIMIT 4
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&db.pool)
    .await
    .expect("explain");
    let plan_text: Vec<String> = plan.into_iter().map(|row| row.0).collect();

    let started = Instant::now();
    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            let worker = WorkerId::new();
            let mut mine = Vec::new();
            loop {
                let batch = store
                    .claim(&pool, &worker, &config, T0)
                    .await
                    .expect("claim");
                if batch.is_empty() {
                    break;
                }
                mine.extend(batch.into_iter().map(|item| item.id));
            }
            mine
        }));
    }
    let mut claimed = Vec::new();
    for handle in handles {
        claimed.extend(handle.await.expect("worker"));
    }
    let elapsed = started.elapsed();

    let unique: HashSet<_> = claimed.iter().collect();
    let duplicates = claimed.len() - unique.len();
    assert_eq!(claimed.len() as i64, QUEUE_SIZE);
    assert_eq!(duplicates, 0, "zero duplicate claims");

    let total_claims = claimed.len();
    let evidence = serde_json::json!({
        "schemaVersion": 1,
        "kind": "work-claim-benchmark",
        "capturedAtUtc": time::OffsetDateTime::now_utc().to_string(),
        "queueSize": QUEUE_SIZE,
        "workerCount": WORKERS,
        "claimBatchSize": config.claim_batch_size,
        "totalClaimed": total_claims,
        "duplicateClaims": duplicates,
        "wallClockMs": elapsed.as_millis(),
        "claimsPerSecond": (total_claims as f64 / elapsed.as_secs_f64()).round(),
        "claimQueryPlan": plan_text,
        "note": "Evidence only; no performance threshold asserted (PRD-PERF-001). Environment: dev PostgreSQL on localhost, debug build.",
    });
    if std::env::var("MELODEE_WRITE_PERF_EVIDENCE").as_deref() == Ok("1") {
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../design/evidence/phase-2/durable-work-execution");
        std::fs::create_dir_all(&out_dir).expect("evidence dir");
        std::fs::write(
            out_dir.join("work-claim-benchmark.json"),
            serde_json::to_string_pretty(&evidence).expect("serialize") + "\n",
        )
        .expect("write evidence");
    }
    println!(
        "claimed {total_claims} items with {WORKERS} workers in {:?} ({} claims/s, {duplicates} duplicates)",
        elapsed,
        (total_claims as f64 / elapsed.as_secs_f64()).round(),
    );
    db.drop().await;
}
