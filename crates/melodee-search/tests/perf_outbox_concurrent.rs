//! Mixed production search/outbox/rebuild benchmark on an owned database.
#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use melodee_persistence::{SEARCH_PROJECTION_NAME, SearchProjectionStore};
use melodee_search::{
    SearchConfig, SearchRebuildTestBarrier, SearchRoot, apply_outbox_once, query_active,
    request_rebuild, run_rebuild_once,
};
use serde::Serialize;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const CLIENTS: usize = 4;
const MAX_SAMPLES_PER_CLIENT: usize = 2048;
const CLIENT_INTERVAL_MS: u64 = 50;
static COUNT_SQL: AtomicBool = AtomicBool::new(false);
static SQL_EVENTS: AtomicU64 = AtomicU64::new(0);

struct SqlCounter;
impl<S: tracing::Subscriber> Layer<S> for SqlCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" && COUNT_SQL.load(Ordering::Relaxed) {
            SQL_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn owned_database_name(name: &str) -> bool {
    name.strip_prefix("melodee_template_")
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn segment_files(path: &Path) -> Option<usize> {
    Some(
        std::fs::read_dir(path)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "term"))
            .count(),
    )
}

#[derive(Serialize)]
struct QuerySample {
    started_ms: f64,
    elapsed_ms: f64,
    success: bool,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit concurrent production-path run on an owned disposable database"]
async fn representative_search_during_rebuild_and_outbox() {
    let marker = std::env::var("MELODEE_PERF_OWNED_DATABASE").expect("owned database marker");
    assert!(
        owned_database_name(&marker),
        "invalid owned database marker"
    );
    let url = std::env::var("MELODEE__DATABASE__URL").expect("disposable database URL");
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(&url)
        .await
        .expect("fixture connection");
    let database = sqlx::query_scalar::<_, String>("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("identify fixture database");
    assert_eq!(database, marker, "refusing a database identity mismatch");
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                // Per-query reload debug output would distort the latency
                // sample; writer/reload costs have a separate benchmark.
                .with_filter(tracing_subscriber::EnvFilter::new(
                    "warn,melodee_search::rebuild=info",
                )),
        )
        .with(SqlCounter.with_filter(tracing_subscriber::EnvFilter::new("sqlx::query=debug")))
        .try_init();
    let directory = tempfile::tempdir().expect("owned search directory");
    let config = SearchConfig {
        search_root: directory.path().join("search"),
        enabled: true,
        ..SearchConfig::default()
    };
    let (artists, releases, tracks) = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)",
    ).fetch_one(&pool).await.expect("count fixture");
    request_rebuild(&config, &pool, "disposable-concurrent-search-benchmark")
        .await
        .expect("request initial rebuild");
    run_rebuild_once(&config, &pool)
        .await
        .expect("initial rebuild")
        .expect("initial claim");
    let root = SearchRoot::open(&config.search_root).expect("owned root");
    let old =
        SearchProjectionStore::active_generation(&pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read generation")
            .expect("initial generation");
    assert_eq!(old.current_document_counts.artists, artists as u64);
    assert_eq!(old.current_document_counts.releases, releases as u64);
    assert_eq!(old.current_document_counts.tracks, tracks as u64);
    let before_segments = segment_files(&root.generation_path(old.generation_id));
    let probes = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        "SELECT (SELECT id FROM artists ORDER BY id LIMIT 1), (SELECT id FROM releases ORDER BY id LIMIT 1), (SELECT id FROM tracks ORDER BY id LIMIT 1)",
    ).fetch_one(&pool).await.expect("select probe identities");
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox WHERE projection_name = $1 AND processed_at IS NULL",
    ).bind(SEARCH_PROJECTION_NAME).fetch_one(&pool).await.expect("read pending rows");
    assert_eq!(
        pending, 0,
        "fixture must begin with an empty pending outbox"
    );

    let barrier = SearchRebuildTestBarrier::new();
    barrier.request_snapshot_pause();
    barrier.request_query_pause();
    let mut rebuild_config = config.clone();
    rebuild_config.rebuild_test_barrier = Some(barrier.clone());
    request_rebuild(
        &rebuild_config,
        &pool,
        "disposable-concurrent-search-benchmark",
    )
    .await
    .expect("request concurrent rebuild");
    let origin = Instant::now();
    let mut held_config = config.clone();
    held_config.rebuild_test_barrier = Some(barrier.clone());
    let held_pool = pool.clone();
    let held_artist = probes.0;
    let held_query = tokio::spawn(async move {
        query_active(
            &held_config,
            &held_pool,
            &format!("id:{held_artist}"),
            10,
            0,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(120), barrier.wait_until_query_paused())
        .await
        .expect("query selected its old-generation reader");
    let held_reader_selected_ms = origin.elapsed().as_secs_f64() * 1000.0;
    let rebuild_pool = pool.clone();
    let rebuild = tokio::spawn(async move {
        let result = run_rebuild_once(&rebuild_config, &rebuild_pool).await;
        (result, Instant::now())
    });
    tokio::time::timeout(
        Duration::from_secs(120),
        barrier.wait_until_snapshot_paused(),
    )
    .await
    .expect("rebuild reached snapshot barrier");
    let snapshot_paused_ms = origin.elapsed().as_secs_f64() * 1000.0;
    // Snapshot is fixed before these writes. Activation must replay them even
    // if the old generation's ordinary consumer acknowledges first.
    let expected = sqlx::query(
        "WITH selected_artist AS MATERIALIZED (SELECT id FROM artists ORDER BY id LIMIT 1024),
         selected_release AS MATERIALIZED (SELECT id FROM releases ORDER BY id LIMIT 512),
         selected_track AS MATERIALIZED (SELECT id FROM tracks ORDER BY id LIMIT 512),
         updated_artist AS (UPDATE artists SET canonical_name = canonical_name || ' Updated',
           normalized_name = normalized_name || ' updated', revision = revision + 1, updated_at = now()
           WHERE id IN (SELECT id FROM selected_artist) RETURNING id, revision),
         updated_release AS (UPDATE releases SET title = title || ' Updated', normalized_title = normalized_title || ' updated',
           revision = revision + 1, updated_at = now() WHERE id IN (SELECT id FROM selected_release) RETURNING id, revision),
         updated_track AS (UPDATE tracks SET title = title || ' Updated', normalized_title = normalized_title || ' updated',
           revision = revision + 1, updated_at = now() WHERE id IN (SELECT id FROM selected_track) RETURNING id, revision),
         events AS (SELECT 'artist' AS kind, id, revision, row_number() OVER (ORDER BY id) AS ordinal FROM updated_artist
           UNION ALL SELECT 'release', id, revision, row_number() OVER (ORDER BY id) FROM updated_release
           UNION ALL SELECT 'track', id, revision, row_number() OVER (ORDER BY id) FROM updated_track)
         INSERT INTO catalog_projection_outbox (projection_name, entity_type, entity_id, operation, entity_revision, available_at, created_at)
         SELECT $1, kind, id, 'upsert', revision, now(), now() FROM events ORDER BY ordinal, kind",
    ).bind(SEARCH_PROJECTION_NAME).execute(&pool).await.expect("seed mixed revisions").rows_affected();
    assert!(expected > 0);

    let stop = Arc::new(AtomicBool::new(false));
    let start = Arc::new(tokio::sync::Barrier::new(CLIENTS + 2));
    let mut clients = Vec::new();
    for _ in 0..CLIENTS {
        let pool = pool.clone();
        let config = config.clone();
        let stop = Arc::clone(&stop);
        let start = Arc::clone(&start);
        let id = probes.0;
        clients.push(tokio::spawn(async move {
            let mut samples = Vec::new();
            start.wait().await;
            while !stop.load(Ordering::Acquire) && samples.len() < MAX_SAMPLES_PER_CLIENT {
                let started = Instant::now();
                let hits = query_active(&config, &pool, &format!("id:{id}"), 10, 0).await;
                let success =
                    hits.is_ok_and(|hits| hits.len() == 1 && hits[0].internal_id == id.to_string());
                samples.push(QuerySample {
                    started_ms: started.duration_since(origin).as_secs_f64() * 1000.0,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    success,
                });
                tokio::time::sleep(Duration::from_millis(CLIENT_INTERVAL_MS)).await;
            }
            samples
        }));
    }
    let worker_pool = pool.clone();
    let worker_config = config.clone();
    let worker_start = Arc::clone(&start);
    let old_root = root.clone();
    let old_generation = old.generation_id;
    let worker = tokio::spawn(async move {
        worker_start.wait().await;
        let begin_ms = origin.elapsed().as_secs_f64() * 1000.0;
        let mut claimed = 0_u64;
        let mut applied = 0_u64;
        let mut acknowledged = 0_u64;
        let mut elapsed_ms = Vec::new();
        while acknowledged < expected {
            let begin = Instant::now();
            let pass = apply_outbox_once(&worker_config, &worker_pool)
                .await
                .map_err(|error| error.to_string())?;
            elapsed_ms.push(begin.elapsed().as_secs_f64() * 1000.0);
            if pass.errors != 0 || pass.claimed == 0 {
                return Err("outbox worker failed or did not advance".to_string());
            }
            claimed += pass.claimed as u64;
            applied += pass.applied as u64;
            acknowledged += pass.acknowledged as u64;
            if claimed > expected {
                return Err("unexpected fixture outbox rows".to_string());
            }
        }
        let end_ms = origin.elapsed().as_secs_f64() * 1000.0;
        let active = SearchProjectionStore::active_generation(
            &worker_pool,
            old_root.path().to_string_lossy().as_ref(),
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok::<_, String>((
            begin_ms,
            end_ms,
            elapsed_ms,
            claimed,
            applied,
            acknowledged,
            active.is_some_and(|generation| generation.generation_id == old_generation),
            segment_files(&old_root.generation_path(old_generation)),
        ))
    });
    let rss_before_kib = peak_rss_kib();
    println!("BEGIN_CONCURRENT_SEARCH_MEASUREMENT");
    COUNT_SQL.store(true, Ordering::Relaxed);
    start.wait().await;
    barrier.release();
    barrier.release_snapshot();
    let snapshot_released_ms = origin.elapsed().as_secs_f64() * 1000.0;
    let (rebuilt, drained) = tokio::join!(rebuild, worker);
    // This request selected the old reader before activation; resume it only
    // after the new generation is active and retirement has run.
    barrier.release_query();
    let held_query_success = held_query
        .await
        .expect("join held query")
        .is_ok_and(|hits| hits.len() == 1 && hits[0].internal_id == probes.0.to_string());
    // Keep clients running briefly after activation so the samples include
    // the newly active generation, not only requests before the switch.
    tokio::time::sleep(Duration::from_millis(CLIENT_INTERVAL_MS * 3)).await;
    stop.store(true, Ordering::Release);
    let mut samples = Vec::new();
    for client in clients {
        samples.extend(client.await.expect("join query client"));
    }
    COUNT_SQL.store(false, Ordering::Relaxed);
    let (rebuilt, rebuild_done) = rebuilt.expect("join rebuild");
    let drained = drained
        .expect("join outbox worker")
        .expect("drain outbox worker");
    assert!(rebuilt.expect("concurrent rebuild").is_some());
    assert_eq!(drained.3, expected);
    assert_eq!(drained.4, expected);
    assert_eq!(drained.5, expected);
    assert!(!samples.is_empty());
    let query_errors = samples.iter().filter(|sample| !sample.success).count();
    let activated = barrier.activated_at().expect("activation time");
    let active =
        SearchProjectionStore::active_generation(&pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("read final generation")
            .expect("final generation");
    assert_ne!(active.generation_id, old.generation_id);
    for (kind, id) in [
        ("artist", probes.0),
        ("release", probes.1),
        ("track", probes.2),
    ] {
        let hits = query_active(&config, &pool, &format!("id:{id}"), 10, 0)
            .await
            .expect("verify final entity");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, kind);
        assert_eq!(hits[0].internal_id, id.to_string());
        assert!(hits[0].display.ends_with(" Updated"));
    }
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox WHERE projection_name = $1 AND processed_at IS NULL",
    ).bind(SEARCH_PROJECTION_NAME).fetch_one(&pool).await.expect("count final pending rows");
    assert_eq!(pending, 0);
    println!(
        "{}",
        json!({
            "workload":"postgres-search-concurrent-rebuild-outbox",
            "build_debug_assertions":cfg!(debug_assertions),
            "fixture":{"artists":artists,"releases":releases,"tracks":tracks},
            "events":expected,"batch_size":config.claim_batch_size,"batch_elapsed_ms":drained.2,
            "outbox_started_ms":drained.0,"outbox_finished_ms":drained.1,
            "old_generation_active_at_drain_finish":drained.6,
            "old_generation_segments_before":before_segments,"old_generation_segments_at_drain_finish":drained.7,
            "new_generation_segments_after":segment_files(&root.generation_path(active.generation_id)),
            "snapshot_paused_ms":snapshot_paused_ms,"snapshot_released_ms":snapshot_released_ms,
            "activation_completed_ms":activated.duration_since(origin).as_secs_f64()*1000.0,
            "rebuild_returned_ms":rebuild_done.duration_since(origin).as_secs_f64()*1000.0,
            "held_reader_selected_ms":held_reader_selected_ms,"held_query_succeeded":held_query_success,
            "clients":CLIENTS,"client_interval_ms":CLIENT_INTERVAL_MS,"max_samples_per_client":MAX_SAMPLES_PER_CLIENT,
        "post_rebuild_observation_ms":CLIENT_INTERVAL_MS * 3,
            "query_samples":samples,"query_errors":query_errors,"sqlx_statement_events_all_concurrent_work":SQL_EVENTS.load(Ordering::Relaxed),
            "peak_process_rss_before_kib":rss_before_kib,"peak_process_rss_after_kib":peak_rss_kib(),
            "limitations":"feature-gated deterministic snapshot barrier; one in-process outbox worker and four in-process search clients; no HTTP, physical media or multi-process contention; SQLx statements are not wire round trips; process RSS is high water including initial rebuild"
        })
    );
    assert_eq!(
        query_errors, 0,
        "interactive query failure or missing document"
    );
    assert!(
        held_query_success,
        "old-generation in-flight query failed after activation"
    );
}

#[test]
fn requires_exact_disposable_database_name() {
    assert!(owned_database_name(
        "melodee_template_0123456789abcdef0123456789abcdef"
    ));
    assert!(!owned_database_name(
        "melodee_template_0123456789abcdef0123456789abcdeg"
    ));
    assert!(!owned_database_name("production"));
}
