//! Integration tests for durable discovery and settling.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{collections::HashSet, fs, path::PathBuf};

use melodee_domain::{IngestionState, WorkEventType};
use melodee_ingestion::{
    InboundSource, IngestionConfig, candidate_relative_path, discover_release_candidates,
    reconcile_all_sources, reconcile_inbound_source, snapshot_release_directory,
    start_reconciliation_loop,
};
use melodee_persistence::WorkItemStore;
use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

const READY_MARKER: &str = ".melodee-ready";

struct IngestionScenario {
    root: PathBuf,
    source: InboundSource,
    config: IngestionConfig,
    pool: PgPool,
    db_name: String,
}

impl IngestionScenario {
    async fn new(name: &str, settling_quiet_seconds: i64, ready_marker_shortcut: i64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "melodee-ingestion-test-{}-{}",
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
            pool: db.pool,
            db_name: db.name,
        }
    }

    async fn create_release(&self, name: &str, file_count: usize, with_ready_marker: bool) {
        let release = self.root.join(name);
        fs::create_dir_all(&release).expect("create release");
        for index in 0..file_count {
            fs::write(
                release.join(format!("track-{index:02}.flac")),
                format!("audio-{index}"),
            )
            .expect("write track file");
        }
        if with_ready_marker {
            fs::write(release.join(READY_MARKER), b"ready").expect("write ready marker");
        }
    }

    async fn create_late_file(&self, release_name: &str, file: &str) {
        fs::write(self.root.join(release_name).join(file), b"late").expect("write late file");
    }

    async fn reconcile_at(&self, seconds: i64) {
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.pool,
            common::T0 + Duration::seconds(seconds),
        )
        .await
        .unwrap_or_else(|error| panic!("reconciliation failed: {error}"));
    }

    async fn work_item_count(&self) -> usize {
        WorkItemStore
            .list(&self.pool, None, 10_000, 0)
            .await
            .unwrap()
            .len()
    }

    async fn work_items(&self) -> Vec<melodee_domain::ReleaseWorkItem> {
        WorkItemStore
            .list(&self.pool, None, 10_000, 0)
            .await
            .unwrap()
    }

    async fn release_item(&self, source_path: &str) -> melodee_domain::ReleaseWorkItem {
        let path = candidate_relative_path(PathBuf::from(source_path).as_path());
        WorkItemStore
            .list(&self.pool, None, 10_000, 0)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.source_path == path)
            .unwrap_or_else(|| panic!("missing work item: {source_path}"))
    }

    fn db_url(&self) -> String {
        common::admin_url().replace("/postgres", &format!("/{}", self.db_name))
    }
}

#[tokio::test]
async fn discovery_creates_durable_work_item_and_snapshot() {
    let scenario = IngestionScenario::new("discovery-creates-work-item", 10, 0).await;
    scenario.create_release("alpha", 3, false).await;

    scenario.reconcile_at(0).await;

    assert_eq!(1, scenario.work_item_count().await);
    let item = scenario.release_item("alpha").await;
    assert_eq!(IngestionState::Settling, item.state);
    assert!(item.latest_settling_snapshot_id.is_some());

    let snapshot = WorkItemStore
        .latest_snapshot(&scenario.pool, item.id)
        .await
        .expect("latest snapshot")
        .expect("snapshot should exist");
    assert!(snapshot.file_count > 0);
}

#[tokio::test]
async fn reconcile_calls_are_idempotent() {
    let scenario = IngestionScenario::new("discovery-idempotent", 10, 0).await;
    scenario.create_release("alpha", 2, false).await;

    scenario.reconcile_at(0).await;
    scenario.reconcile_at(1).await;

    assert_eq!(1, scenario.work_item_count().await);
    let item = scenario.release_item("alpha").await;
    let count = scenario
        .work_items()
        .await
        .into_iter()
        .filter(|row| row.source_path == item.source_path)
        .count();
    assert_eq!(1, count);

    let expected_path = candidate_relative_path(PathBuf::from("alpha").as_path());
    assert_eq!(expected_path, item.source_path);
}

#[tokio::test]
async fn concurrent_reconcilers_do_not_create_duplicates() {
    let scenario = IngestionScenario::new("discovery-concurrent", 0, 0).await;
    scenario.create_release("alpha", 2, false).await;

    let config_one = scenario.config.clone();
    let config_two = scenario.config.clone();
    let pool_one = scenario.pool.clone();
    let pool_two = scenario.pool.clone();

    let first = tokio::spawn(async move {
        reconcile_all_sources(&config_one, &WorkItemStore, &pool_one, common::T0).await
    });

    let second = tokio::spawn(async move {
        reconcile_all_sources(&config_two, &WorkItemStore, &pool_two, common::T0).await
    });

    assert!(first.await.expect("one recon task").is_ok());
    assert!(second.await.expect("two recon task").is_ok());

    assert_eq!(1, scenario.work_item_count().await);
}

#[tokio::test]
async fn settling_requires_quiet_period_without_marker() {
    let scenario = IngestionScenario::new("settling-quiets", 6, 0).await;
    scenario.create_release("alpha", 1, false).await;

    scenario.reconcile_at(0).await;
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("alpha").await.state
    );

    scenario.reconcile_at(3).await;
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("alpha").await.state
    );

    scenario.reconcile_at(7).await;
    assert_eq!(
        IngestionState::Ready,
        scenario.release_item("alpha").await.state
    );
}

#[tokio::test]
async fn ready_marker_shortcuts_settling() {
    let scenario = IngestionScenario::new("ready-marker-shortcut", 30, 0).await;
    scenario.create_release("alpha", 2, true /* marker */).await;

    scenario.reconcile_at(0).await;
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("alpha").await.state
    );

    scenario.reconcile_at(1).await;
    assert_eq!(
        IngestionState::Ready,
        scenario.release_item("alpha").await.state
    );
}

#[tokio::test]
async fn changed_snapshot_reverts_ready_to_settling() {
    let scenario = IngestionScenario::new("ready-revert", 4, 0).await;
    scenario.create_release("alpha", 1, false).await;

    scenario.reconcile_at(0).await;
    scenario.reconcile_at(5).await;
    assert_eq!(
        IngestionState::Ready,
        scenario.release_item("alpha").await.state
    );

    scenario.create_late_file("alpha", "late.flac").await;
    scenario.reconcile_at(6).await;
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("alpha").await.state
    );

    scenario.reconcile_at(10).await;
    assert_eq!(
        IngestionState::Ready,
        scenario.release_item("alpha").await.state
    );
}

#[tokio::test]
async fn missing_candidate_is_not_deleted() {
    let scenario = IngestionScenario::new("missing-candidate", 5, 0).await;
    scenario.create_release("alpha", 1, false).await;

    scenario.reconcile_at(0).await;
    fs::remove_dir_all(scenario.root.join("alpha")).expect("remove release");

    scenario.reconcile_at(1).await;
    let item = scenario.release_item("alpha").await;
    assert_ne!(item.state, IngestionState::Deleted);

    fs::create_dir_all(scenario.root.join("alpha")).expect("recreate");
    fs::write(scenario.root.join("alpha").join("track-99.flac"), b"track").expect("write");
    scenario.reconcile_at(2).await;

    let item = scenario.release_item("alpha").await;
    assert_eq!(IngestionState::Settling, item.state);
}

#[tokio::test]
async fn lost_watcher_notification_is_recovered_by_scan() {
    let scenario = IngestionScenario::new("lost-watcher-event", 1, 0).await;
    scenario.create_release("alpha", 1, false).await;

    scenario.reconcile_at(0).await;
    scenario.create_release("beta", 1, false).await;

    scenario.reconcile_at(1).await;
    assert_eq!(2, scenario.work_item_count().await);
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("beta").await.state
    );
}

#[tokio::test]
async fn restart_preserves_settling_state() {
    let scenario = IngestionScenario::new("restart-settling", 8, 0).await;
    scenario.create_release("alpha", 1, false).await;

    scenario.reconcile_at(0).await;
    assert_eq!(
        IngestionState::Settling,
        scenario.release_item("alpha").await.state
    );

    let restart_pool = PgPool::connect(&scenario.db_url())
        .await
        .expect("reconnect after restart");

    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &restart_pool,
        common::T0 + Duration::seconds(9),
    )
    .await
    .expect("reconcile after restart");
    restart_pool.close().await;

    let item = WorkItemStore
        .list(&scenario.pool, None, 100, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|row| row.source_path == "alpha")
        .expect("item");
    assert_eq!(IngestionState::Ready, item.state);
}

#[tokio::test]
async fn concurrent_reconcilers_settle_to_ready_with_single_ready_transition() {
    let scenario = IngestionScenario::new("ready-race", 0, 0).await;
    scenario.create_release("alpha", 1, false).await;
    scenario.reconcile_at(0).await;

    let now = common::T0 + Duration::seconds(1);
    let config_one = scenario.config.clone();
    let config_two = scenario.config.clone();
    let pool_one = scenario.pool.clone();
    let pool_two = scenario.pool.clone();
    let source_one = scenario.source.clone();
    let source_two = scenario.source.clone();

    let one = tokio::spawn(async move {
        reconcile_inbound_source(&config_one, &WorkItemStore, &pool_one, &source_one, now).await
    });

    let two = tokio::spawn(async move {
        reconcile_inbound_source(&config_two, &WorkItemStore, &pool_two, &source_two, now).await
    });

    one.await.expect("one task").expect("reconcile one");
    two.await.expect("two task").expect("reconcile two");

    let item = scenario.release_item("alpha").await;
    assert_eq!(IngestionState::Ready, item.state);

    let ready_events = WorkItemStore
        .list_events(&scenario.pool, item.id)
        .await
        .expect("events")
        .into_iter()
        .filter(|event| {
            event.event_type == WorkEventType::Transitioned
                && event.to_state == Some(IngestionState::Ready)
        })
        .count();
    assert_eq!(1, ready_events);
}

#[tokio::test]
async fn two_hundred_releases_discover_and_settle_without_duplicates() {
    let mut scenario = IngestionScenario::new("two-hundred", 1, 0).await;
    scenario.config.reconciliation_interval = Duration::seconds(1);

    for index in 0..200 {
        scenario
            .create_release(
                &format!("release-{index:03}"),
                (index % 7) + 1,
                index % 10 == 0,
            )
            .await;
    }

    scenario.reconcile_at(0).await;
    scenario.reconcile_at(1).await;

    for index in 0..25 {
        scenario
            .create_late_file(&format!("release-{index:03}"), "late.flac")
            .await;
    }

    scenario.reconcile_at(2).await;
    scenario.reconcile_at(4).await;

    let restart_pool = PgPool::connect(&scenario.db_url())
        .await
        .expect("reconnect for restart");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &restart_pool,
        common::T0 + Duration::seconds(6),
    )
    .await
    .expect("reconcile after restart");
    restart_pool.close().await;

    let all_items = scenario.work_items().await;
    assert_eq!(200, all_items.len());

    let seen_paths: HashSet<String> = all_items
        .iter()
        .map(|item| item.source_path.clone())
        .collect();
    assert_eq!(200, seen_paths.len());

    let ready_count = all_items
        .iter()
        .filter(|item| item.state == IngestionState::Ready)
        .count();
    assert_eq!(200, ready_count);
}

#[tokio::test]
async fn snapshot_error_for_disappeared_directory_is_survivable() {
    let scenario = IngestionScenario::new("snapshot-miss", 2, 0).await;
    scenario.create_release("alpha", 3, false).await;
    scenario.reconcile_at(0).await;

    assert!(snapshot_release_directory(&scenario.root.join("alpha"), READY_MARKER).is_ok());

    fs::remove_dir_all(scenario.root.join("alpha")).expect("remove release");
    assert!(snapshot_release_directory(&scenario.root.join("alpha"), READY_MARKER).is_err());

    scenario.reconcile_at(1).await;
    scenario.reconcile_at(2).await;

    let item = scenario.release_item("alpha").await;
    assert_ne!(item.state, IngestionState::Deleted);
}

#[tokio::test]
async fn directory_discovery_ignores_non_directories_and_invalid_candidates() {
    let scenario = IngestionScenario::new("non-directory", 5, 0).await;

    fs::write(scenario.root.join("ignore.txt"), b"skip").expect("write");
    scenario.create_release("release", 1, false).await;

    let discovered = discover_release_candidates(&scenario.source).expect("discover candidates");
    assert_eq!(1, discovered.len());
    assert_eq!("release", discovered[0].relative_path.to_string_lossy());

    scenario.reconcile_at(0).await;
    assert_eq!(1, scenario.work_item_count().await);
}

#[tokio::test]
async fn symlink_candidates_are_safe() {
    let scenario = IngestionScenario::new("safe-symlink", 1, 0).await;
    let outside = std::env::temp_dir().join(format!(
        "melodee-ingest-outside-{}",
        Uuid::now_v7().simple()
    ));
    fs::create_dir_all(&outside).expect("outside root");
    fs::write(outside.join("release-outside.txt"), b"outside").expect("outside file");

    scenario.create_release("release-inside", 1, false).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("../", scenario.root.join("escape-dir")).unwrap_or_else(|_| {
            std::os::unix::fs::symlink("../outside", scenario.root.join("escape-dir"))
                .expect("symlink fallback")
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir;
        symlink_dir("..", scenario.root.join("escape-dir")).expect("symlink");
    }

    let discovered = discover_release_candidates(&scenario.source).expect("discover candidates");
    let names: HashSet<_> = discovered
        .into_iter()
        .map(|candidate| candidate.relative_path.to_string_lossy().to_string())
        .collect();
    assert!(names.contains("release-inside"));
    assert!(!names.iter().any(|name| name.contains("escape")));
}

#[tokio::test]
async fn scheduler_runs_periodic_reconciliation_with_restart_signal() {
    let mut scenario = IngestionScenario::new("scheduler", 0, 0).await;
    scenario.config.ready_marker_shortcut = Duration::ZERO;
    scenario.config.reconciliation_interval = Duration::seconds(1);
    scenario.create_release("alpha", 1, false).await;

    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.pool.clone(),
        WorkItemStore,
    );

    tokio::time::sleep(std::time::Duration::from_millis(3400)).await;

    handle.stop().await;

    assert_eq!(1, scenario.work_item_count().await);
    assert_eq!(
        IngestionState::Ready,
        scenario.release_item("alpha").await.state
    );
}
