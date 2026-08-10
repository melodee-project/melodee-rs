//! Phase 3.2 vertical normalization, concurrency, lease, and failure scenarios.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use melodee_domain::{IngestionState, WorkEventType};
use melodee_ingestion::{
    InboundSource, IngestionConfig, inspect_ready_once, normalize_once, reconcile_all_sources,
    start_normalization_loop,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: sqlx::PgPool,
    database: common::TestDb,
}

impl Scenario {
    async fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "melodee-normalization-{name}-{}",
            Uuid::now_v7().simple()
        ));
        fs::create_dir(&root).expect("source root");
        let database = common::TestDb::create(name).await;
        let source = InboundSource::from_root(root.clone(), None, Some(name.to_owned())).unwrap();
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.settling_quiet_period = Duration::ZERO;
        config.inspection_concurrency = 32;
        config.normalization_concurrency = 8;
        config.work_area_base = root.with_extension("work");
        Self {
            root,
            config,
            pool: database.pool.clone(),
            database,
        }
    }

    async fn discover_settle_and_inspect(&self) {
        reconcile_all_sources(&self.config, &WorkItemStore, &self.pool, common::T0)
            .await
            .expect("discover");
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.pool,
            common::T0 + Duration::seconds(1),
        )
        .await
        .expect("settle");
        let mut inspected = 0_u64;
        loop {
            let stats = inspect_ready_once(
                &self.config,
                &WorkItemStore,
                &self.pool,
                &WorkEngineConfig::default(),
                common::T0 + Duration::seconds(2 + i64::try_from(inspected).unwrap_or(0)),
            )
            .await
            .expect("inspection batch");
            inspected += stats.completed;
            if stats.claimed == 0 {
                break;
            }
        }
    }

    async fn finish(self) {
        let work_area = self.config.work_area_base.clone();
        let root = self.root.clone();
        self.database.drop().await;
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(work_area);
    }
}

fn tools_available() -> bool {
    ["ffmpeg", "ffprobe", "fpcalc"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn tool_path(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .expect("tool on PATH")
}

fn generate_wav(path: &Path) {
    let output = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=5")
        .args(["-c:a", "pcm_s16le"])
        .arg(path)
        .output()
        .expect("generate WAV");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

fn source_hashes(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut hashes = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let release = entry.unwrap().path();
        if !release.is_dir() {
            continue;
        }
        for file in fs::read_dir(&release).unwrap() {
            let file = file.unwrap().path();
            if file.is_file() {
                hashes.insert(
                    file.strip_prefix(root).unwrap().to_path_buf(),
                    blake3::hash(&fs::read(file).unwrap()).to_hex().to_string(),
                );
            }
        }
    }
    hashes
}

#[cfg(unix)]
#[tokio::test]
async fn two_hundred_releases_normalize_with_zero_loss_and_bounded_concurrency() {
    if !tools_available() {
        eprintln!("skipping normalization acceptance: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_200").await;
    let fixture = scenario.root.join("fixture.wav");
    generate_wav(&fixture);
    for index in 0..200 {
        let release = scenario.root.join(format!("release-{index:03}"));
        fs::create_dir(&release).unwrap();
        fs::copy(&fixture, release.join("track.wav")).unwrap();
    }
    fs::remove_file(fixture).unwrap();
    let source_before = source_hashes(&scenario.root);

    let active = scenario.root.join("active-count");
    let maximum = scenario.root.join("maximum-count");
    let lock = scenario.root.join("counter-lock");
    fs::write(&active, "0").unwrap();
    fs::write(&maximum, "0").unwrap();
    let wrapper = scenario.root.join("bounded-ffmpeg");
    let real_ffmpeg = tool_path("ffmpeg");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\nwhile ! mkdir '{}' 2>/dev/null; do sleep 0.001; done\nactive=$(cat '{}')\nactive=$((active + 1))\nprintf '%s' \"$active\" > '{}'\nmaximum=$(cat '{}')\nif [ \"$active\" -gt \"$maximum\" ]; then printf '%s' \"$active\" > '{}'; fi\nrmdir '{}'\nsleep 0.05\n'{}' \"$@\"\nstatus=$?\nwhile ! mkdir '{}' 2>/dev/null; do sleep 0.001; done\nactive=$(cat '{}')\nactive=$((active - 1))\nprintf '%s' \"$active\" > '{}'\nrmdir '{}'\nexit \"$status\"",
            real_ffmpeg.display(),
            lock.display(),
            active.display(),
            active.display(),
            maximum.display(),
            maximum.display(),
            lock.display(),
            real_ffmpeg.display(),
            lock.display(),
            active.display(),
            active.display(),
            lock.display(),
        ),
    );
    scenario.config.ffmpeg_path = wrapper;
    scenario.discover_settle_and_inspect().await;

    let started = Instant::now();
    let mut successful = 0_u64;
    let mut expected_failures = 0_u64;
    let mut unexpected_failures = 0_u64;
    loop {
        let stats = normalize_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("normalization batch");
        successful += stats.completed;
        expected_failures += stats.failed;
        unexpected_failures += stats.errors;
        if stats.claimed == 0 {
            break;
        }
    }
    let measured_maximum = fs::read_to_string(&maximum)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(measured_maximum > 1, "normalization never ran concurrently");
    assert!(measured_maximum <= scenario.config.normalization_concurrency);
    assert_eq!(200, successful);
    assert_eq!(0, expected_failures);
    assert_eq!(0, unexpected_failures);
    assert_eq!(source_before, source_hashes(&scenario.root));

    let final_states: Vec<(String, i64)> = sqlx::query_as(
        "SELECT state, COUNT(*) FROM release_work_items GROUP BY state ORDER BY state",
    )
    .fetch_all(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(vec![("validating".to_owned(), 200)], final_states);
    let (manifests, duplicate_manifests, transitions): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_work_item_manifests),
           (SELECT COUNT(*) - COUNT(DISTINCT work_item_id) FROM release_work_item_manifests),
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE from_state = 'normalizing' AND to_state = 'validating')",
    )
    .fetch_one(&scenario.pool)
    .await
    .unwrap();
    assert_eq!((200, 0, 200), (manifests, duplicate_manifests, transitions));
    eprintln!(
        "phase3.2-acceptance={{\"total\":200,\"successful\":{successful},\"expected_failure\":{expected_failures},\"unexpected_failure\":{unexpected_failures},\"lost\":0,\"duplicate_authoritative_result\":{duplicate_manifests},\"source_mutations\":0,\"validating\":200,\"configured_concurrency\":{},\"measured_concurrency\":{measured_maximum},\"elapsed_ms\":{}}}",
        scenario.config.normalization_concurrency,
        started.elapsed().as_millis()
    );
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn lease_shorter_than_old_heartbeat_is_renewed_during_long_operation() {
    if !tools_available() {
        eprintln!("skipping normalization heartbeat test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_heartbeat").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_wav(&release.join("track.wav"));
    scenario.discover_settle_and_inspect().await;

    let wrapper = scenario.root.join("three-second-ffmpeg");
    let real_ffmpeg = tool_path("ffmpeg");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\nsleep 3\nexec '{}' \"$@\"",
            real_ffmpeg.display(),
            real_ffmpeg.display()
        ),
    );
    scenario.config.ffmpeg_path = wrapper;
    scenario.config.normalization_concurrency = 1;
    let work_config = WorkEngineConfig {
        lease_duration: Duration::seconds(2),
        ..WorkEngineConfig::default()
    };
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, stats.completed);
    let item = WorkItemStore
        .list(&scenario.pool, None, 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(IngestionState::Validating, item.state);
    let renewals = WorkItemStore
        .list_events(&scenario.pool, item.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == WorkEventType::LeaseRenewed)
        .count();
    assert!(
        renewals >= 2,
        "long operation did not renew its short lease"
    );
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn short_lease_is_renewed_and_lease_loss_cancels_stale_attempt() {
    if !tools_available() {
        eprintln!("skipping normalization lease test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_lease").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_wav(&release.join("track.wav"));
    scenario.discover_settle_and_inspect().await;

    let slow_wrapper = scenario.root.join("slow-ffmpeg");
    let real_ffmpeg = tool_path("ffmpeg");
    write_script(
        &slow_wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\nend=$(( $(date +%s) + 30 ))\nwhile [ \"$(date +%s)\" -lt \"$end\" ]; do :; done\nexec '{}' \"$@\"",
            real_ffmpeg.display(),
            real_ffmpeg.display()
        ),
    );
    scenario.config.ffmpeg_path = slow_wrapper;
    scenario.config.normalization_concurrency = 1;
    let work_config = WorkEngineConfig {
        lease_duration: Duration::seconds(2),
        ..WorkEngineConfig::default()
    };
    let pool = scenario.pool.clone();
    let config = scenario.config.clone();
    let task = tokio::spawn(async move {
        normalize_once(
            &config,
            &WorkItemStore,
            &pool,
            &work_config,
            OffsetDateTime::now_utc(),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    sqlx::query(
        "UPDATE release_work_items
         SET lease_owner = 'forced-new-owner', lease_expires_at = now() + interval '30 seconds'
         WHERE state = 'normalizing'",
    )
    .execute(&scenario.pool)
    .await
    .unwrap();
    let cancellation_started = Instant::now();
    let first = task.await.unwrap().unwrap();
    assert_eq!(1, first.claimed);
    assert_eq!(0, first.completed);
    assert_eq!(1, first.errors);
    assert!(cancellation_started.elapsed() < std::time::Duration::from_secs(3));

    let item = WorkItemStore
        .list(&scenario.pool, None, 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(IngestionState::Normalizing, item.state);
    assert_eq!(Some("forced-new-owner"), item.lease_owner.as_deref());
    assert!(
        !WorkItemStore
            .list_events(&scenario.pool, item.id)
            .await
            .unwrap()
            .iter()
            .any(|event| {
                event.event_type == WorkEventType::Transitioned
                    && event.to_state == Some(IngestionState::Validating)
            })
    );

    sqlx::query(
        "UPDATE release_work_items
         SET lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL
         WHERE id = $1",
    )
    .bind(item.id)
    .execute(&scenario.pool)
    .await
    .unwrap();
    scenario.config.ffmpeg_path = real_ffmpeg;
    let retry = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &work_config,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, retry.completed);
    let attempts = fs::read_dir(
        scenario
            .config
            .work_area_base
            .join(item.id.simple().to_string())
            .join("attempts"),
    )
    .unwrap()
    .count();
    assert_eq!(2, attempts, "retry reused or removed stale attempt area");
    let final_item = WorkItemStore.get(&scenario.pool, item.id).await.unwrap();
    assert_eq!(IngestionState::Validating, final_item.state);
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn one_fingerprint_failure_does_not_stop_sibling_release() {
    if !tools_available() {
        eprintln!("skipping normalization failure-isolation test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_failure_isolation").await;
    for name in ["good", "bad"] {
        let release = scenario.root.join(name);
        fs::create_dir(&release).unwrap();
        generate_wav(&release.join("track.wav"));
    }
    scenario.discover_settle_and_inspect().await;
    let real_fpcalc = tool_path("fpcalc");
    let wrapper = scenario.root.join("selective-fpcalc");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\ncase \"$5\" in *'/bad/'*) printf 'synthetic failure' >&2; exit 17;; esac\nexec '{}' \"$@\"",
            real_fpcalc.display(),
            real_fpcalc.display()
        ),
    );
    scenario.config.fpcalc_path = wrapper;
    scenario.config.normalization_concurrency = 2;
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(2, stats.claimed);
    assert_eq!(1, stats.completed);
    assert_eq!(1, stats.failed);
    assert_eq!(0, stats.errors);
    let states: BTreeMap<String, String> = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|item| (item.source_path, item.state.as_str().to_owned()))
        .collect();
    assert_eq!(Some(&"validating".to_owned()), states.get("good"));
    assert_eq!(Some(&"failed".to_owned()), states.get("bad"));
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn graceful_worker_shutdown_cancels_in_flight_ffmpeg() {
    if !tools_available() {
        eprintln!("skipping normalization shutdown test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_shutdown").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_wav(&release.join("track.wav"));
    scenario.discover_settle_and_inspect().await;

    let wrapper = scenario.root.join("shutdown-ffmpeg");
    let real_ffmpeg = tool_path("ffmpeg");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\nend=$(( $(date +%s) + 30 ))\nwhile [ \"$(date +%s)\" -lt \"$end\" ]; do :; done\nexec '{}' \"$@\"",
            real_ffmpeg.display(),
            real_ffmpeg.display()
        ),
    );
    scenario.config.ffmpeg_path = wrapper;
    scenario.config.normalization_concurrency = 1;
    let handle = start_normalization_loop(
        scenario.config.clone(),
        scenario.pool.clone(),
        WorkItemStore,
        WorkEngineConfig::default(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let started = Instant::now();
    let _results = handle.stop().await;
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let item = WorkItemStore
        .list(&scenario.pool, None, 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(IngestionState::Normalizing, item.state);
    assert!(
        !WorkItemStore
            .list_events(&scenario.pool, item.id)
            .await
            .unwrap()
            .iter()
            .any(|event| event.to_state == Some(IngestionState::Validating))
    );
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn source_tree_change_during_normalization_discards_result() {
    if !tools_available() {
        eprintln!("skipping normalization source-change test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_source_change").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_wav(&release.join("track.wav"));
    scenario.discover_settle_and_inspect().await;

    let wrapper = scenario.root.join("source-change-ffmpeg");
    let real_ffmpeg = tool_path("ffmpeg");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\nsleep 1\nexec '{}' \"$@\"",
            real_ffmpeg.display(),
            real_ffmpeg.display()
        ),
    );
    scenario.config.ffmpeg_path = wrapper;
    scenario.config.normalization_concurrency = 1;
    let config = scenario.config.clone();
    let pool = scenario.pool.clone();
    let task = tokio::spawn(async move {
        normalize_once(
            &config,
            &WorkItemStore,
            &pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    fs::write(release.join("late.nfo"), b"late change").unwrap();
    let stats = task.await.unwrap().unwrap();
    assert_eq!(1, stats.claimed);
    assert_eq!(0, stats.completed);
    assert_eq!(1, stats.failed);
    let item = WorkItemStore
        .list(&scenario.pool, None, 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(IngestionState::Failed, item.state);
    assert_eq!(
        Some("normalization.source_changed"),
        item.last_error_code.as_ref().map(|code| code.as_str())
    );
    let manifest = WorkItemStore
        .get_manifest(&scenario.pool, item.id)
        .await
        .unwrap()
        .unwrap();
    assert!(manifest.payload["normalization"].is_null());
    scenario.finish().await;
}
