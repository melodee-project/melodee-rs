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
    database: common::TestDb,
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
            database: db,
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
            &self.database.pool,
            common::T0 + Duration::seconds(seconds),
        )
        .await
        .unwrap_or_else(|error| panic!("reconciliation failed: {error}"));
    }

    async fn work_item_count(&self) -> usize {
        WorkItemStore
            .list(&self.database.pool, None, 10_000, 0)
            .await
            .unwrap()
            .len()
    }

    async fn work_items(&self) -> Vec<melodee_domain::ReleaseWorkItem> {
        WorkItemStore
            .list(&self.database.pool, None, 10_000, 0)
            .await
            .unwrap()
    }

    async fn release_item(&self, source_path: &str) -> melodee_domain::ReleaseWorkItem {
        let path = candidate_relative_path(PathBuf::from(source_path).as_path());
        WorkItemStore
            .list(&self.database.pool, None, 10_000, 0)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.source_path == path)
            .unwrap_or_else(|| panic!("missing work item: {source_path}"))
    }

    fn db_url(&self) -> String {
        common::database_url(&self.database.name)
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
        .latest_snapshot(&scenario.database.pool, item.id)
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
    let pool_one = scenario.database.pool.clone();
    let pool_two = scenario.database.pool.clone();

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
        .list(&scenario.database.pool, None, 100, 0)
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
    let pool_one = scenario.database.pool.clone();
    let pool_two = scenario.database.pool.clone();
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
        .list_events(&scenario.database.pool, item.id)
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
        scenario.database.pool.clone(),
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

#[tokio::test]
async fn scheduler_processes_every_source_but_retains_a_bounded_snapshot() {
    let scenario = IngestionScenario::new("scheduler-many-sources", 0, 0).await;
    let mut sources = Vec::new();
    for index in 0..70 {
        let source_root = scenario.root.join(format!("source-{index:03}"));
        let release = source_root.join(format!("release-{index:03}"));
        fs::create_dir_all(&release).expect("create source release");
        fs::write(release.join("track.flac"), b"fixture").expect("write candidate");
        sources.push(
            InboundSource::from_root(
                source_root,
                Some(Uuid::now_v7()),
                Some(format!("source-{index:03}")),
            )
            .expect("create source"),
        );
    }
    let mut config = IngestionConfig::with_sources(sources);
    config.reconciliation_interval = Duration::seconds(60);
    let handle = start_reconciliation_loop(
        config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let active_scans: i64 =
                sqlx::query_scalar("SELECT count(*) FROM ingestion_discovery_scan")
                    .fetch_one(&scenario.database.pool)
                    .await
                    .expect("scan count");
            if scenario.work_item_count().await == 70 && active_scans == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("every source was processed");
    // Allow the completed pass to publish its bounded result snapshot.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let retained = tokio::time::timeout(std::time::Duration::from_secs(30), handle.stop())
        .await
        .expect("scheduler stopped");
    assert_eq!(retained.len(), 64);
    assert_eq!(
        retained.first().map(|item| item.source.as_str()),
        Some("source-006")
    );
    assert_eq!(
        retained.last().map(|item| item.source.as_str()),
        Some("source-069")
    );
    let all = reconcile_all_sources(&config, &WorkItemStore, &scenario.database.pool, common::T0)
        .await
        .expect("public one-shot reconciliation");
    assert_eq!(
        all.len(),
        70,
        "the public one-shot operation still returns every source"
    );
    fs::remove_dir_all(&scenario.root).expect("remove owned source root");
    scenario.database.drop().await;
}

#[tokio::test]
async fn partitioned_scan_discovers_more_than_one_batch_without_spooling() {
    let scenario = IngestionScenario::new("scan_batches", 30, 0).await;
    for number in (0..260).rev() {
        scenario
            .create_release(&format!("release-{number:03}"), 0, false)
            .await;
    }
    scenario.reconcile_at(0).await;
    assert_eq!(scenario.work_item_count().await, 260);
    scenario.reconcile_at(1).await;
    assert_eq!(scenario.work_item_count().await, 260);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM ingestion_discovery_scan")
        .fetch_one(&scenario.database.pool)
        .await
        .expect("completed scan cleanup");
    assert_eq!(remaining, 0);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn expired_discovery_owner_cannot_mutate_and_next_scan_recovers() {
    let scenario = IngestionScenario::new("scan_expired", 30, 0).await;
    scenario.create_release("present", 0, false).await;
    scenario.reconcile_at(0).await;
    let item = scenario.release_item("present").await;
    let old = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('default', $1, $1, clock_timestamp() - interval '1 second', clock_timestamp())")
        .bind(old).execute(&scenario.database.pool).await.expect("expired lease");
    let stale = melodee_persistence::DiscoveryScanFence {
        scope_key: "default".to_owned(),
        generation: old,
        owner: old,
    };
    let error = WorkItemStore
        .upsert_settling_progress_fenced(
            &scenario.database.pool,
            item.id,
            None,
            None,
            common::T0,
            Some("source-missing"),
            Some(&stale),
        )
        .await
        .expect_err("expired owner must be fenced");
    assert!(matches!(
        error,
        melodee_persistence::WorkStoreError::DiscoveryScanLeaseExpired
    ));
    scenario.reconcile_at(1).await;
    assert_ne!(
        scenario
            .release_item("present")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn missing_detection_is_confined_to_its_source_library() {
    let scenario = IngestionScenario::new("scan_scope", 30, 0).await;
    scenario.create_release("same-name", 0, false).await;
    scenario.reconcile_at(0).await;
    let other_root = scenario.root.join("other-root");
    fs::create_dir_all(other_root.join("same-name")).expect("other release");
    let other = InboundSource::from_root(
        other_root.clone(),
        Some(Uuid::now_v7()),
        Some("other".to_owned()),
    )
    .expect("other source");
    reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &other,
        common::T0,
    )
    .await
    .expect("other scan");
    fs::remove_dir_all(scenario.root.join("same-name")).expect("remove first release");
    reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0 + Duration::seconds(1),
    )
    .await
    .expect("first scan");
    let states: Vec<(Option<Uuid>, Option<String>)> = sqlx::query_as("SELECT source_library_id, state_reason FROM release_work_items WHERE source_path = 'same-name' ORDER BY source_library_id NULLS FIRST")
        .fetch_all(&scenario.database.pool).await.expect("source states");
    assert_eq!(states.len(), 2);
    assert_eq!(states[0].1.as_deref(), Some("source-missing"));
    assert_ne!(states[1].1.as_deref(), Some("source-missing"));
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn cancelled_waiting_scan_does_not_mark_missing_and_recovers_after_expiry() {
    let scenario = IngestionScenario::new("scan_cancel_wait", 30, 0).await;
    scenario.create_release("still-present", 0, false).await;
    scenario.reconcile_at(0).await;
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('default', $1, $1, clock_timestamp() + interval '5 minutes', clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("active owner");
    let pool = scenario.database.pool.clone();
    let source = scenario.source.clone();
    let config = scenario.config.clone();
    let task = tokio::spawn(async move {
        reconcile_inbound_source(
            &config,
            &WorkItemStore,
            &pool,
            &source,
            common::T0 + Duration::seconds(1),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !task.is_finished(),
        "overlap must wait for the current owner"
    );
    task.abort();
    let _ = task.await;
    assert_ne!(
        scenario
            .release_item("still-present")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE generation = $1")
        .bind(generation).execute(&scenario.database.pool).await.expect("expire owner");
    scenario.reconcile_at(2).await;
    assert_ne!(
        scenario
            .release_item("still-present")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn disabled_ingestion_still_cleans_expired_scan_spool() {
    let scenario = IngestionScenario::new("disabled_scan_cleanup", 30, 0).await;
    let generation = Uuid::now_v7();
    let discovered = melodee_domain::ReleaseWorkItem::discovered(
        "removed-release".to_owned(),
        None,
        Uuid::now_v7(),
        common::T0,
    );
    let (item, _) = WorkItemStore
        .create_or_get(&scenario.database.pool, &discovered)
        .await
        .expect("work item");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('removed-source', $1, $1, clock_timestamp() - interval '1 minute', clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("expired removed source");
    let capture: i64 = sqlx::query_scalar("INSERT INTO release_work_item_settling_snapshots (work_item_id, observed_at, signature, file_count, total_bytes, ready_marker_seen, bypassed_settling, capture_complete, capture_scope, capture_generation, capture_owner, capture_source_path, capture_source_state) VALUES ($1,clock_timestamp(),'',0,0,false,false,false,'removed-source',$2,$2,'removed-release','discovered') RETURNING id")
        .bind(item.id).bind(generation).fetch_one(&scenario.database.pool).await.expect("unfinished capture");
    sqlx::query("INSERT INTO release_work_item_settling_snapshot_files (snapshot_id,relative_path,file_type,file_size,modified_at,modified_at_nanos) SELECT $1, n::text,'file',0,clock_timestamp(),'0' FROM generate_series(1, 257) AS n")
        .bind(capture).execute(&scenario.database.pool).await.expect("unfinished files");
    let hash_capture = Uuid::now_v7();
    sqlx::query("INSERT INTO custody_hash_spool_captures(id,work_item_id,lease_owner,purpose,source_generation,target_binding,state,observation_epoch,created_at,updated_at) VALUES($1,$2,'expired-owner','source','unix:1:2','removed-release','capturing',$1,clock_timestamp(),clock_timestamp())")
        .bind(hash_capture).bind(item.id).execute(&scenario.database.pool).await.expect("unfinished hash capture");
    sqlx::query("INSERT INTO custody_hash_spool_entries(capture_id,path_key,parent_key,name_key,legacy_path_hex,is_directory,legacy_length,digest,observed_epoch) SELECT $1,n::text,'',lpad(to_hex(n),16,'0'),'6100',false,0,repeat('a',64),$1 FROM generate_series(1,257) n")
        .bind(hash_capture).execute(&scenario.database.pool).await.expect("unfinished hash entries");
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'removed-release','old',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(item.id).bind(format!("v3:{}", "a".repeat(64))).execute(&scenario.database.pool).await.expect("retained obligation");
    let worker = melodee_persistence::WorkerId::new();
    let lease = WorkItemStore
        .claim_due_retained_sources(&scenario.database.pool, &worker, 1, Duration::minutes(1))
        .await
        .expect("claim retained")
        .remove(0);
    let checkpoint = WorkItemStore
        .begin_or_adopt_retained_chunk_checkpoint(
            &scenario.database.pool,
            &lease,
            "file",
            "root",
            "candidate",
            1,
        )
        .await
        .expect("checkpoint");
    sqlx::query("UPDATE inbound_retained_sources SET lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL WHERE work_item_id=$1")
        .bind(item.id).execute(&scenario.database.pool).await.expect("abandon retained lease");
    sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET updated_at=clock_timestamp()-interval '2 hours' WHERE work_item_id=$1")
        .bind(item.id).execute(&scenario.database.pool).await.expect("age checkpoint");
    let baseline = Uuid::now_v7();
    sqlx::query("INSERT INTO inbound_retained_baselines(id,work_item_id,lease_owner,stage_capture_id,stage_epoch,stage_digest,stage_generation,stage_binding,state,logical_bytes,created_at,updated_at) VALUES($1,$2,'expired-owner',$3,$3,$4,'old','stage','capturing',512,clock_timestamp()-interval '2 hours',clock_timestamp()-interval '2 hours')")
        .bind(baseline).bind(item.id).bind(generation).bind(format!("v3:{}", "a".repeat(64))).execute(&scenario.database.pool).await.expect("provisional baseline");
    sqlx::query("UPDATE inbound_retained_baseline_budget SET captures=1,bytes=512 WHERE singleton")
        .execute(&scenario.database.pool)
        .await
        .expect("baseline accounting");
    let mut config = IngestionConfig::with_sources(Vec::new());
    config.reconciliation_interval = Duration::seconds(1);
    let handle = start_reconciliation_loop(
        config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM ingestion_discovery_scan WHERE generation = $1",
            )
            .bind(generation)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("candidate count");
            let hashes: i64 =
                sqlx::query_scalar("SELECT count(*) FROM custody_hash_spool_captures WHERE id=$1")
                    .bind(hash_capture)
                    .fetch_one(&scenario.database.pool)
                    .await
                    .expect("hash captures");
            let chunks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM inbound_retained_chunk_checkpoints WHERE capture_id=$1",
            )
            .bind(checkpoint.capture_id())
            .fetch_one(&scenario.database.pool)
            .await
            .expect("chunk checkpoints");
            let baselines: i64 =
                sqlx::query_scalar("SELECT count(*) FROM inbound_retained_baselines WHERE id=$1")
                    .bind(baseline)
                    .fetch_one(&scenario.database.pool)
                    .await
                    .expect("baselines");
            if count == 0 && hashes == 0 && chunks == 0 && baselines == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("disabled scheduler cleanup");
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_work_item_settling_snapshots WHERE id=$1")
            .bind(capture)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("capture count");
    assert_eq!(
        remaining, 0,
        "scan row cannot be removed ahead of capture cleanup"
    );
    handle.stop().await;
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn successful_scan_discard_is_reclaimed_after_scan_row_is_removed() {
    let scenario = IngestionScenario::new("completed_scan_discard_gc", 30, 0).await;
    scenario.create_release("release", 257, false).await;
    scenario.reconcile_at(0).await;
    scenario.reconcile_at(1).await;
    let item = scenario.release_item("release").await;
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_item_settling_snapshots WHERE work_item_id=$1 AND capture_discarding AND NOT capture_complete")
        .bind(item.id).fetch_one(&scenario.database.pool).await.expect("queued capture");
    assert_eq!(queued, 1);
    let scan_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ingestion_discovery_scan WHERE scope_key='default'",
    )
    .fetch_one(&scenario.database.pool)
    .await
    .expect("scan count");
    assert_eq!(scan_count, 0, "successful discovery retires its scan");
    let config = IngestionConfig::with_sources(Vec::new());
    let handle = start_reconciliation_loop(config, scenario.database.pool.clone(), WorkItemStore);
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_item_settling_snapshots WHERE work_item_id=$1 AND NOT capture_complete")
                .bind(item.id).fetch_one(&scenario.database.pool).await.expect("capture count");
            if count == 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }).await.expect("independent cleanup");
    handle.stop().await;
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn scheduler_stop_interrupts_wait_for_another_scan_owner() {
    let mut scenario = IngestionScenario::new("stop_busy_scan", 30, 0).await;
    scenario.config.reconciliation_interval = Duration::seconds(60);
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('default', $1, $1, clock_timestamp() + interval '5 minutes', clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("other owner");
    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), handle.stop())
        .await
        .expect("stop interrupts busy lease wait");
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ingestion_discovery_scan WHERE generation = $1)",
    )
    .bind(generation)
    .fetch_one(&scenario.database.pool)
    .await
    .expect("peer lease");
    assert!(retained, "shutdown must not delete a live peer lease");
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn scheduler_stop_reaps_stalled_snapshot_without_publishing() {
    let mut scenario = IngestionScenario::new("stop_stalled_snapshot", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    let hook = melodee_ingestion::SnapshotStallTestHook::default();
    scenario.config.snapshot_stall_test_hook = Some(hook.clone());
    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !hook.started() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("snapshot producer started");
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(2), handle.stop()).await;
    hook.release();
    stopped.expect("scheduler stop remains responsive");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !hook.finished() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("producer returns after filesystem resumes");
    let published: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM release_work_item_settling_snapshots WHERE capture_complete",
    )
    .fetch_one(&scenario.database.pool)
    .await
    .expect("published snapshot count");
    assert_eq!(published, 0, "cancelled producer must not publish");
    let item = scenario.release_item("release").await;
    assert_ne!(item.state, IngestionState::Ready);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn one_shot_scan_returns_busy_when_peer_keeps_scope() {
    let scenario = IngestionScenario::new("one_shot_busy", 30, 0).await;
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('default', $1, $1, clock_timestamp() + interval '5 minutes', clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("peer owner");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(35),
        reconcile_inbound_source(
            &scenario.config,
            &WorkItemStore,
            &scenario.database.pool,
            &scenario.source,
            common::T0,
        ),
    )
    .await
    .expect("one-shot must be bounded");
    assert!(matches!(
        result,
        Err(melodee_ingestion::IngestionError::Persistence(
            melodee_persistence::WorkStoreError::DiscoveryScanBusy(_)
        ))
    ));
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn scheduler_skips_busy_source_and_processes_later_source() {
    let mut scenario = IngestionScenario::new("busy_source_fairness", 30, 0).await;
    let other_root = scenario.root.join("second-source");
    fs::create_dir_all(other_root.join("release")).expect("other release");
    let other =
        InboundSource::from_root(other_root, Some(Uuid::now_v7()), Some("second".to_owned()))
            .expect("second source");
    scenario.config.sources.push(other.clone());
    scenario.config.reconciliation_interval = Duration::seconds(60);
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ('default', $1, $1, clock_timestamp() + interval '5 minutes', clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("busy first source");
    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let item = WorkItemStore
                .get_by_source_identity(&scenario.database.pool, other.id, "release")
                .await
                .expect("work item query");
            if item.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("later source was not starved");
    handle.stop().await;
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn scan_preserves_canonical_nested_target_seen_through_aliases() {
    use std::os::unix::fs::symlink;

    let scenario = IngestionScenario::new("nested_alias", 30, 0).await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("nested target");
    symlink("holder/release", scenario.root.join("alias-a")).expect("first alias");
    symlink("holder/release", scenario.root.join("alias-b")).expect("second alias");
    scenario.reconcile_at(0).await;
    let item = scenario.release_item("holder/release").await;
    fs::remove_file(scenario.root.join("alias-a")).expect("remove one alias");
    scenario.reconcile_at(1).await;
    let after = scenario.release_item("holder/release").await;
    assert_eq!(after.id, item.id);
    assert_ne!(after.state_reason.as_deref(), Some("source-missing"));
    fs::remove_file(scenario.root.join("alias-b")).expect("remove last alias");
    scenario.reconcile_at(2).await;
    assert_eq!(
        scenario
            .release_item("holder/release")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn legacy_literal_snapshot_remains_stable_after_signature_upgrade() {
    let scenario = IngestionScenario::new("legacy_snapshot_signature", 0, 0).await;
    scenario.create_release("release", 1, false).await;
    scenario.reconcile_at(0).await;
    let before = scenario.release_item("release").await;
    let snapshot = snapshot_release_directory(&scenario.root.join("release"), READY_MARKER)
        .expect("current snapshot");
    let legacy: String = snapshot
        .files
        .iter()
        .map(|file| {
            format!(
                "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                file.relative_path,
                file.file_type,
                file.file_size,
                file.modified_at.unix_timestamp_nanos()
            )
        })
        .collect();
    sqlx::query("UPDATE release_work_item_settling_snapshots SET signature = $2 WHERE id = $1")
        .bind(before.latest_settling_snapshot_id)
        .bind(legacy)
        .execute(&scenario.database.pool)
        .await
        .expect("legacy signature");
    let result = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0 + Duration::seconds(1),
    )
    .await
    .expect("reconcile legacy snapshot");
    assert_eq!(result.stats.snapshot_changed, 0);
    let after = scenario.release_item("release").await;
    assert_eq!(
        after.latest_settling_snapshot_id,
        before.latest_settling_snapshot_id
    );
    assert_eq!(after.state, IngestionState::Ready);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn resumed_partition_keeps_observation_from_an_earlier_alias_partition() {
    use std::os::unix::fs::symlink;
    let scenario = IngestionScenario::new("partition_alias_resume", 30, 0).await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("target");
    let aliases: Vec<(String, u8)> = (0..100)
        .map(|index| {
            let name = format!("alias-{index:03}");
            let partition = blake3::hash(name.as_bytes()).as_bytes()[0] & 15;
            (name, partition)
        })
        .collect();
    let (first, earlier) = aliases
        .iter()
        .find(|(_, bucket)| *bucket == 1)
        .expect("earlier alias");
    let (second, current) = aliases
        .iter()
        .find(|(_, bucket)| *bucket == 9)
        .expect("current alias");
    symlink("holder/release", scenario.root.join(first)).expect("first alias");
    symlink("holder/release", scenario.root.join(second)).expect("second alias");
    scenario.reconcile_at(0).await;
    let item = scenario.release_item("holder/release").await;
    let generation = Uuid::now_v7();
    let mask = (1_i32 << earlier) | (1_i32 << current);
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2, discovery_seen_mask=$3, discovery_done_mask=$3 WHERE id=$1")
        .bind(item.id).bind(generation).bind(mask).execute(&scenario.database.pool).await.expect("partial observation");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,phase_started) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),$2,'collect',true)")
        .bind(generation).bind(i16::from(*current)).execute(&scenario.database.pool).await.expect("interrupted partition");
    fs::remove_file(scenario.root.join(second)).expect("remove unfinished alias");
    scenario.reconcile_at(1).await;
    assert_ne!(
        scenario
            .release_item("holder/release")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn resumed_partition_discards_observation_of_removed_alias() {
    use std::os::unix::fs::symlink;
    let scenario = IngestionScenario::new("partition_removed_alias", 30, 0).await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("target");
    let alias = "alias";
    let partition = blake3::hash(alias.as_bytes()).as_bytes()[0] & 15;
    symlink("holder/release", scenario.root.join(alias)).expect("alias");
    scenario.reconcile_at(0).await;
    let item = scenario.release_item("holder/release").await;
    let generation = Uuid::now_v7();
    let bit = 1_i32 << partition;
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2, discovery_seen_mask=$3, discovery_done_mask=$3 WHERE id=$1")
        .bind(item.id).bind(generation).bind(bit).execute(&scenario.database.pool).await.expect("partial observation");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,phase_started) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),$2,'collect',true)")
        .bind(generation).bind(i16::from(partition)).execute(&scenario.database.pool).await.expect("interrupted partition");
    fs::remove_file(scenario.root.join(alias)).expect("remove alias");
    scenario.reconcile_at(1).await;
    assert_eq!(
        scenario
            .release_item("holder/release")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn two_aliases_in_one_partition_keep_normal_pass_settling_behavior() {
    use std::os::unix::fs::symlink;
    let scenario = IngestionScenario::new("same_partition_alias", 0, 0).await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("target");
    let aliases: Vec<String> = (0..200)
        .map(|index| format!("alias-{index:03}"))
        .filter(|name| blake3::hash(name.as_bytes()).as_bytes()[0] & 15 == 3)
        .take(2)
        .collect();
    assert_eq!(aliases.len(), 2);
    for alias in &aliases {
        symlink("holder/release", scenario.root.join(alias)).expect("alias");
    }
    scenario.reconcile_at(0).await;
    assert_eq!(
        scenario.release_item("holder/release").await.state,
        IngestionState::Ready
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn failed_candidate_is_retried_after_interrupted_partition() {
    let scenario = IngestionScenario::new("partition_failure_retry", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    let partition = blake3::hash(b"release").as_bytes()[0] & 15;
    let discovered = melodee_domain::ReleaseWorkItem::discovered(
        "release".to_owned(),
        None,
        Uuid::now_v7(),
        common::T0,
    );
    let (item, _) = WorkItemStore
        .create_or_get(&scenario.database.pool, &discovered)
        .await
        .expect("work item");
    sqlx::query("ALTER TABLE release_work_item_settling_snapshots RENAME TO test_temporarily_unavailable_snapshots")
        .execute(&scenario.database.pool).await.expect("temporarily fail snapshot lookup");
    sqlx::query("CREATE TABLE test_interrupt_partition_target (partition smallint NOT NULL)")
        .execute(&scenario.database.pool)
        .await
        .expect("interrupt target table");
    sqlx::query("INSERT INTO test_interrupt_partition_target VALUES ($1)")
        .bind(i16::from(partition))
        .execute(&scenario.database.pool)
        .await
        .expect("interrupt target");
    sqlx::query("CREATE FUNCTION test_interrupt_partition() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.partition = (SELECT partition FROM test_interrupt_partition_target) AND NEW.partition > OLD.partition THEN RAISE EXCEPTION 'test interruption'; END IF; RETURN NEW; END $$")
        .execute(&scenario.database.pool).await.expect("interrupt function");
    sqlx::query("CREATE TRIGGER test_interrupt_partition BEFORE UPDATE ON ingestion_discovery_scan FOR EACH ROW EXECUTE FUNCTION test_interrupt_partition()")
        .execute(&scenario.database.pool).await.expect("interrupt trigger");
    let failed = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0,
    )
    .await;
    assert!(failed.is_err(), "partition checkpoint must be interrupted");
    let done: i32 =
        sqlx::query_scalar("SELECT discovery_done_mask FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("done mask");
    assert_eq!(
        done & (1_i32 << partition),
        0,
        "failed candidate must not be acknowledged"
    );
    sqlx::query("ALTER TABLE test_temporarily_unavailable_snapshots RENAME TO release_work_item_settling_snapshots")
        .execute(&scenario.database.pool).await.expect("restore snapshot table");
    sqlx::query("DROP TRIGGER test_interrupt_partition ON ingestion_discovery_scan")
        .execute(&scenario.database.pool)
        .await
        .expect("remove trigger");
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key='default'")
        .execute(&scenario.database.pool).await.expect("expire owner");
    scenario.reconcile_at(1).await;
    let retried = scenario.release_item("release").await;
    assert!(
        retried.latest_settling_snapshot_id.is_some(),
        "candidate was retried"
    );
    assert_eq!(retried.state, IngestionState::Settling);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn transient_candidate_failure_keeps_partition_pending_without_external_interruption() {
    let scenario = IngestionScenario::new("partition_transient_failure", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    let discovered = melodee_domain::ReleaseWorkItem::discovered(
        "release".to_owned(),
        None,
        Uuid::now_v7(),
        common::T0,
    );
    let (item, _) = WorkItemStore
        .create_or_get(&scenario.database.pool, &discovered)
        .await
        .expect("work item");
    sqlx::query(
        "ALTER TABLE release_work_item_settling_snapshots RENAME TO test_unavailable_snapshots",
    )
    .execute(&scenario.database.pool)
    .await
    .expect("temporarily fail snapshot lookup");
    let result = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0,
    )
    .await;
    assert!(matches!(
        result,
        Err(melodee_ingestion::IngestionError::Persistence(
            melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(_)
        ))
    ));
    let partition = blake3::hash(b"release").as_bytes()[0] & 15;
    let done: i32 =
        sqlx::query_scalar("SELECT discovery_done_mask FROM release_work_items WHERE id=$1")
            .bind(item.id)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("done mask");
    assert_eq!(done & (1_i32 << partition), 0);
    sqlx::query(
        "ALTER TABLE test_unavailable_snapshots RENAME TO release_work_item_settling_snapshots",
    )
    .execute(&scenario.database.pool)
    .await
    .expect("restore snapshot table");
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key='default'")
        .execute(&scenario.database.pool).await.expect("end retry backoff");
    scenario.reconcile_at(1).await;
    assert!(
        scenario
            .release_item("release")
            .await
            .latest_settling_snapshot_id
            .is_some()
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn failed_work_item_creation_before_marker_is_retried() {
    let scenario = IngestionScenario::new("partition_create_retry", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    sqlx::query("CREATE FUNCTION test_reject_work_item() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test create failure'; END $$")
        .execute(&scenario.database.pool).await.expect("failure function");
    sqlx::query("CREATE TRIGGER test_reject_work_item BEFORE INSERT ON release_work_items FOR EACH ROW EXECUTE FUNCTION test_reject_work_item()")
        .execute(&scenario.database.pool).await.expect("failure trigger");
    let result = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0,
    )
    .await;
    assert!(matches!(
        result,
        Err(melodee_ingestion::IngestionError::Persistence(
            melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(_)
        ))
    ));
    assert_eq!(scenario.work_item_count().await, 0);
    sqlx::query("DROP TRIGGER test_reject_work_item ON release_work_items")
        .execute(&scenario.database.pool)
        .await
        .expect("remove failure trigger");
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key='default'")
        .execute(&scenario.database.pool).await.expect("end retry backoff");
    scenario.reconcile_at(1).await;
    assert!(
        scenario
            .release_item("release")
            .await
            .latest_settling_snapshot_id
            .is_some()
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn persistent_failure_does_not_starve_later_partitions() {
    let scenario = IngestionScenario::new("partition_failure_fairness", 30, 0).await;
    let names: Vec<(String, u8)> = (0..200)
        .map(|index| {
            let name = format!("release-{index:03}");
            let partition = blake3::hash(name.as_bytes()).as_bytes()[0] & 15;
            (name, partition)
        })
        .collect();
    let (failed, _) = names
        .iter()
        .find(|(_, bucket)| *bucket == 0)
        .expect("first partition");
    let (healthy, _) = names
        .iter()
        .find(|(_, bucket)| *bucket == 15)
        .expect("last partition");
    scenario.create_release(failed, 1, false).await;
    scenario.create_release(healthy, 1, false).await;
    sqlx::query("CREATE TABLE test_failed_source (source_path text NOT NULL)")
        .execute(&scenario.database.pool)
        .await
        .expect("failure target table");
    sqlx::query("INSERT INTO test_failed_source VALUES ($1)")
        .bind(failed)
        .execute(&scenario.database.pool)
        .await
        .expect("failure target");
    sqlx::query("CREATE FUNCTION test_fail_one_source() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.source_path = (SELECT source_path FROM test_failed_source) THEN RAISE EXCEPTION 'injected persistent failure'; END IF; RETURN NEW; END $$")
        .execute(&scenario.database.pool).await.expect("failure function");
    sqlx::query("CREATE TRIGGER test_fail_one_source BEFORE INSERT ON release_work_items FOR EACH ROW EXECUTE FUNCTION test_fail_one_source()")
        .execute(&scenario.database.pool).await.expect("failure trigger");
    let result = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0,
    )
    .await;
    assert!(matches!(
        result,
        Err(melodee_ingestion::IngestionError::Persistence(
            melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(_)
        ))
    ));
    assert!(
        scenario
            .release_item(healthy)
            .await
            .latest_settling_snapshot_id
            .is_some(),
        "later partition must make progress"
    );
    sqlx::query("DROP TRIGGER test_fail_one_source ON release_work_items")
        .execute(&scenario.database.pool)
        .await
        .expect("remove failure trigger");
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key='default'")
        .execute(&scenario.database.pool).await.expect("end retry backoff");
    scenario.reconcile_at(1).await;
    assert!(
        scenario
            .release_item(failed)
            .await
            .latest_settling_snapshot_id
            .is_some()
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn replay_revalidates_a_completed_candidate_that_changed() {
    let scenario = IngestionScenario::new("partition_revalidate", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    scenario.reconcile_at(0).await;
    let before = scenario.release_item("release").await;
    let partition = blake3::hash(b"release").as_bytes()[0] & 15;
    let generation = Uuid::now_v7();
    let bit = 1_i32 << partition;
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2, discovery_seen_mask=$3, discovery_done_mask=$3 WHERE id=$1")
        .bind(before.id).bind(generation).bind(bit).execute(&scenario.database.pool).await.expect("previous processing");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,phase_started) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),$2,'collect',true)")
        .bind(generation).bind(i16::from(partition)).execute(&scenario.database.pool).await.expect("interrupted scan");
    fs::write(
        scenario.root.join("release/track-00.flac"),
        b"changed-size-audio",
    )
    .expect("source change");
    scenario.reconcile_at(1).await;
    let after = scenario.release_item("release").await;
    assert_ne!(
        after.latest_settling_snapshot_id, before.latest_settling_snapshot_id,
        "replay must capture the changed source"
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn scheduler_continues_later_source_after_incomplete_scan() {
    let mut scenario = IngestionScenario::new("incomplete_source_fairness", 30, 0).await;
    scenario.create_release("bad", 1, false).await;
    let other_root = scenario.root.join("second-source");
    fs::create_dir_all(other_root.join("healthy")).expect("other release");
    let other =
        InboundSource::from_root(other_root, Some(Uuid::now_v7()), Some("second".to_owned()))
            .expect("second source");
    scenario.config.sources.push(other.clone());
    scenario.config.reconciliation_interval = Duration::seconds(60);
    sqlx::query("CREATE FUNCTION test_fail_first_source() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.source_library_id IS NULL AND NEW.source_path='bad' THEN RAISE EXCEPTION 'injected failure'; END IF; RETURN NEW; END $$")
        .execute(&scenario.database.pool).await.expect("failure function");
    sqlx::query("CREATE TRIGGER test_fail_first_source BEFORE INSERT ON release_work_items FOR EACH ROW EXECUTE FUNCTION test_fail_first_source()")
        .execute(&scenario.database.pool).await.expect("failure trigger");
    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let item = WorkItemStore
                .get_by_source_identity(&scenario.database.pool, other.id, "healthy")
                .await
                .expect("work item query");
            if item.is_some_and(|item| item.latest_settling_snapshot_id.is_some()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("later source must make progress");
    handle.stop().await;
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ingestion_discovery_scan WHERE scope_key='default')",
    )
    .fetch_one(&scenario.database.pool)
    .await
    .expect("unfinished source");
    assert!(pending, "failed source must remain durably pending");
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn recovery_reset_yields_and_resumes_without_stale_observations() {
    let scenario = IngestionScenario::new("bounded_recovery_reset", 30, 0).await;
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO release_work_items (id,api_key,source_path,current_path,state,correlation_id,discovered_at,updated_at,created_at,discovery_scan_generation,discovery_seen_mask) SELECT gen_random_uuid(),gen_random_uuid(),'missing-' || n,'missing-' || n,'discovered',gen_random_uuid(),clock_timestamp(),clock_timestamp(),clock_timestamp(),$1,65535 FROM generate_series(0,512) AS n")
        .bind(generation).execute(&scenario.database.pool).await.expect("prior observations");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff) VALUES ('default',$1,$1,clock_timestamp()-interval '1 minute',clock_timestamp())")
        .bind(generation).execute(&scenario.database.pool).await.expect("expired owner");
    let later_root = scenario.root.join("later-source");
    fs::create_dir_all(later_root.join("release")).expect("later release");
    fs::write(later_root.join("release/track.flac"), b"audio").expect("later file");
    let later =
        InboundSource::from_root(later_root, Some(Uuid::now_v7()), Some("later".to_owned()))
            .expect("later source");
    let mut config = IngestionConfig::with_sources(vec![scenario.source.clone(), later.clone()]);
    config.reconciliation_interval = Duration::seconds(60);
    let handle = start_reconciliation_loop(config, scenario.database.pool.clone(), WorkItemStore);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let later_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM release_work_items WHERE source_library_id=$1",
            )
            .bind(later.id)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("later count");
            if later_count > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("later source progresses");
    let (reset, remaining): (bool, i64) = sqlx::query_as("SELECT recovery_reset, (SELECT count(*) FROM release_work_items WHERE discovery_scan_generation=$1 AND discovery_seen_mask<>0) FROM ingestion_discovery_scan WHERE scope_key='default'")
        .bind(generation).fetch_one(&scenario.database.pool).await.expect("durable reset");
    assert!(reset);
    assert_eq!(remaining, 1, "one turn clears at most four 128-row pages");
    handle.stop().await;

    scenario.create_release("missing-0", 1, false).await;
    scenario.reconcile_at(1).await;
    let reappeared = scenario.release_item("missing-0").await;
    assert_ne!(reappeared.state_reason.as_deref(), Some("source-missing"));
    let still_missing: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE source_library_id IS NULL AND state_reason='source-missing'")
        .fetch_one(&scenario.database.pool).await.expect("missing count");
    assert_eq!(still_missing, 512);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn missing_pages_yield_and_reobserve_a_reappearing_done_candidate() {
    let scenario = IngestionScenario::new("bounded_missing_pages", 30, 0).await;
    scenario.create_release("reappearing", 1, false).await;
    scenario.reconcile_at(0).await;
    let original = scenario.release_item("reappearing").await;
    let parked = scenario.root.with_extension("parked");
    fs::rename(scenario.root.join("reappearing"), &parked).expect("park candidate");
    sqlx::query("INSERT INTO release_work_items (id,api_key,source_path,current_path,state,correlation_id,discovered_at,updated_at,created_at) SELECT ('ffffffff-0000-4000-8000-' || lpad(to_hex(n),12,'0'))::uuid,gen_random_uuid(),'missing-' || n,'missing-' || n,'discovered',gen_random_uuid(),clock_timestamp(),clock_timestamp(),clock_timestamp() FROM generate_series(0,511) AS n")
        .execute(&scenario.database.pool).await.expect("missing candidates");
    let later_root = scenario.root.with_extension("later");
    fs::create_dir_all(later_root.join("release")).expect("later release");
    fs::write(later_root.join("release/track.flac"), b"audio").expect("later file");
    let later = InboundSource::from_root(
        later_root.clone(),
        Some(Uuid::now_v7()),
        Some("later".to_owned()),
    )
    .expect("later source");
    let mut config = IngestionConfig::with_sources(vec![scenario.source.clone(), later.clone()]);
    config.reconciliation_interval = Duration::seconds(60);
    let handle = start_reconciliation_loop(
        config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let later_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM release_work_items WHERE source_library_id=$1",
            )
            .bind(later.id)
            .fetch_one(&scenario.database.pool)
            .await
            .expect("later count");
            if later_count > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("later source progresses");
    let marked: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE source_library_id IS NULL AND state_reason='source-missing'")
        .fetch_one(&scenario.database.pool).await.expect("marked count");
    assert_eq!(marked, 512, "one turn marks at most four 128-row pages");
    assert_eq!(
        scenario
            .release_item("reappearing")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    handle.stop().await;
    fs::rename(&parked, scenario.root.join("reappearing")).expect("restore candidate");
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=(SELECT generation FROM ingestion_discovery_scan WHERE scope_key='default'), discovery_done_mask=65535 WHERE id=$1")
        .bind(original.id).execute(&scenario.database.pool).await.expect("previous done evidence");
    let second = start_reconciliation_loop(config, scenario.database.pool.clone(), WorkItemStore);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let partial: bool = sqlx::query_scalar("SELECT COALESCE((SELECT missing_revalidation AND clear_after IS NOT NULL AND lease_expires_at<=clock_timestamp() FROM ingestion_discovery_scan WHERE scope_key='default'),false)")
                .fetch_one(&scenario.database.pool).await.expect("revalidation cursor");
            if partial { break; }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }).await.expect("revalidation yields with a durable cursor");
    second.stop().await;
    scenario.reconcile_at(1).await;
    let reappeared = scenario.release_item("reappearing").await;
    assert_ne!(reappeared.state_reason.as_deref(), Some("source-missing"));
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE source_library_id IS NULL AND state_reason='source-missing'")
        .fetch_one(&scenario.database.pool).await.expect("missing count");
    assert_eq!(remaining, 512);
    fs::remove_dir_all(later_root).expect("later cleanup");
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(all(feature = "test-support", unix))]
#[tokio::test]
async fn missing_revalidation_aborts_when_root_is_replaced_after_absence_check() {
    use melodee_ingestion::DiscoveryResolveTestHook;
    let mut scenario = IngestionScenario::new("missing_guarded_swap", 30, 0).await;
    scenario.create_release("release", 1, false).await;
    scenario.reconcile_at(0).await;
    let before = scenario.release_item("release").await;
    let parked = scenario.root.with_extension("parked");
    fs::rename(scenario.root.join("release"), &parked).expect("park candidate");
    let generation = Uuid::now_v7();
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,initial_pass_complete,missing_resume) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),16,'missing',true,true)")
        .bind(generation).execute(&scenario.database.pool).await.expect("resume missing pass");
    let hook = DiscoveryResolveTestHook::new("release");
    scenario.config.discovery_resolve_test_hook = Some(hook.clone());
    let config = scenario.config.clone();
    let source = scenario.source.clone();
    let pool = scenario.database.pool.clone();
    let task = tokio::spawn(async move {
        reconcile_inbound_source(
            &config,
            &WorkItemStore,
            &pool,
            &source,
            common::T0 + Duration::seconds(1),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), hook.wait_before())
        .await
        .expect("before lookup");
    hook.release_before();
    tokio::time::timeout(std::time::Duration::from_secs(10), hook.wait_after())
        .await
        .expect("after absence lookup");
    let old_root = scenario.root.with_extension("old");
    fs::rename(&scenario.root, &old_root).expect("move old root");
    fs::create_dir_all(scenario.root.join("release")).expect("replacement candidate");
    fs::write(scenario.root.join("release/track.flac"), b"replacement").expect("replacement file");
    hook.release_after();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .expect("scan exits")
            .expect("join")
            .is_err()
    );
    let after = scenario.release_item("release").await;
    assert_eq!(after.id, before.id);
    assert_ne!(after.state_reason.as_deref(), Some("source-missing"));
    sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE scope_key='default'")
        .execute(&scenario.database.pool).await.expect("expire interrupted owner");
    scenario.config.discovery_resolve_test_hook = None;
    scenario.reconcile_at(2).await;
    assert_ne!(
        scenario
            .release_item("release")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(old_root).expect("cleanup old root");
    fs::remove_dir_all(parked).expect("cleanup parked");
    fs::remove_dir_all(&scenario.root).expect("cleanup new root");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn missing_revalidation_does_not_guess_lossy_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;
    let scenario = IngestionScenario::new("missing_non_utf8", 30, 0).await;
    let legacy = melodee_domain::ReleaseWorkItem::discovered(
        "bad-�".to_owned(),
        None,
        Uuid::now_v7(),
        common::T0,
    );
    let (legacy, _) = WorkItemStore
        .create_or_get(&scenario.database.pool, &legacy)
        .await
        .expect("legacy ambiguous row");
    sqlx::query("UPDATE release_work_items SET discovery_path_format=0 WHERE id=$1")
        .bind(legacy.id)
        .execute(&scenario.database.pool)
        .await
        .expect("legacy path version");
    let mut keys = Vec::new();
    for suffix in [0x80_u8, 0x81] {
        let name = std::ffi::OsString::from_vec(vec![b'b', b'a', b'd', b'-', suffix]);
        fs::create_dir(scenario.root.join(&name)).expect("candidate");
        keys.push(candidate_relative_path(PathBuf::from(&name).as_path()));
        fs::write(scenario.root.join(name).join("track.flac"), b"audio").expect("file");
    }
    scenario.reconcile_at(0).await;
    assert_ne!(keys[0], keys[1]);
    let first = WorkItemStore
        .get_by_source_identity(&scenario.database.pool, None, &keys[0])
        .await
        .expect("first lookup")
        .expect("first item");
    let second = WorkItemStore
        .get_by_source_identity(&scenario.database.pool, None, &keys[1])
        .await
        .expect("second lookup")
        .expect("second item");
    assert_ne!(first.id, second.id);
    assert_ne!(legacy.id, first.id);
    let generation = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$1,discovery_seen_mask=0 WHERE source_library_id IS NULL")
        .bind(generation).execute(&scenario.database.pool).await.expect("old observation");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,initial_pass_complete,missing_resume) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),16,'missing',true,true)")
        .bind(generation).execute(&scenario.database.pool).await.expect("resume missing pass");
    scenario.reconcile_at(1).await;
    assert_ne!(
        scenario.release_item("bad-�").await.state_reason.as_deref(),
        Some("source-missing")
    );
    for key in keys {
        let item = WorkItemStore
            .get_by_source_identity(&scenario.database.pool, None, &key)
            .await
            .expect("lookup")
            .expect("item");
        assert_ne!(item.state_reason.as_deref(), Some("source-missing"));
    }
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn long_non_utf8_nested_occurrence_is_persisted_without_btree_key_overflow() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let scenario = IngestionScenario::new("long_non_utf8_occurrence", 30, 0).await;
    let mut relative = PathBuf::from("holder");
    for index in 0..8 {
        relative.push(format!("{index}{}", "a".repeat(190)));
    }
    relative.push(std::ffi::OsString::from_vec(vec![b'r', 0x80]));
    let target = scenario.root.join(&relative);
    fs::create_dir_all(&target).expect("nested candidate");
    fs::write(target.join("track.flac"), b"audio").expect("file");
    symlink(&target, scenario.root.join("alias")).expect("alias");
    let key = candidate_relative_path(&relative);
    assert!(
        key.len() > 2_704,
        "exercise PostgreSQL B-tree entry ceiling"
    );
    scenario.reconcile_at(0).await;
    let item = WorkItemStore
        .get_by_source_identity(&scenario.database.pool, None, &key)
        .await
        .expect("lookup")
        .expect("long path item");
    assert_eq!(item.source_path, key);
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn unreadable_missing_path_retries_without_starving_later_rows() {
    let scenario = IngestionScenario::new("missing_retry_fairness", 30, 0).await;
    let generation = Uuid::now_v7();
    for path in ["../invalid", "legitimately-missing"] {
        let item = melodee_domain::ReleaseWorkItem::discovered(
            path.to_owned(),
            None,
            Uuid::now_v7(),
            common::T0,
        );
        // Keep the application's other required fields while choosing stable IDs.
        let (created, _) = WorkItemStore
            .create_or_get(&scenario.database.pool, &item)
            .await
            .expect("work item");
        sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2,discovery_seen_mask=0 WHERE id=$1")
            .bind(created.id).bind(generation).execute(&scenario.database.pool).await.expect("prior observation");
    }
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,initial_pass_complete,missing_resume) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),16,'missing',true,true)")
        .bind(generation).execute(&scenario.database.pool).await.expect("resume missing pass");
    let error = reconcile_inbound_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.database.pool,
        &scenario.source,
        common::T0 + Duration::seconds(1),
    )
    .await
    .expect_err("invalid path remains retryable");
    assert!(matches!(
        error,
        melodee_ingestion::IngestionError::Persistence(
            melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(_)
        )
    ));
    assert_eq!(
        scenario
            .release_item("legitimately-missing")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    let invalid_item = scenario.release_item("../invalid").await;
    let retry: Option<Uuid> = sqlx::query_scalar(
        "SELECT discovery_missing_retry_generation FROM release_work_items WHERE id=$1",
    )
    .bind(invalid_item.id)
    .fetch_one(&scenario.database.pool)
    .await
    .expect("retry marker");
    assert_eq!(retry, Some(generation));
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn scheduler_continues_later_source_when_first_root_is_missing() {
    let mut scenario = IngestionScenario::new("missing_source_fairness", 30, 0).await;
    let other_root =
        std::env::temp_dir().join(format!("melodee-other-source-{}", Uuid::now_v7().simple()));
    fs::create_dir_all(other_root.join("healthy")).expect("other release");
    let other = InboundSource::from_root(
        other_root.clone(),
        Some(Uuid::now_v7()),
        Some("second".to_owned()),
    )
    .expect("second source");
    scenario.config.sources.push(other.clone());
    scenario.config.reconciliation_interval = Duration::seconds(60);
    fs::remove_dir_all(&scenario.root).expect("remove first root");
    let handle = start_reconciliation_loop(
        scenario.config.clone(),
        scenario.database.pool.clone(),
        WorkItemStore,
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let item = WorkItemStore
                .get_by_source_identity(&scenario.database.pool, other.id, "healthy")
                .await
                .expect("work item query");
            if item.is_some_and(|item| item.latest_settling_snapshot_id.is_some()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("later source must make progress");
    handle.stop().await;
    fs::remove_dir_all(other_root).expect("cleanup");
    scenario.database.drop().await;
}

#[tokio::test]
async fn resumed_scan_does_not_reuse_observations_after_root_replacement() {
    let scenario = IngestionScenario::new("replaced_source_root", 30, 0).await;
    scenario.create_release("formerly-present", 1, false).await;
    scenario.reconcile_at(0).await;
    let item = scenario.release_item("formerly-present").await;
    let generation = Uuid::now_v7();
    sqlx::query("UPDATE release_work_items SET discovery_scan_generation=$2, discovery_seen_mask=65535, discovery_done_mask=65535 WHERE id=$1")
        .bind(item.id).bind(generation).execute(&scenario.database.pool).await.expect("old observations");
    sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key,generation,owner,lease_expires_at,birth_cutoff,partition,phase,phase_started) VALUES ('default',$1,$1,clock_timestamp()-interval '1 second',clock_timestamp(),12,'collect',true)")
        .bind(generation).execute(&scenario.database.pool).await.expect("crashed scan");
    let old_root = scenario.root.with_extension("old");
    fs::rename(&scenario.root, &old_root).expect("move old root");
    fs::create_dir_all(&scenario.root).expect("replacement root");
    scenario.reconcile_at(1).await;
    assert_eq!(
        scenario
            .release_item("formerly-present")
            .await
            .state_reason
            .as_deref(),
        Some("source-missing")
    );
    fs::remove_dir_all(old_root).expect("cleanup old root");
    fs::remove_dir_all(&scenario.root).expect("cleanup new root");
    scenario.database.drop().await;
}

#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[tokio::test]
async fn absolute_alias_swap_away_and_back_never_marks_original_missing() {
    use melodee_ingestion::DiscoveryResolveTestHook;
    use std::os::unix::fs::symlink;
    struct ReleaseHook(DiscoveryResolveTestHook);
    impl Drop for ReleaseHook {
        fn drop(&mut self) {
            self.0.release_before();
            self.0.release_after();
        }
    }
    let mut scenario = IngestionScenario::new("absolute_alias_swap", 30, 0).await;
    fs::create_dir_all(scenario.root.join("holder/release")).expect("old release");
    fs::write(scenario.root.join("holder/release/old"), b"old").expect("old data");
    symlink(
        scenario.root.join("holder/release"),
        scenario.root.join("inside"),
    )
    .expect("absolute alias");
    scenario.reconcile_at(0).await;
    let before = scenario.release_item("holder/release").await;
    let hook = DiscoveryResolveTestHook::new("inside");
    let _release_on_drop = ReleaseHook(hook.clone());
    scenario.config.discovery_resolve_test_hook = Some(hook.clone());
    let config = scenario.config.clone();
    let pool = scenario.database.pool.clone();
    let source = scenario.source.clone();
    let task = tokio::spawn(async move {
        reconcile_inbound_source(
            &config,
            &WorkItemStore,
            &pool,
            &source,
            common::T0 + Duration::seconds(1),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), hook.wait_before())
        .await
        .expect("resolve pause");
    let old_root = scenario.root.with_extension("old");
    fs::rename(&scenario.root, &old_root).expect("move original");
    fs::create_dir_all(scenario.root.join("holder/release")).expect("replacement release");
    fs::write(scenario.root.join("holder/release/new"), b"new").expect("replacement data");
    hook.release_before();
    tokio::time::timeout(std::time::Duration::from_secs(15), hook.wait_after())
        .await
        .expect("resolved while swapped");
    fs::remove_dir_all(&scenario.root).expect("remove replacement");
    fs::rename(&old_root, &scenario.root).expect("restore original");
    hook.release_after();
    tokio::time::timeout(std::time::Duration::from_secs(30), task)
        .await
        .expect("scan finished")
        .expect("join")
        .expect("scan");
    let after = scenario.release_item("holder/release").await;
    assert_eq!(after.id, before.id);
    assert_ne!(after.state_reason.as_deref(), Some("source-missing"));
    assert_eq!(
        after.latest_settling_snapshot_id, before.latest_settling_snapshot_id,
        "replacement metadata must not be persisted"
    );
    fs::remove_dir_all(&scenario.root).expect("cleanup");
    scenario.database.drop().await;
}
