//! Performance snapshot script for settling behavior.
//!
//! This test is ignored by default and is intended to be executed explicitly when
//! capturing benchmark evidence for evidence artifacts.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{collections::HashMap, fs, path::PathBuf, time::Instant};

use melodee_domain::IngestionState;
use melodee_ingestion::{
    InboundSource, IngestionConfig, discover_release_candidates, reconcile_all_sources,
};
use melodee_persistence::WorkItemStore;
use serde::Serialize;
use time::Duration;
use uuid::Uuid;

#[derive(Debug, Serialize, Clone)]
struct PerformanceRecord {
    scenario: String,
    candidates: usize,
    total_files: usize,
    total_bytes: i64,
    settle_quiet_seconds: i64,
    marker_shortcut_seconds: i64,
    pass_count: usize,
    first_pass_ms: i64,
    second_pass_ms: i64,
    third_pass_ms: Option<i64>,
    ready_count_after_first: usize,
    ready_count_after_second: usize,
    snapshot_rows: i64,
    latest_reconcile_stats: HashMap<String, u64>,
}

#[derive(Debug, Serialize)]
struct SettlementBenchmarks {
    toolchain: String,
    rust_version: String,
    host: String,
    scenarios: Vec<PerformanceRecord>,
    notes: Vec<String>,
}

struct Scenario {
    root: PathBuf,
    source: InboundSource,
    config: IngestionConfig,
    database: common::TestDb,
}

impl Scenario {
    async fn new(name: &str, settling_quiet_seconds: i64, ready_marker_shortcut: i64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "melodee-ingestion-perf-{}-{}",
            name,
            Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&root).expect("create test root");

        let db = common::TestDb::create(name).await;
        let source = InboundSource::from_root(root.clone(), None, Some(format!("{name}-source")))
            .unwrap_or_else(|error| panic!("source from root: {error}"));
        let mut config = IngestionConfig::with_sources(vec![source.clone()]);
        config.settling_quiet_period = Duration::seconds(settling_quiet_seconds);
        config.ready_marker_shortcut = Duration::seconds(ready_marker_shortcut);
        Self {
            root,
            source,
            config,
            database: db,
        }
    }

    async fn create_release(&self, name: &str, file_count: usize, with_ready_marker: bool) {
        let release = self.root.join(name);
        fs::create_dir_all(&release).expect("create release");

        for index in 0..file_count {
            fs::write(
                release.join(format!("track-{index:04}.flac")),
                vec![b'a'; 32],
            )
            .expect("write track file");
        }

        if with_ready_marker {
            fs::write(release.join(".melodee-ready"), b"ready").expect("write marker");
        }
    }

    async fn create_file_tree(&self, name: &str, nested_dirs: usize, files_per_dir: usize) {
        let release = self.root.join(name);
        fs::create_dir_all(&release).expect("create release");

        for dir_index in 0..nested_dirs {
            let dir = release.join(format!("dir-{dir_index:02}"));
            fs::create_dir_all(&dir).expect("create nested dir");
            for file_index in 0..files_per_dir {
                fs::write(
                    dir.join(format!("track-{file_index:04}.flac")),
                    vec![b'b'; 64],
                )
                .expect("write nested file");
            }
        }

        for index in 0..(files_per_dir.max(1) / 2) {
            fs::write(release.join(format!("root-{index:04}.cue")), vec![b'c'; 16])
                .expect("write misc");
        }
    }

    async fn count_files(&self) -> usize {
        let mut count = 0_usize;
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir).expect("list dir");
            for entry in entries {
                let entry = entry.expect("dir entry");
                let meta = entry.metadata().expect("entry metadata");
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    count += 1;
                }
            }
        }
        count
    }

    async fn candidate_count(&self) -> usize {
        discover_release_candidates(&self.source)
            .expect("discover candidates")
            .len()
    }

    async fn reconcile(&self, now_seconds: i64) -> Vec<melodee_ingestion::ReconcileResult> {
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.database.pool,
            common::T0 + Duration::seconds(now_seconds),
        )
        .await
        .expect("reconcile")
    }

    async fn ready_count(&self) -> usize {
        WorkItemStore
            .list(&self.database.pool, None, 10_000, 0)
            .await
            .expect("list")
            .into_iter()
            .filter(|item| item.state == IngestionState::Ready)
            .count()
    }

    async fn snapshot_row_count(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM release_work_item_settling_snapshots")
            .fetch_one(&self.database.pool)
            .await
            .expect("count snapshots")
    }
}

#[tokio::test]
#[ignore]
async fn performance_snapshot_settling() {
    use std::env;

    let rust_version =
        env::var("RUSTC_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    let host = env::var("HOSTNAME").unwrap_or_else(|_| "local".to_owned());

    let mut scenarios = Vec::new();

    let small = Scenario::new("phase1b-small", 2, 0).await;
    small.create_release("release-small", 12, false).await;

    let before = Instant::now();
    let first = small.reconcile(0).await;
    let first_ms = before.elapsed().as_millis() as i64;

    let before = Instant::now();
    let _second = small.reconcile(1).await;
    let second_ms = before.elapsed().as_millis() as i64;

    let before = Instant::now();
    let _third = small.reconcile(4).await;
    let third_ms = before.elapsed().as_millis() as i64;

    let first_stats = first.first().expect("small first");
    let first_map = stats_to_map(first_stats);

    scenarios.push(PerformanceRecord {
        scenario: "small-release-stability-pass".to_owned(),
        candidates: small.candidate_count().await,
        total_files: small.count_files().await,
        total_bytes: 0,
        settle_quiet_seconds: 2,
        marker_shortcut_seconds: 0,
        pass_count: 3,
        first_pass_ms: first_ms,
        second_pass_ms: second_ms,
        third_pass_ms: Some(third_ms),
        ready_count_after_first: small.ready_count().await,
        ready_count_after_second: small.ready_count().await,
        snapshot_rows: small.snapshot_row_count().await,
        latest_reconcile_stats: first_map,
    });

    let large = Scenario::new("phase1b-large", 2, 0).await;
    large.create_file_tree("release-large", 12, 20).await;

    let before = Instant::now();
    let _first = large.reconcile(0).await;
    let first_ms = before.elapsed().as_millis() as i64;

    let before = Instant::now();
    let second = large.reconcile(1).await;
    let second_ms = before.elapsed().as_millis() as i64;

    let second_map = stats_to_map(second.first().expect("large second"));

    scenarios.push(PerformanceRecord {
        scenario: "large-release-stability-pass".to_owned(),
        candidates: large.candidate_count().await,
        total_files: large.count_files().await,
        total_bytes: 0,
        settle_quiet_seconds: 2,
        marker_shortcut_seconds: 0,
        pass_count: 2,
        first_pass_ms: first_ms,
        second_pass_ms: second_ms,
        third_pass_ms: None,
        ready_count_after_first: large.ready_count().await,
        ready_count_after_second: large.ready_count().await,
        snapshot_rows: large.snapshot_row_count().await,
        latest_reconcile_stats: second_map,
    });

    let mut two_hundred = Scenario::new("phase1b-two-hundred", 1, 0).await;
    two_hundred.config.reconciliation_interval = Duration::seconds(1);

    for index in 0..200 {
        let file_count = ((index % 7) + 1) * 2;
        two_hundred
            .create_release(&format!("release-{index:03}"), file_count, index % 10 == 0)
            .await;
    }

    let before = Instant::now();
    let first = two_hundred.reconcile(0).await;
    let first_ms = before.elapsed().as_millis() as i64;

    let before = Instant::now();
    let second = two_hundred.reconcile(2).await;
    let second_ms = before.elapsed().as_millis() as i64;

    let first_map = stats_to_map(first.first().expect("two-hundred first"));
    let _second_map = stats_to_map(second.first().expect("two-hundred second"));

    scenarios.push(PerformanceRecord {
        scenario: "two-hundred-initial-reconcile".to_owned(),
        candidates: 200,
        total_files: two_hundred.count_files().await,
        total_bytes: 0,
        settle_quiet_seconds: 1,
        marker_shortcut_seconds: 0,
        pass_count: 2,
        first_pass_ms: first_ms,
        second_pass_ms: second_ms,
        third_pass_ms: None,
        ready_count_after_first: two_hundred.ready_count().await,
        ready_count_after_second: two_hundred.ready_count().await,
        snapshot_rows: two_hundred.snapshot_row_count().await,
        latest_reconcile_stats: first_map,
    });

    let artifact = SettlementBenchmarks {
        toolchain: rust_version,
        rust_version: env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_owned()),
        host,
        scenarios,
        notes: vec![
            "Ignore-mode benchmark capturing only".to_string(),
            "No file-content hash or copy is performed; only metadata snapshot".to_string(),
            "No thresholds are enforced; this artifact is informational only".to_string(),
        ],
    };

    let output =
        std::path::Path::new("target/verification/performance/perf-settling-benchmark.json");
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("make evidence directory");
    }
    fs::write(
        output,
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark") + "\n",
    )
    .expect("write evidence");

    println!("wrote {output:?}");
}

fn stats_to_map(result: &melodee_ingestion::ReconcileResult) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    map.insert("candidates_seen".to_owned(), result.stats.candidates_seen);
    map.insert("new_work_items".to_owned(), result.stats.new_work_items);
    map.insert(
        "existing_work_items".to_owned(),
        result.stats.existing_work_items,
    );
    map.insert("settling_updated".to_owned(), result.stats.settling_updated);
    map.insert("ready_promoted".to_owned(), result.stats.ready_promoted);
    map.insert("ready_reverted".to_owned(), result.stats.ready_reverted);
    map.insert("snapshot_changed".to_owned(), result.stats.snapshot_changed);
    map.insert(
        "missing_candidates".to_owned(),
        result.stats.missing_candidates,
    );
    map.insert("errors".to_owned(), result.stats.errors);
    map.insert(
        "duration_ms".to_owned(),
        result.stats.duration_ms.max(0) as u64,
    );
    map
}
