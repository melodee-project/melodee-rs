//! Media inspection integration scenarios using real PostgreSQL and ffprobe.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{fs, path::PathBuf};

use melodee_domain::IngestionState;
use melodee_ingestion::{
    InboundSource, IngestionConfig, inspect_ready_once, reconcile_all_sources,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore};
use time::Duration;
use uuid::Uuid;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: sqlx::PgPool,
    database: common::TestDb,
}

impl Scenario {
    async fn new(name: &str) -> Self {
        let unique = Uuid::now_v7();
        let root = std::env::temp_dir().join(format!("melodee-inspection-{name}-{unique}"));
        fs::create_dir_all(&root).expect("root");
        let database = common::TestDb::create(name).await;
        let source =
            InboundSource::from_root(root.clone(), None, Some(name.to_owned())).expect("source");
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.settling_quiet_period = Duration::ZERO;
        Self {
            root,
            config,
            pool: database.pool.clone(),
            database,
        }
    }

    async fn settle(&self, name: &str) -> melodee_domain::ReleaseWorkItem {
        let release = self.root.join(name);
        fs::create_dir_all(&release).expect("release");
        write_tagless_wav(&release.join("track.wav"));
        reconcile_all_sources(&self.config, &WorkItemStore, &self.pool, common::T0)
            .await
            .expect("first reconcile");
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.pool,
            common::T0 + Duration::seconds(1),
        )
        .await
        .expect("second reconcile");
        WorkItemStore
            .list(&self.pool, None, 10, 0)
            .await
            .expect("list")
            .into_iter()
            .find(|item| item.source_path == name)
            .expect("ready item")
    }
}

#[tokio::test]
async fn ready_release_gets_one_durable_manifest() {
    let scenario = Scenario::new("inspection-success").await;
    let item = scenario.settle("unicode-Δ").await;
    assert_eq!(IngestionState::Ready, item.state);

    let stats = inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        common::T0 + Duration::seconds(2),
        None,
    )
    .await
    .expect("inspect");
    assert_eq!(1, stats.completed);
    let inspected = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(IngestionState::Normalizing, inspected.state);
    let manifest = WorkItemStore
        .get_manifest(&scenario.pool, item.id)
        .await
        .expect("manifest lookup")
        .expect("manifest");
    assert_eq!(1, manifest.schema_version);
    assert_eq!("unicode-Δ", manifest.payload["source_path"]);
    assert_eq!("wav", manifest.payload["files"][0]["audio"]["container"]);

    scenario.database.drop().await;
}

#[tokio::test]
async fn source_change_before_inspection_returns_to_settling_without_manifest() {
    let scenario = Scenario::new("inspection-source-change").await;
    let item = scenario.settle("changing").await;
    fs::write(scenario.root.join("changing").join("late.nfo"), b"late").expect("late file");

    let stats = inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        common::T0 + Duration::seconds(2),
        None,
    )
    .await
    .expect("inspect");
    assert_eq!(1, stats.source_changed);
    let item = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(IngestionState::Settling, item.state);
    assert!(
        WorkItemStore
            .get_manifest(&scenario.pool, item.id)
            .await
            .expect("manifest lookup")
            .is_none()
    );

    scenario.database.drop().await;
}

/// WP-11 stub gate: the scripting service implements this trait in the
/// server; here a stub proves the pipeline wiring (deny → return to
/// settling unprocessed, no manifest; allow → normal inspection).
struct StubGate(melodee_ingestion::DirectoryGateOutcome);

impl melodee_ingestion::DirectoryProcessingGate for StubGate {
    fn evaluate<'a>(
        &'a self,
        candidate_root: &'a std::path::Path,
        snapshot: &'a melodee_ingestion::CandidateSnapshot,
    ) -> melodee_ingestion::DirectoryGateFuture<'a> {
        let outcome = self.0.clone();
        // The gate must see the verified pre-inspection snapshot.
        assert!(!snapshot.files.is_empty());
        assert!(candidate_root.is_dir());
        Box::pin(async move { outcome })
    }
}

#[tokio::test]
async fn script_gate_deny_returns_candidate_to_settling_unprocessed() {
    let scenario = Scenario::new("inspection-script-deny").await;
    let item = scenario.settle("denied").await;
    assert_eq!(IngestionState::Ready, item.state);

    let gate = StubGate(melodee_ingestion::DirectoryGateOutcome::Skip {
        reason: "denied by test script".to_owned(),
    });
    let stats = inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        common::T0 + Duration::seconds(2),
        Some(&gate),
    )
    .await
    .expect("inspect");
    assert_eq!(1, stats.script_skipped);
    assert_eq!(0, stats.completed);

    let item = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(
        IngestionState::Settling,
        item.state,
        "skip leaves the directory unprocessed this pass"
    );
    assert!(
        WorkItemStore
            .get_manifest(&scenario.pool, item.id)
            .await
            .expect("manifest lookup")
            .is_none(),
        "a skipped candidate is never inspected"
    );

    // A later pass re-settles and re-promotes the candidate (the settling
    // quiet period is zero in this fixture), and an allowing gate then
    // processes it — skip is not a terminal parking.
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        common::T0 + Duration::seconds(3),
    )
    .await
    .expect("re-reconcile");
    let item = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(IngestionState::Ready, item.state);

    let gate = StubGate(melodee_ingestion::DirectoryGateOutcome::Allow);
    let stats = inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        common::T0 + Duration::seconds(4),
        Some(&gate),
    )
    .await
    .expect("inspect");
    assert_eq!(1, stats.completed);
    let item = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(IngestionState::Normalizing, item.state);

    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn source_change_during_inspection_discards_manifest() {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let mut scenario = Scenario::new("inspection-during-change").await;
    let item = scenario.settle("during").await;
    let ffprobe = Command::new("sh")
        .arg("-c")
        .arg("command -v ffprobe")
        .output()
        .expect("find ffprobe");
    let ffprobe_path = String::from_utf8(ffprobe.stdout)
        .expect("ffprobe path")
        .trim()
        .to_owned();
    let wrapper = scenario.root.join("slow-ffprobe.sh");
    fs::write(
        &wrapper,
        format!("#!/bin/sh\nsleep 1\nexec {ffprobe_path} \"$@\"\n"),
    )
    .expect("wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("permissions");
    scenario.config.ffprobe_path = wrapper;

    let config = scenario.config.clone();
    let pool = scenario.pool.clone();
    let inspection = tokio::spawn(async move {
        inspect_ready_once(
            &config,
            &WorkItemStore,
            &pool,
            &WorkEngineConfig::default(),
            common::T0 + Duration::seconds(2),
            None,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fs::write(scenario.root.join("during").join("late.nfo"), b"late").expect("late file");
    let stats = inspection
        .await
        .expect("inspection task")
        .expect("inspection");
    assert_eq!(1, stats.source_changed);
    let item = WorkItemStore
        .get(&scenario.pool, item.id)
        .await
        .expect("item");
    assert_eq!(IngestionState::Settling, item.state);
    assert!(
        WorkItemStore
            .get_manifest(&scenario.pool, item.id)
            .await
            .expect("manifest lookup")
            .is_none()
    );

    scenario.database.drop().await;
}

#[tokio::test]
async fn two_hundred_releases_reach_one_manifest_each() {
    let mut scenario = Scenario::new("inspection-200").await;
    scenario.config.inspection_concurrency = 16;
    for index in 0..200 {
        let release = scenario.root.join(format!("release-{index:03}"));
        fs::create_dir_all(&release).expect("release");
        write_tagless_wav(&release.join("track.wav"));
        if index % 4 == 0 {
            fs::write(release.join("notes.nfo"), b"fixture").expect("nfo");
        }
        if index % 5 == 0 {
            fs::write(release.join(".melodee-ready"), b"ready").expect("marker");
        }
    }
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, common::T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        common::T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");

    let mut completed = 0_u64;
    while completed < 200 {
        let stats = inspect_ready_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            common::T0 + Duration::seconds(2 + i64::try_from(completed).unwrap_or(0)),
            None,
        )
        .await
        .expect("inspect batch");
        if stats.claimed == 0 {
            break;
        }
        completed += stats.completed;
    }
    assert_eq!(200, completed);
    let (work_items, manifests, duplicate_work_item_ids): (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM release_work_items),
            (SELECT COUNT(*) FROM release_work_item_manifests),
            (SELECT COUNT(*) - COUNT(DISTINCT work_item_id) FROM release_work_item_manifests)",
    )
    .fetch_one(&scenario.pool)
    .await
    .expect("counts");
    assert_eq!(200, work_items);
    assert_eq!(200, manifests);
    assert_eq!(0, duplicate_work_item_ids);
    let no_change_started = std::time::Instant::now();
    let no_change = inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        common::T0 + Duration::seconds(999),
        None,
    )
    .await
    .expect("no-change inspection");
    eprintln!(
        "phase1c-200-no-change-ms={}",
        no_change_started.elapsed().as_millis()
    );
    assert_eq!(0, no_change.claimed);

    scenario.database.drop().await;
}

fn write_tagless_wav(path: &std::path::Path) {
    let sample_rate = 8_000_u32;
    let sample_count = 800_u32;
    let data_size = sample_count * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    bytes.extend(std::iter::repeat_n(0_u8, data_size as usize));
    fs::write(path, bytes).expect("wav");
}
