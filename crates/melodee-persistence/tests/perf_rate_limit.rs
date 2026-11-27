//! Opt-in contention measurement through the production rate-limit store.
//! Only an invocation-owned `perf_representative.py` database is accepted.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use melodee_persistence::{AuthStore, RateLimitOutcome, RateLimitPolicy};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::{sync::Barrier, task::JoinSet};

fn percentile(samples: &mut [u128], percent: usize) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() * percent).div_ceil(100) - 1]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an invocation-owned fixture and a release build"]
async fn production_rate_limit_contention() {
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture URL");
    let options = PgConnectOptions::from_str(&url)
        .expect("PostgreSQL URL")
        .application_name("melodee-perf-rate-limit");
    let pool = PgPoolOptions::new()
        .max_connections(40)
        .connect_with(options)
        .await
        .expect("fixture connection");
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database name");
    assert!(
        name.strip_prefix("melodee_template_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit())
        )
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").ok().as_deref(),
        Some(name.as_str())
    );
    let sampler_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("sampler connection");

    const PER_WORKER: usize = 20;
    for round in 0..3 {
        for workers in [1_usize, 8, 32] {
            for shared in [true, false] {
                let scope = format!("perf-rate-{round}-{workers}-{shared}");
                let start_barrier = Arc::new(Barrier::new(workers + 1));
                let stop = Arc::new(AtomicBool::new(false));
                let lock_samples = Arc::new(AtomicU64::new(0));
                let max_waiters = Arc::new(AtomicU64::new(0));
                let sample_count = Arc::new(AtomicU64::new(0));
                let sampler = {
                    let pool = sampler_pool.clone();
                    let stop = stop.clone();
                    let lock_samples = lock_samples.clone();
                    let max_waiters = max_waiters.clone();
                    let sample_count = sample_count.clone();
                    tokio::spawn(async move {
                        while !stop.load(Ordering::Relaxed) {
                            let waiters: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND application_name='melodee-perf-rate-limit' AND wait_event_type='Lock'")
                            .fetch_one(&pool).await.expect("lock sample");
                            let waiters = u64::try_from(waiters).unwrap();
                            lock_samples.fetch_add(waiters, Ordering::Relaxed);
                            max_waiters.fetch_max(waiters, Ordering::Relaxed);
                            sample_count.fetch_add(1, Ordering::Relaxed);
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                };
                let mut tasks = JoinSet::new();
                for worker in 0..workers {
                    let pool = pool.clone();
                    let barrier = start_barrier.clone();
                    let scope = scope.clone();
                    tasks.spawn(async move {
                        let mut key = [0_u8; 32];
                        key[..8].copy_from_slice(
                            &(if shared { 0 } else { worker } as u64).to_be_bytes(),
                        );
                        let policy = RateLimitPolicy {
                            scope: &scope,
                            max_attempts: 1_000_000,
                            window: time::Duration::minutes(30),
                            max_retained_keys: 10_000,
                        };
                        let store = AuthStore::new();
                        let mut samples = Vec::with_capacity(PER_WORKER);
                        barrier.wait().await;
                        for _ in 0..PER_WORKER {
                            let started = Instant::now();
                            let result = store
                                .record_rate_limit_attempt(
                                    &pool,
                                    &policy,
                                    &key,
                                    time::OffsetDateTime::now_utc(),
                                )
                                .await
                                .expect("production rate limit");
                            samples.push(started.elapsed().as_micros());
                            assert!(matches!(result, RateLimitOutcome::Allowed { .. }));
                        }
                        samples
                    });
                }
                let started = Instant::now();
                start_barrier.wait().await;
                let mut samples = Vec::with_capacity(workers * PER_WORKER);
                while let Some(task) = tasks.join_next().await {
                    samples.extend(task.expect("worker"));
                }
                let elapsed = started.elapsed();
                stop.store(true, Ordering::Relaxed);
                sampler.await.expect("sampler");
                let (keys, attempts): (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(attempt_count),0)::bigint FROM native_rate_limit_counters WHERE scope=$1")
                .bind(&scope).fetch_one(&pool).await.expect("verify counters");
                assert_eq!(keys, if shared { 1 } else { workers as i64 });
                assert_eq!(attempts, (workers * PER_WORKER) as i64);
                println!(
                    "round={round} workers={workers} shared_actor={shared} samples={} elapsed_seconds={:.4} attempts_per_second={:.1} p50_us={} p95_us={} p99_us={} lock_samples={} lock_waiter_sum={} max_lock_waiters={} errors=0 counters_exact=true",
                    samples.len(),
                    elapsed.as_secs_f64(),
                    samples.len() as f64 / elapsed.as_secs_f64(),
                    percentile(&mut samples.clone(), 50),
                    percentile(&mut samples.clone(), 95),
                    percentile(&mut samples, 99),
                    sample_count.load(Ordering::Relaxed),
                    lock_samples.load(Ordering::Relaxed),
                    max_waiters.load(Ordering::Relaxed)
                );
            }
        }
    }
    println!(
        "path=AuthStore::record_rate_limit_attempt http_and_password_hashing=excluded postgres_lock_samples=approximate sampling_interval_ms=5"
    );
}
