//! Explicit incremental-index workload over the owned PostgreSQL fixture.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use melodee_persistence::{SEARCH_PROJECTION_NAME, SearchProjectionStore};
use melodee_search::{
    SearchConfig, SearchRoot, apply_outbox_once, query_active, request_rebuild, run_rebuild_once,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

static COUNT_SQL: AtomicBool = AtomicBool::new(false);
static SQL_STATEMENTS: AtomicU64 = AtomicU64::new(0);

struct SqlStatementCounter;

impl<S: tracing::Subscriber> Layer<S> for SqlStatementCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" && COUNT_SQL.load(Ordering::Relaxed) {
            SQL_STATEMENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn owned_database_name(name: &str) -> bool {
    name.strip_prefix("melodee_template_")
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[tokio::test]
#[ignore = "explicit production-path performance run on an owned disposable database"]
async fn representative_incremental_search_outbox() {
    let marker = std::env::var("MELODEE_PERF_OWNED_DATABASE").expect("owned database marker");
    assert!(
        owned_database_name(&marker),
        "invalid owned database marker"
    );
    let url = std::env::var("MELODEE__DATABASE__URL").expect("disposable database URL");
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("connect disposable database");
    let database = sqlx::query_scalar::<_, String>("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("identify disposable database");
    assert_eq!(database, marker, "refusing a database identity mismatch");
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_filter(tracing_subscriber::EnvFilter::new(
                    "warn,melodee_search=debug",
                )),
        )
        .with(
            SqlStatementCounter
                .with_filter(tracing_subscriber::EnvFilter::new("sqlx::query=debug")),
        )
        .try_init();
    let directory = tempfile::tempdir().expect("owned temporary search directory");
    let config = SearchConfig {
        search_root: directory.path().join("search"),
        enabled: true,
        ..SearchConfig::default()
    };
    let (artists, releases, tracks) = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)",
    )
    .fetch_one(&pool)
    .await
    .expect("count fixture");
    request_rebuild(&config, &pool, "disposable-outbox-benchmark")
        .await
        .expect("request rebuild");
    run_rebuild_once(&config, &pool)
        .await
        .expect("execute production rebuild")
        .expect("claimed rebuild");
    let root = SearchRoot::open(&config.search_root).expect("open owned root");
    let active =
        SearchProjectionStore::active_generation(&pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("load active generation")
            .expect("rebuild activated");
    assert_eq!(active.current_document_counts.artists, artists as u64);
    assert_eq!(active.current_document_counts.releases, releases as u64);
    assert_eq!(active.current_document_counts.tracks, tracks as u64);
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox WHERE projection_name = $1 AND processed_at IS NULL",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .fetch_one(&pool)
    .await
    .expect("check fixture outbox");
    assert_eq!(
        pending, 0,
        "fixture must begin with an empty pending outbox"
    );

    let probe = sqlx::query_scalar::<_, Uuid>("SELECT id FROM artists ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("select probe Artist");
    let release_probe =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM releases ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("select probe Release");
    let track_probe = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tracks ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("select probe Track");
    // Each trial writes matching revision changes and mixed outbox obligations
    // atomically; production workers perform all timed drain operations.
    for trial in 1..=3 {
        let previous_id = sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(max(id), 0) FROM catalog_projection_outbox",
        )
        .fetch_one(&pool)
        .await
        .expect("capture outbox boundary");
        let expected = sqlx::query(
            "WITH selected_artist AS MATERIALIZED (SELECT id FROM artists ORDER BY id LIMIT 1024),
             selected_release AS MATERIALIZED (SELECT id FROM releases ORDER BY id LIMIT 512),
             selected_track AS MATERIALIZED (SELECT id FROM tracks ORDER BY id LIMIT 512),
             updated_artist AS (
               UPDATE artists SET canonical_name = canonical_name || ' Updated',
                 normalized_name = normalized_name || ' updated', revision = revision + 1,
                 updated_at = now() WHERE id IN (SELECT id FROM selected_artist) RETURNING id, revision
             ), updated_release AS (
               UPDATE releases SET title = title || ' Updated', normalized_title = normalized_title || ' updated',
                 revision = revision + 1, updated_at = now()
               WHERE id IN (SELECT id FROM selected_release) RETURNING id, revision
             ), updated_track AS (
               UPDATE tracks SET title = title || ' Updated', normalized_title = normalized_title || ' updated',
                 revision = revision + 1, updated_at = now()
               WHERE id IN (SELECT id FROM selected_track) RETURNING id, revision
             ), events AS (
               SELECT 'artist' AS kind, id, revision, row_number() OVER (ORDER BY id) AS ordinal FROM updated_artist
               UNION ALL SELECT 'release', id, revision, row_number() OVER (ORDER BY id) FROM updated_release
               UNION ALL SELECT 'track', id, revision, row_number() OVER (ORDER BY id) FROM updated_track
             )
             INSERT INTO catalog_projection_outbox (projection_name, entity_type, entity_id, operation,
                                                   entity_revision, available_at, created_at)
             SELECT $1, kind, id, 'upsert', revision, now(), now() FROM events ORDER BY ordinal, kind",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .execute(&pool)
        .await
        .expect("seed bounded mixed outbox")
        .rows_affected();
        assert!(expected > 0);
        let peak_before_kib = peak_rss_kib();
        println!("BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial={trial}");
        SQL_STATEMENTS.store(0, Ordering::Relaxed);
        COUNT_SQL.store(true, Ordering::Relaxed);
        let started = Instant::now();
        let mut samples_ms = Vec::new();
        let mut claimed = 0_u64;
        let mut applied = 0_u64;
        let mut acknowledged = 0_u64;
        while acknowledged < expected {
            let pass_started = Instant::now();
            let pass = apply_outbox_once(&config, &pool)
                .await
                .expect("drain outbox pass");
            samples_ms.push(pass_started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(pass.errors, 0, "unexpected outbox error");
            assert!(pass.claimed > 0, "outbox stopped making progress");
            claimed += pass.claimed as u64;
            applied += pass.applied as u64;
            acknowledged += pass.acknowledged as u64;
            assert!(claimed <= expected, "unexpected fixture outbox rows");
        }
        let elapsed = started.elapsed().as_secs_f64();
        COUNT_SQL.store(false, Ordering::Relaxed);
        let statement_count = SQL_STATEMENTS.load(Ordering::Relaxed);
        assert_eq!(claimed, expected);
        assert_eq!(applied, expected);
        assert_eq!(acknowledged, expected);
        let (artist_events, release_events, track_events, lag_ms) = sqlx::query_as::<_, (i64, i64, i64, f64)>(
            "SELECT count(*) FILTER (WHERE entity_type = 'artist'), count(*) FILTER (WHERE entity_type = 'release'),
                    count(*) FILTER (WHERE entity_type = 'track'),
                    coalesce(extract(epoch FROM max(processed_at - created_at))::double precision * 1000, 0)
             FROM catalog_projection_outbox WHERE projection_name = $1 AND id > $2 AND processed_at IS NOT NULL",
        )
        .bind(SEARCH_PROJECTION_NAME)
        .bind(previous_id)
        .fetch_one(&pool)
        .await
        .expect("measure mixed outbox lag");
        assert_eq!(
            artist_events + release_events + track_events,
            expected as i64
        );
        assert!(artist_events > 0 && release_events > 0 && track_events > 0);
        let mut query_samples_ms = Vec::new();
        for _ in 0..11 {
            let started = Instant::now();
            let hits = query_active(&config, &pool, &format!("id:{probe}"), 10, 0)
                .await
                .expect("query updated Artist");
            query_samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].internal_id, probe.to_string());
            assert!(hits[0].display.ends_with(" Updated"));
        }
        // These untimed checks ensure the mixed batch updates each entity
        // kind's indexed content, beyond its acknowledgement count.
        for (kind, id) in [("release", release_probe), ("track", track_probe)] {
            let hits = query_active(&config, &pool, &format!("id:{id}"), 10, 0)
                .await
                .expect("query updated mixed entity");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].kind, kind);
            assert_eq!(hits[0].internal_id, id.to_string());
            assert!(hits[0].display.ends_with(" Updated"));
        }
        println!(
            "{}",
            json!({
                "workload":"postgres-search-incremental", "trial":trial,
                "build_debug_assertions":cfg!(debug_assertions),
                "fixture":{"artists":artists, "releases":releases, "tracks":tracks},
                "events":expected, "artist_events":artist_events, "release_events":release_events,
                "track_events":track_events, "batches":samples_ms.len(), "batch_size":config.claim_batch_size,
                "elapsed_seconds":elapsed, "events_per_second":expected as f64 / elapsed,
                "sqlx_statement_events":statement_count, "max_end_to_end_outbox_lag_ms":lag_ms,
                "batch_elapsed_ms":samples_ms, "query_elapsed_ms":query_samples_ms,
                "peak_process_rss_before_kib":peak_before_kib, "peak_process_rss_after_kib":peak_rss_kib(),
                "limitations":"three consecutive trials on one process and catalog; mixed upserts only; no concurrent activity or physical media; SQL setup excluded; query timings in-process; SQLx events count statements, not network round trips; RSS is process high-water including the full rebuild"
            })
        );
    }
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
