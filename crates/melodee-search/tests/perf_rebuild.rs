//! Explicit, disposable PostgreSQL-to-Tantivy rebuild measurement.
//!
//! This is ignored by normal tests. Run via scripts/perf_representative.py;
//! the exact invocation-owned database marker is mandatory.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::time::Instant;

use melodee_persistence::SearchProjectionStore;
use melodee_search::{SearchConfig, SearchRoot, request_rebuild, run_rebuild_once};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

fn owned_database_name(name: &str) -> bool {
    name.strip_prefix("melodee_template_")
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn directory_size(path: &Path) -> std::io::Result<u64> {
    let mut bytes = 0_u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if metadata.is_dir() {
            bytes = bytes.saturating_add(directory_size(&entry.path())?);
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(bytes)
}

fn peak_rss_kib() -> Option<u64> {
    // Linux-specific measurement is optional; absence is recorded as null.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

#[tokio::test]
#[ignore = "explicit production-path performance run on an owned disposable database"]
async fn representative_postgres_search_rebuild() {
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

    // Search events include writer lifecycle, individual commits, snapshot
    // age and activation timings. Keep SQL logging off to avoid parameters.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("warn,melodee_search=debug")
        .without_time()
        .with_ansi(false)
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
    let pg_settings = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT current_setting('server_version'), current_setting('shared_buffers'), current_setting('work_mem'), current_setting('max_connections')",
    )
    .fetch_one(&pool)
    .await
    .expect("read PostgreSQL settings");
    request_rebuild(&config, &pool, "disposable-performance-benchmark")
        .await
        .expect("request rebuild");
    let start = Instant::now();
    run_rebuild_once(&config, &pool)
        .await
        .expect("execute production rebuild")
        .expect("claimed rebuild");
    let elapsed = start.elapsed().as_secs_f64();
    let root = SearchRoot::open(&config.search_root).expect("open owned root");
    let active =
        SearchProjectionStore::active_generation(&pool, root.path().to_string_lossy().as_ref())
            .await
            .expect("load active generation")
            .expect("rebuild activated");
    assert_eq!(active.current_document_counts.artists, artists as u64);
    assert_eq!(active.current_document_counts.releases, releases as u64);
    assert_eq!(active.current_document_counts.tracks, tracks as u64);
    let total = artists + releases + tracks;
    let mut config_values = serde_json::to_value(&config).expect("serialize search configuration");
    if let Some(object) = config_values.as_object_mut() {
        object.remove("search_root");
    }
    println!(
        "{}",
        json!({
            "workload": "postgres-search-rebuild",
            "build_debug_assertions": cfg!(debug_assertions),
            "fixture": {"artists": artists, "releases": releases, "tracks": tracks},
            "postgres": {"version": pg_settings.0, "shared_buffers": pg_settings.1, "work_mem": pg_settings.2, "max_connections": pg_settings.3},
            "search_config": config_values,
            "elapsed_seconds": elapsed,
            "documents_per_second": (total as f64) / elapsed,
            "logical_index_bytes": directory_size(root.path()).expect("sum index file bytes"),
            "peak_process_rss_kib": peak_rss_kib(),
            "cpu_parallelism": std::thread::available_parallelism().ok().map(|v| v.get()),
            "cache_conditions": "uncontrolled after fixture seed",
            "sample_count": 1,
            "limitations": "fixture metadata and relationships are synthetic; no physical media, ACLs or concurrent workloads; RSS is process-wide high-water mark; index size is logical file bytes"
        })
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
    assert!(!owned_database_name(
        "melodee_template_0123456789abcdef0123456789abcdef_extra"
    ));
    assert!(!owned_database_name("production"));
}
