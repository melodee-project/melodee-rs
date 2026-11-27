//! Opt-in production-store contention probe, for the owned representative fixture.
//!
//! Run through `scripts/perf_representative.py` with a release build. It writes
//! scrobbles and billing changes only to the invocation-owned fixture database.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use melodee_persistence::{NativeScrobbleResult, NativeUserStateStore};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio::{sync::Barrier, task::JoinSet};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};
use uuid::Uuid;

const WORKERS: usize = 8;
const PER_WORKER: usize = 20;

const MAX_STAGE_SAMPLES: usize = 100_000;

struct QueryCounter {
    queries: Arc<AtomicU64>,
    stages: Arc<Mutex<Vec<(u8, u64)>>>,
    overflow: Arc<AtomicBool>,
}

#[derive(Default)]
struct StageVisitor {
    stage: Option<u8>,
    elapsed_us: Option<u64>,
}

impl Visit for StageVisitor {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "stage" {
            self.stage = match value {
                "binding" => Some(0),
                "track_wait" => Some(1),
                "artist_wait" => Some(2),
                "release_wait" => Some(3),
                "table_wait" => Some(4),
                "play_insert" => Some(5),
                "counters" => Some(6),
                "commit" => Some(7),
                _ => None,
            };
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "elapsed_us" {
            self.elapsed_us = Some(value);
        }
    }
}

impl<S: Subscriber> Layer<S> for QueryCounter {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.queries.fetch_add(1, Ordering::Relaxed);
        }
        if event.metadata().target() == "melodee::scrobble" {
            let mut visitor = StageVisitor::default();
            event.record(&mut visitor);
            if let (Some(stage), Some(elapsed)) = (visitor.stage, visitor.elapsed_us) {
                let mut rows = self.stages.lock().expect("stage samples mutex");
                if rows.len() < MAX_STAGE_SAMPLES {
                    rows.push((stage, elapsed));
                } else {
                    self.overflow.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

fn peak_process_rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
}

#[derive(Clone, Copy)]
struct Track {
    id: Uuid,
    public_id: Uuid,
    release_id: Uuid,
    benchmark_credit_id: Uuid,
}

async fn assert_popularity(pool: &PgPool, tracks: &[Track], artists: &[Uuid]) {
    let ids: Vec<Uuid> = tracks.iter().map(|t| t.id).collect();
    let releases: Vec<Uuid> = tracks.iter().map(|t| t.release_id).collect();
    let inconsistent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_recommendation_candidates c
         WHERE (c.entity_type='track' AND c.entity_id=ANY($1) AND c.popularity <> (
                    SELECT count(*) FROM native_track_plays p WHERE p.track_id=c.entity_id))
            OR (c.entity_type='release' AND c.entity_id=ANY($2) AND c.popularity <> (
                    SELECT count(*) FROM native_track_plays p JOIN tracks t ON t.id=p.track_id
                    WHERE t.release_id=c.entity_id))
            OR (c.entity_type='artist' AND c.entity_id=ANY($3) AND c.popularity <> (
                    SELECT count(*) FROM native_track_plays p JOIN track_artists a ON a.track_id=p.track_id
                    WHERE a.artist_id=c.entity_id AND a.role='primary'))",
    )
    .bind(&ids).bind(&releases).bind(artists)
    .fetch_one(pool).await.expect("check recommendation freshness");
    assert_eq!(
        inconsistent, 0,
        "recommendation popularity must equal plays"
    );
    let present: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_recommendation_candidates c
         WHERE (c.entity_type='track' AND c.entity_id=ANY($1))
            OR (c.entity_type='release' AND c.entity_id=ANY($2))
            OR (c.entity_type='artist' AND c.entity_id=ANY($3))",
    )
    .bind(&ids)
    .bind(&releases)
    .bind(artists)
    .fetch_one(pool)
    .await
    .expect("check recommendation candidates exist");
    assert_eq!(
        present,
        i64::try_from(ids.len() + releases.len() + artists.len()).unwrap()
    );
}

fn percentile(samples: &mut [u128], pct: usize) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() * pct).div_ceil(100).saturating_sub(1)]
}

fn metric(name: &str) -> melodee_persistence::metrics::MetricSnapshot {
    melodee_persistence::metrics::snapshot()
        .into_iter()
        .find(|entry| entry.name == name)
        .expect("fixed performance metric exists")
}

fn metric_delta(
    before: &melodee_persistence::metrics::MetricSnapshot,
    after: &melodee_persistence::metrics::MetricSnapshot,
) -> (u64, u64, Vec<u64>) {
    (
        after.count.saturating_sub(before.count),
        after.sum_micros.saturating_sub(before.sum_micros),
        after
            .buckets
            .iter()
            .zip(before.buckets.iter())
            .map(|(a, b)| a.saturating_sub(*b))
            .collect(),
    )
}

#[allow(clippy::too_many_arguments)]
async fn scenario(
    pool: &PgPool,
    tracks: &[Track],
    users: &[Uuid],
    name: &'static str,
    round: usize,
    shared: bool,
    move_artist: Option<(Uuid, Uuid)>,
    query_count: &Arc<AtomicU64>,
    stages: &Arc<Mutex<Vec<(u8, u64)>>>,
    overflow: &Arc<AtomicBool>,
) {
    stages.lock().expect("stage samples mutex").clear();
    overflow.store(false, Ordering::Relaxed);
    let admission_before = metric("scrobble_wait_admission");
    let probes_before = metric("scrobble_wait_probe");
    let barrier = Arc::new(Barrier::new(
        WORKERS + usize::from(move_artist.is_some()) + 1,
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let lock_samples = Arc::new(AtomicU64::new(0));
    let max_waiters = Arc::new(AtomicU64::new(0));
    let sampler_queries = Arc::new(AtomicU64::new(0));
    let queries_before = query_count.load(Ordering::Relaxed);
    let sampler = {
        let pool = pool.clone();
        let stop = stop.clone();
        let lock_samples = lock_samples.clone();
        let max_waiters = max_waiters.clone();
        let sampler_queries = sampler_queries.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let waiters: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND application_name='melodee-rm18-contention' AND wait_event_type='Lock'")
                    .fetch_one(&pool).await.expect("sample PostgreSQL lock waits");
                sampler_queries.fetch_add(1, Ordering::Relaxed);
                let waiters = u64::try_from(waiters).unwrap();
                lock_samples.fetch_add(waiters, Ordering::Relaxed);
                max_waiters.fetch_max(waiters, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };
    let started = Instant::now();
    let first_play = Arc::new(AtomicU64::new(u64::MAX));
    let last_play = Arc::new(AtomicU64::new(0));
    let first_relationship = Arc::new(AtomicU64::new(u64::MAX));
    let last_relationship = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();
    for worker in 0..WORKERS {
        let pool = pool.clone();
        let barrier = barrier.clone();
        let first_play = first_play.clone();
        let last_play = last_play.clone();
        let track = tracks[if shared { 0 } else { worker }];
        let user = users[worker];
        tasks.spawn(async move {
            let mut samples = Vec::with_capacity(PER_WORKER);
            barrier.wait().await;
            for iteration in 0..PER_WORKER {
                let now = time::OffsetDateTime::now_utc();
                let event = format!("rm18-{name}-{round}-{worker}-{iteration}");
                let start = Instant::now();
                first_play.fetch_min(
                    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                let outcome = NativeUserStateStore::record_scrobble(
                    &pool,
                    user,
                    track.public_id,
                    &event,
                    now,
                    now,
                    Some(30),
                )
                .await;
                last_play.fetch_max(
                    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                samples.push(start.elapsed().as_micros());
                assert_eq!(
                    outcome.expect("record production scrobble"),
                    NativeScrobbleResult::Applied
                );
            }
            samples
        });
    }
    if let Some((artist_a, artist_b)) = move_artist {
        let pool = pool.clone();
        let barrier = barrier.clone();
        let first_relationship = first_relationship.clone();
        let last_relationship = last_relationship.clone();
        let track = tracks[0];
        tasks.spawn(async move {
            barrier.wait().await;
            for iteration in 0..PER_WORKER {
                first_relationship.fetch_min(
                    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                let changed = sqlx::query(
                    "UPDATE track_artists SET artist_id=$2 WHERE id=$1 AND role='primary'",
                )
                .bind(track.benchmark_credit_id)
                .bind(if iteration % 2 == 0 {
                    artist_b
                } else {
                    artist_a
                })
                .execute(&pool)
                .await
                .expect("change primary artist");
                last_relationship.fetch_max(
                    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                assert_eq!(changed.rows_affected(), 1);
            }
            Vec::new()
        });
    }
    barrier.wait().await;
    let mut samples = Vec::with_capacity(WORKERS * PER_WORKER);
    while let Some(result) = tasks.join_next().await {
        samples.extend(result.expect("benchmark task"));
    }
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.await.expect("lock wait sampler");
    assert_eq!(samples.len(), WORKERS * PER_WORKER);
    if move_artist.is_some() {
        assert!(
            first_play.load(Ordering::Relaxed) < last_relationship.load(Ordering::Relaxed)
                && first_relationship.load(Ordering::Relaxed) < last_play.load(Ordering::Relaxed),
            "relationship changes and scrobbles did not overlap"
        );
    }
    let sqlx_events = query_count
        .load(Ordering::Relaxed)
        .saturating_sub(queries_before)
        .saturating_sub(sampler_queries.load(Ordering::Relaxed));
    assert!(sqlx_events > 0, "SQLx statement tracing must be enabled");
    let admission = metric_delta(&admission_before, &metric("scrobble_wait_admission"));
    let probes = metric_delta(&probes_before, &metric("scrobble_wait_probe"));
    println!(
        "scenario={name} round={round} workers={WORKERS} samples={} seconds={:.3} scrobbles_per_second={:.2} p50_us={} p95_us={} p99_us={} errors=0 relationship_updates={} sqlx_query_events={} lock_waiter_samples={} lock_max_waiters={} sampling_interval_ms=5 admission_count={} admission_sum_us={} admission_buckets={:?} probe_count={} probe_sum_us={} probe_buckets={:?}",
        samples.len(),
        elapsed.as_secs_f64(),
        samples.len() as f64 / elapsed.as_secs_f64(),
        percentile(&mut samples.clone(), 50),
        percentile(&mut samples.clone(), 95),
        percentile(&mut samples, 99),
        if move_artist.is_some() { PER_WORKER } else { 0 },
        sqlx_events,
        lock_samples.load(Ordering::Relaxed),
        max_waiters.load(Ordering::Relaxed),
        admission.0,
        admission.1,
        admission.2,
        probes.0,
        probes.1,
        probes.2,
    );
    assert!(
        !overflow.load(Ordering::Relaxed),
        "stage sample bound exceeded"
    );
    let rows = stages.lock().expect("stage samples mutex");
    for stage in [5, 6, 7] {
        assert_eq!(
            rows.iter().filter(|(name, _)| *name == stage).count(),
            WORKERS * PER_WORKER,
            "missing {stage} stage observations"
        );
    }
    for (index, stage) in [
        "binding",
        "track_wait",
        "artist_wait",
        "release_wait",
        "table_wait",
        "play_insert",
        "counters",
        "commit",
    ]
    .iter()
    .enumerate()
    {
        let mut values = rows
            .iter()
            .filter_map(|(name, value)| (usize::from(*name) == index).then_some(u128::from(*value)))
            .collect::<Vec<_>>();
        if !values.is_empty() {
            println!(
                "stage_scenario={name} round={round} stage={stage} samples={} p50_us={} p95_us={} p99_us={}",
                values.len(),
                percentile(&mut values.clone(), 50),
                percentile(&mut values.clone(), 95),
                percentile(&mut values, 99)
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires an invocation-owned representative fixture and a release build"]
async fn recommendation_scrobble_contention() {
    let query_count = Arc::new(AtomicU64::new(0));
    let stages = Arc::new(Mutex::new(Vec::new()));
    let overflow = Arc::new(AtomicBool::new(false));
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(QueryCounter {
        queries: query_count.clone(),
        stages: stages.clone(),
        overflow: overflow.clone(),
    }))
    .expect("install SQLx query counter");
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture database URL");
    let options = PgConnectOptions::from_str(&url)
        .expect("PostgreSQL URL")
        .application_name("melodee-rm18-contention");
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect_with(options)
        .await
        .expect("fixture connection");
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        name.strip_prefix("melodee_template_")
            .is_some_and(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())),
        "refusing a non-fixture database"
    );
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").ok().as_deref(),
        Some(name.as_str()),
        "refusing a database not named by the fixture invocation"
    );
    let users: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username LIKE 'BENCHMARK-%' ORDER BY normalized_username LIMIT $1")
        .bind(WORKERS as i64).fetch_all(&pool).await.unwrap();
    assert_eq!(users.len(), WORKERS);
    let artists: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM artists WHERE canonical_name IN ('Artist 1','Artist 2') ORDER BY canonical_name")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(artists.len(), 2);
    let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as("SELECT DISTINCT ON (t.release_id) t.id,t.public_api_id,t.release_id FROM tracks t ORDER BY t.release_id,t.id LIMIT $1")
        .bind(WORKERS as i64).fetch_all(&pool).await.unwrap();
    assert_eq!(rows.len(), WORKERS);
    let tracks: Vec<Track> = rows
        .into_iter()
        .map(|(id, public_id, release_id)| Track {
            id,
            public_id,
            release_id,
            benchmark_credit_id: Uuid::now_v7(),
        })
        .collect();
    for track in &tracks {
        sqlx::query("INSERT INTO track_artists (id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) SELECT $3,$1,$2,'primary',COALESCE(max(presentation_order),0)+1,'Benchmark artist','',now() FROM track_artists WHERE track_id=$1")
            .bind(track.id).bind(artists[0]).bind(track.benchmark_credit_id).execute(&pool).await.expect("add benchmark billing");
    }
    let track_ids = tracks.iter().map(|track| track.id).collect::<Vec<_>>();
    let mut checked_artists: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT artist_id FROM track_artists WHERE track_id=ANY($1) AND role='primary' ORDER BY artist_id")
        .bind(&track_ids).fetch_all(&pool).await.expect("read all existing primary Artists");
    checked_artists.extend_from_slice(&artists);
    checked_artists.sort_unstable();
    checked_artists.dedup();
    let rounds = std::env::var("MELODEE_PERF_RECOMMENDATION_ROUNDS")
        .map_or(1, |value| value.parse::<usize>().expect("round count"));
    assert!(
        (1..=10).contains(&rounds),
        "round count must be 1 through 10"
    );
    println!(
        "wait_metric_bucket_upper_ms={:?} overflow_bucket=true",
        melodee_persistence::metrics::BUCKET_BOUNDS_MS
    );
    for round in 0..rounds {
        scenario(
            &pool,
            &tracks,
            &users,
            "shared_track",
            round,
            true,
            None,
            &query_count,
            &stages,
            &overflow,
        )
        .await;
        assert_popularity(&pool, &tracks, &checked_artists).await;
        scenario(
            &pool,
            &tracks,
            &users,
            "separate_tracks_shared_artist",
            round,
            false,
            None,
            &query_count,
            &stages,
            &overflow,
        )
        .await;
        assert_popularity(&pool, &tracks, &checked_artists).await;
        scenario(
            &pool,
            &tracks,
            &users,
            "concurrent_artist_changes",
            round,
            true,
            Some((artists[0], artists[1])),
            &query_count,
            &stages,
            &overflow,
        )
        .await;
        assert_popularity(&pool, &tracks, &checked_artists).await;
    }
    println!(
        "recommendation_consistency=exact scenarios=3 rounds={rounds} network=excluded lock_sampling=approximate sqlx_events_exclude_begin_and_trigger_internals=true benchmark_process_peak_rss_kib={:?} postgres_resources=not_measured",
        peak_process_rss_kib()
    );
}
