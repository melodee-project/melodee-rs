//! Phase 3 vertical normalization/publication, concurrency, lease, and fault scenarios.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::Instant,
};

use melodee_domain::{IngestionState, PublicationJournalStep, ReleaseManifest, WorkEventType};
#[cfg(feature = "test-support")]
use melodee_ingestion::PublicationCopyTestBarrier;
use melodee_ingestion::{
    InboundSource, IngestionConfig, PublicationBundle, PublicationOutcome, inspect_ready_once,
    normalize_once, publish_canonical_bundle, publish_once, reconcile_all_sources,
    start_normalization_loop, validate_once,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
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

    fn enable_publication(&mut self) -> PathBuf {
        let storage_root = self.root.with_extension("canonical");
        fs::create_dir(&storage_root).expect("canonical storage root");
        self.config.storage_root = Some(storage_root.clone());
        storage_root
    }

    async fn finish(self) {
        let work_area = self.config.work_area_base.clone();
        let storage_root = self.config.storage_root.clone();
        let root = self.root.clone();
        self.database.drop().await;
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(work_area);
        if let Some(storage_root) = storage_root {
            let _ = fs::remove_dir_all(storage_root);
        }
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

fn generate_tagged_mp3(path: &Path) {
    let output = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=5")
        .args([
            "-c:a",
            "libmp3lame",
            "-q:a",
            "4",
            "-metadata",
            "album=Synthetic Release",
            "-metadata",
            "album_artist=Synthetic Artist",
            "-metadata",
            "artist=Synthetic Artist",
            "-metadata",
            "title=Synthetic Track",
            "-metadata",
            "track=1",
        ])
        .arg(path)
        .output()
        .expect("generate tagged MP3");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_tagged_audio(path: &Path, extension: &str) {
    let codec = match extension {
        "mp3" => "libmp3lame",
        "flac" => "flac",
        "ogg" => "libvorbis",
        "wav" => "pcm_s16le",
        _ => panic!("unsupported test format {extension}"),
    };
    let output = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=523:duration=5")
        .args([
            "-c:a",
            codec,
            "-metadata",
            "album=Pathology Release",
            "-metadata",
            "album_artist=Pathology Artist",
            "-metadata",
            "artist=Pathology Artist",
            "-metadata",
            "title=Unicode Track",
            "-metadata",
            "track=1",
        ])
        .arg(path)
        .output()
        .expect("generate tagged media");
    assert!(
        output.status.success(),
        "{} fixture generation failed: {}",
        extension,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_png(path: &Path) {
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=96x64",
            "-frames:v",
            "1",
        ])
        .arg(path)
        .output()
        .expect("generate PNG artwork");
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

fn regular_files_named(root: &Path, name: &str) -> usize {
    fn walk(current: &Path, name: &str, count: &mut usize) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(&path, name, count);
            } else if metadata.is_file()
                && path.file_name().and_then(|value| value.to_str()) == Some(name)
            {
                *count += 1;
            }
        }
    }

    let mut count = 0;
    walk(root, name, &mut count);
    count
}

fn paths_with_fragment(root: &Path, fragment: &str) -> usize {
    fn walk(current: &Path, fragment: &str, count: &mut usize) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(fragment))
            {
                *count += 1;
            }
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                walk(&path, fragment, count);
            }
        }
    }

    let mut count = 0;
    walk(root, fragment, &mut count);
    count
}

#[cfg(unix)]
#[tokio::test]
async fn two_hundred_releases_reach_published_once_with_zero_loss_and_bounded_concurrency() {
    if !tools_available() {
        eprintln!("skipping normalization acceptance: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_200").await;
    let fixture = scenario.root.join("fixture.mp3");
    generate_tagged_mp3(&fixture);
    for index in 0..200 {
        let release = scenario.root.join(format!("release-{index:03}"));
        fs::create_dir(&release).unwrap();
        fs::copy(&fixture, release.join("track.mp3")).unwrap();
    }
    fs::remove_file(fixture).unwrap();
    let storage_root = scenario.enable_publication();
    scenario.config.validation_concurrency = 8;
    scenario.config.publication_concurrency = 8;
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
    let mut normalized = 0_u64;
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
        normalized += stats.completed;
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
    assert_eq!(200, normalized);
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

    let mut validated = 0_u64;
    let mut validation_review = 0_u64;
    let mut validation_failures = 0_u64;
    let mut validation_errors = 0_u64;
    loop {
        let stats = validate_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("validation batch");
        validated += stats.completed;
        validation_review += stats.needs_review;
        validation_failures += stats.failed;
        validation_errors += stats.errors;
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, validated);
    assert_eq!(0, validation_review);
    assert_eq!(0, validation_failures);
    assert_eq!(0, validation_errors);

    let mut published = 0_u64;
    let mut adopted = 0_u64;
    let mut publication_failures = 0_u64;
    let mut publication_errors = 0_u64;
    loop {
        let stats = publish_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("publication batch");
        published += stats.completed;
        adopted += stats.adopted;
        publication_failures += stats.failed;
        publication_errors += stats.errors;
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, published);
    assert_eq!(0, adopted);
    assert_eq!(0, publication_failures);
    assert_eq!(0, publication_errors);
    assert_eq!(source_before, source_hashes(&scenario.root));

    let no_more = publish_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(0, no_more.claimed);
    let final_states: Vec<(String, i64)> = sqlx::query_as(
        "SELECT state, COUNT(*) FROM release_work_items GROUP BY state ORDER BY state",
    )
    .fetch_all(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(vec![("published".to_owned(), 200)], final_states);
    let (
        authorizations,
        plans,
        distinct_release_ids,
        receipts,
        published_transitions,
        finalized_journal,
    ): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_publication_authorizations),
           (SELECT COUNT(*) FROM release_publication_plans),
           (SELECT COUNT(DISTINCT release_id) FROM release_publication_plans),
           (SELECT COUNT(*) FROM release_publication_receipts),
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE from_state = 'publishing' AND to_state = 'published'),
           (SELECT COUNT(*) FROM release_publication_journal
            WHERE step IN ('finalized', 'final_adopted'))",
    )
    .fetch_one(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(
        (200, 200, 200, 200, 200, 200),
        (
            authorizations,
            plans,
            distinct_release_ids,
            receipts,
            published_transitions,
            finalized_journal,
        )
    );
    let canonical_manifests = regular_files_named(&storage_root, "manifest.json");
    assert_eq!(200, canonical_manifests);
    assert_eq!(0, paths_with_fragment(&storage_root, ".incoming"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".copying"));
    eprintln!(
        "phase3.4-acceptance={{\"total\":200,\"normalized\":{normalized},\"validated\":{validated},\"published\":{published},\"expected_failure\":{},\"unexpected_failure\":{},\"lost\":0,\"duplicate_authoritative_result\":{},\"source_mutations\":0,\"publication_receipts\":{receipts},\"configured_normalization_concurrency\":{},\"measured_normalization_concurrency\":{measured_maximum},\"configured_validation_concurrency\":{},\"configured_publication_concurrency\":{},\"elapsed_ms\":{}}}",
        expected_failures + validation_review + validation_failures + publication_failures,
        unexpected_failures + validation_errors + publication_errors,
        plans - distinct_release_ids,
        scenario.config.normalization_concurrency,
        scenario.config.validation_concurrency,
        scenario.config.publication_concurrency,
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

#[cfg(unix)]
#[tokio::test]
async fn final_rename_before_database_completion_is_reclaimed_and_adopted_once() {
    if !tools_available() {
        eprintln!("skipping publication crash-recovery test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_rename_crash").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_tagged_mp3(&release.join("track.mp3"));
    let storage_root = scenario.enable_publication();
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    scenario.config.publication_concurrency = 1;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;
    let normalized = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, normalized.completed);
    let validated = validate_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, validated.completed);

    let item = WorkItemStore
        .list(&scenario.pool, Some(IngestionState::Staged), 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let crashed_worker = WorkerId::new();
    let expired_config = WorkEngineConfig {
        claim_batch_size: 1,
        lease_duration: Duration::seconds(2),
        ..WorkEngineConfig::default()
    };
    let crash_time = OffsetDateTime::now_utc() - Duration::seconds(10);
    let claimed = WorkItemStore
        .claim_publication(&scenario.pool, &crashed_worker, &expired_config, crash_time)
        .await
        .unwrap();
    assert_eq!(1, claimed.len());
    assert_eq!(IngestionState::Publishing, claimed[0].state);

    let stored_publication = WorkItemStore
        .get_publication(&scenario.pool, item.id)
        .await
        .unwrap()
        .unwrap();
    let stored_manifest = WorkItemStore
        .get_manifest(&scenario.pool, item.id)
        .await
        .unwrap()
        .unwrap();
    let manifest: ReleaseManifest = serde_json::from_value(stored_manifest.payload).unwrap();
    let work_root = PathBuf::from(
        manifest
            .normalization
            .as_ref()
            .unwrap()
            .provenance
            .work_area
            .as_ref()
            .unwrap(),
    );
    let bundle = PublicationBundle {
        manifest,
        plan: stored_publication.plan,
        plan_hash: stored_publication.plan_hash,
        work_root,
    };
    let filesystem_receipt =
        publish_canonical_bundle(&storage_root, &bundle, &AtomicBool::new(false)).unwrap();
    assert_eq!(PublicationOutcome::Published, filesystem_receipt.outcome);
    assert_eq!(
        IngestionState::Publishing,
        WorkItemStore
            .get(&scenario.pool, item.id)
            .await
            .unwrap()
            .state
    );
    assert!(
        WorkItemStore
            .get_publication(&scenario.pool, item.id)
            .await
            .unwrap()
            .unwrap()
            .published_at
            .is_none()
    );

    let recovered = publish_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, recovered.claimed);
    assert_eq!(1, recovered.completed);
    assert_eq!(1, recovered.adopted);
    let final_item = WorkItemStore.get(&scenario.pool, item.id).await.unwrap();
    assert_eq!(IngestionState::Published, final_item.state);
    assert!(final_item.lease_owner.is_none());
    let journal = WorkItemStore
        .list_publication_journal(&scenario.pool, item.id)
        .await
        .unwrap();
    for step in [
        PublicationJournalStep::AssetCopied,
        PublicationJournalStep::BundleSealed,
        PublicationJournalStep::FinalAdopted,
    ] {
        assert_eq!(
            1,
            journal.iter().filter(|entry| entry.step == step).count(),
            "recovery must durably record exactly one {step:?} milestone"
        );
    }
    assert_eq!(
        1_i64,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM release_publication_receipts WHERE work_item_id = $1",
        )
        .bind(item.id)
        .fetch_one(&scenario.pool)
        .await
        .unwrap()
    );
    let published_events = WorkItemStore
        .list_events(&scenario.pool, item.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.from_state == Some(IngestionState::Publishing)
                && event.to_state == Some(IngestionState::Published)
        })
        .count();
    assert_eq!(1, published_events);
    assert_eq!(source_before, source_hashes(&scenario.root));
    scenario.finish().await;
}

#[cfg(all(unix, feature = "test-support"))]
#[tokio::test]
async fn publication_lease_theft_cancels_stale_copy_and_direct_retry_converges() {
    if !tools_available() {
        eprintln!("skipping publication lease-theft test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_lease_theft").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_tagged_mp3(&release.join("track.mp3"));
    let storage_root = scenario.enable_publication();
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    scenario.config.publication_concurrency = 1;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;
    assert_eq!(
        1,
        normalize_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );
    assert_eq!(
        1,
        validate_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );
    let staged = WorkItemStore
        .list(&scenario.pool, Some(IngestionState::Staged), 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let attempt_count_before = staged.attempt_count;
    let publication = WorkItemStore
        .get_publication(&scenario.pool, staged.id)
        .await
        .unwrap()
        .unwrap();
    let final_path = storage_root.join(&publication.plan.target_relative_path);

    let barrier = PublicationCopyTestBarrier::new();
    scenario.config.publication_copy_test_barrier = Some(barrier.clone());
    let task_config = scenario.config.clone();
    let task_pool = scenario.pool.clone();
    let work_config = WorkEngineConfig {
        claim_batch_size: 1,
        lease_duration: Duration::seconds(2),
        ..WorkEngineConfig::default()
    };
    let stale_task = tokio::spawn(async move {
        publish_once(
            &task_config,
            &WorkItemStore,
            &task_pool,
            &work_config,
            OffsetDateTime::now_utc(),
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        barrier.wait_until_reached(),
    )
    .await
    .expect("publication copy barrier was not reached");

    let journal_before_theft = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let journal = WorkItemStore
                .list_publication_journal(&scenario.pool, staged.id)
                .await
                .unwrap();
            if journal
                .iter()
                .any(|entry| entry.step == PublicationJournalStep::IncomingPrepared)
            {
                break journal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("IncomingPrepared was not durably journaled before theft");
    assert_eq!(1, journal_before_theft.len());

    let forced_owner = format!("forced-publication-owner-{}", Uuid::now_v7().simple());
    let stolen = sqlx::query(
        "UPDATE release_work_items
         SET lease_owner = $2, lease_acquired_at = now(),
             lease_expires_at = now() + interval '30 seconds'
         WHERE id = $1 AND state = 'publishing'",
    )
    .bind(staged.id)
    .bind(&forced_owner)
    .execute(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(1, stolen.rows_affected());

    let stale = tokio::time::timeout(std::time::Duration::from_secs(4), stale_task)
        .await
        .expect("lease loss did not cancel publication promptly")
        .unwrap()
        .unwrap();
    assert_eq!(1, stale.claimed);
    assert_eq!(0, stale.completed);
    assert_eq!(0, stale.failed);
    assert_eq!(1, stale.errors);
    let fenced = WorkItemStore.get(&scenario.pool, staged.id).await.unwrap();
    assert_eq!(IngestionState::Publishing, fenced.state);
    assert_eq!(Some(forced_owner.as_str()), fenced.lease_owner.as_deref());
    assert_eq!(attempt_count_before, fenced.attempt_count);
    let journal_after_theft = WorkItemStore
        .list_publication_journal(&scenario.pool, staged.id)
        .await
        .unwrap();
    assert_eq!(journal_before_theft, journal_after_theft);
    assert!(journal_after_theft.iter().all(|entry| {
        !matches!(
            entry.step,
            PublicationJournalStep::AssetCopied
                | PublicationJournalStep::BundleSealed
                | PublicationJournalStep::Finalized
                | PublicationJournalStep::FinalAdopted
        )
    }));
    assert!(
        WorkItemStore
            .get_publication(&scenario.pool, staged.id)
            .await
            .unwrap()
            .unwrap()
            .published_at
            .is_none()
    );
    assert!(!final_path.exists());
    assert_eq!(1, paths_with_fragment(&storage_root, ".incoming"));
    assert_eq!(1, paths_with_fragment(&storage_root, ".copying"));
    assert_eq!(source_before, source_hashes(&scenario.root));

    let expired = sqlx::query(
        "UPDATE release_work_items
         SET lease_expires_at = now() - interval '1 second'
         WHERE id = $1 AND lease_owner = $2 AND state = 'publishing'",
    )
    .bind(staged.id)
    .bind(&forced_owner)
    .execute(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(1, expired.rows_affected());
    scenario.config.publication_copy_test_barrier = None;
    let retry = publish_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, retry.claimed);
    assert_eq!(1, retry.completed);
    assert_eq!(0, retry.adopted);
    assert_eq!(0, retry.failed);
    assert_eq!(0, retry.errors);
    let published = WorkItemStore.get(&scenario.pool, staged.id).await.unwrap();
    assert_eq!(IngestionState::Published, published.state);
    assert!(published.lease_owner.is_none());
    assert_eq!(attempt_count_before, published.attempt_count);
    let final_journal = WorkItemStore
        .list_publication_journal(&scenario.pool, staged.id)
        .await
        .unwrap();
    for step in [
        PublicationJournalStep::IncomingPrepared,
        PublicationJournalStep::AssetCopied,
        PublicationJournalStep::BundleSealed,
        PublicationJournalStep::Finalized,
    ] {
        assert_eq!(
            1,
            final_journal
                .iter()
                .filter(|entry| entry.step == step)
                .count(),
            "retry must converge with exactly one {step:?} milestone"
        );
    }
    assert!(final_path.join("manifest.json").is_file());
    assert_eq!(0, paths_with_fragment(&storage_root, ".incoming"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".copying"));
    assert_eq!(
        1_i64,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM release_publication_receipts WHERE work_item_id = $1",
        )
        .bind(staged.id)
        .fetch_one(&scenario.pool)
        .await
        .unwrap()
    );
    assert_eq!(source_before, source_hashes(&scenario.root));
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn conflicting_final_is_not_overwritten_and_remains_directly_reclaimable() {
    if !tools_available() {
        eprintln!("skipping publication conflict recovery test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_conflict_reclaim").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_tagged_mp3(&release.join("track.mp3"));
    let storage_root = scenario.enable_publication();
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    scenario.config.publication_concurrency = 1;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;
    assert_eq!(
        1,
        normalize_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );
    assert_eq!(
        1,
        validate_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );
    let staged = WorkItemStore
        .list(&scenario.pool, Some(IngestionState::Staged), 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let attempt_count_before = staged.attempt_count;
    let publication = WorkItemStore
        .get_publication(&scenario.pool, staged.id)
        .await
        .unwrap()
        .unwrap();
    let conflicting_final = storage_root.join(&publication.plan.target_relative_path);
    fs::create_dir_all(&conflicting_final).unwrap();
    fs::write(
        conflicting_final.join("foreign-sentinel"),
        b"must not be overwritten",
    )
    .unwrap();

    let stats = publish_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, stats.claimed);
    assert_eq!(0, stats.completed);
    assert_eq!(1, stats.failed);
    assert_eq!(0, stats.errors);
    let conflicted = WorkItemStore.get(&scenario.pool, staged.id).await.unwrap();
    assert_eq!(IngestionState::Publishing, conflicted.state);
    assert!(conflicted.lease_owner.is_none());
    assert_eq!(attempt_count_before, conflicted.attempt_count);
    assert_eq!(
        Some("publication.target_conflict"),
        conflicted
            .last_error_code
            .as_ref()
            .map(|code| code.as_str())
    );
    let durable_publication = WorkItemStore
        .get_publication(&scenario.pool, staged.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(publication.plan_hash, durable_publication.plan_hash);
    assert!(durable_publication.published_at.is_none());
    let journal = WorkItemStore
        .list_publication_journal(&scenario.pool, staged.id)
        .await
        .unwrap();
    assert_eq!(
        1,
        journal
            .iter()
            .filter(|entry| entry.step == PublicationJournalStep::TargetConflict)
            .count()
    );
    assert_eq!(
        b"must not be overwritten",
        fs::read(conflicting_final.join("foreign-sentinel"))
            .unwrap()
            .as_slice()
    );
    assert_eq!(1, fs::read_dir(&conflicting_final).unwrap().count());

    let recovery_worker = WorkerId::new();
    let reclaimed = WorkItemStore
        .claim_publication(
            &scenario.pool,
            &recovery_worker,
            &WorkEngineConfig {
                claim_batch_size: 1,
                ..WorkEngineConfig::default()
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(1, reclaimed.len());
    assert_eq!(staged.id, reclaimed[0].id);
    assert_eq!(IngestionState::Publishing, reclaimed[0].state);
    assert_eq!(attempt_count_before, reclaimed[0].attempt_count);
    assert_eq!(source_before, source_hashes(&scenario.root));
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_validation_and_publication_are_lease_fenced_to_one_result() {
    if !tools_available() {
        eprintln!("skipping publication fencing test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_fencing").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_tagged_mp3(&release.join("track.mp3"));
    scenario.enable_publication();
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    scenario.config.publication_concurrency = 1;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;
    assert_eq!(
        1,
        normalize_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );

    let now = OffsetDateTime::now_utc();
    let work_config = WorkEngineConfig::default();
    let (left, right) = tokio::join!(
        validate_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &work_config,
            now,
        ),
        validate_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &work_config,
            now,
        )
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(1, left.claimed + right.claimed);
    assert_eq!(1, left.completed + right.completed);
    assert_eq!(0, left.errors + right.errors);

    let (left, right) = tokio::join!(
        publish_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &work_config,
            OffsetDateTime::now_utc(),
        ),
        publish_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &work_config,
            OffsetDateTime::now_utc(),
        )
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(1, left.claimed + right.claimed);
    assert_eq!(1, left.completed + right.completed);
    assert_eq!(0, left.errors + right.errors);
    let (published, receipts, authorizations, plans): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_work_items WHERE state = 'published'),
           (SELECT COUNT(*) FROM release_publication_receipts),
           (SELECT COUNT(*) FROM release_publication_authorizations),
           (SELECT COUNT(*) FROM release_publication_plans)",
    )
    .fetch_one(&scenario.pool)
    .await
    .unwrap();
    assert_eq!((1, 1, 1, 1), (published, receipts, authorizations, plans));
    assert_eq!(source_before, source_hashes(&scenario.root));
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn missing_presentation_is_durably_routed_to_needs_review_before_canonical_io() {
    if !tools_available() {
        eprintln!("skipping durable NeedsReview test: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_needs_review").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_wav(&release.join("tagless.wav"));
    let storage_root = scenario.enable_publication();
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;
    assert_eq!(
        1,
        normalize_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap()
        .completed
    );
    let validation = validate_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(1, validation.claimed);
    assert_eq!(0, validation.completed);
    assert_eq!(1, validation.needs_review);
    assert_eq!(0, validation.failed);
    let item = WorkItemStore
        .list(&scenario.pool, None, 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(IngestionState::NeedsReview, item.state);
    assert_eq!(
        Some("validation.needs_review"),
        item.state_reason.as_deref()
    );
    assert!(
        WorkItemStore
            .get_publication(&scenario.pool, item.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(fs::read_dir(storage_root).unwrap().next().is_none());
    assert_eq!(source_before, source_hashes(&scenario.root));
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn real_mp3_flac_ogg_wav_unicode_artwork_and_sidecars_reach_published() {
    use std::os::unix::fs::symlink;

    if !tools_available() {
        eprintln!("skipping real publication pathology matrix: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("publication_pathologies").await;
    let outside = scenario.root.with_extension("outside-audio");
    fs::write(&outside, b"outside source sentinel").unwrap();
    for extension in ["mp3", "flac", "ogg", "wav"] {
        let release = scenario.root.join(format!("Release Ω {extension}"));
        fs::create_dir(&release).unwrap();
        generate_tagged_audio(
            &release.join(format!("-01 Unicode Ω.{extension}")),
            extension,
        );
        generate_png(&release.join("cover Ω.png"));
        fs::write(
            release.join("disc Ω.cue"),
            format!(
                "TITLE \"Pathology Release\"\nPERFORMER \"Pathology Artist\"\nFILE \"-01 Unicode Ω.{extension}\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Unicode Track\"\n    INDEX 01 00:00:00\n"
            ),
        )
        .unwrap();
        fs::write(release.join("notes Ω.nfo"), b"Pathology notes").unwrap();
        fs::write(
            release.join("playlist Ω.m3u"),
            format!("#EXTM3U\n-01 Unicode Ω.{extension}\n"),
        )
        .unwrap();
        fs::write(
            release.join("lyrics Ω.lrc"),
            b"[ar:Pathology Artist]\n[00:00.00]Synthetic lyric\n",
        )
        .unwrap();
        fs::write(
            release.join("checksums Ω.sfv"),
            format!("-01 Unicode Ω.{extension} DEADBEEF\n"),
        )
        .unwrap();
    }
    symlink(
        &outside,
        scenario.root.join("Release Ω mp3/linked-outside.mp3"),
    )
    .unwrap();
    let storage_root = scenario.enable_publication();
    scenario.config.normalization_concurrency = 4;
    scenario.config.validation_concurrency = 4;
    scenario.config.publication_concurrency = 4;
    let source_before = source_hashes(&scenario.root);
    scenario.discover_settle_and_inspect().await;

    let normalized = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(4, normalized.claimed);
    assert_eq!(4, normalized.completed);
    assert_eq!(0, normalized.failed);
    assert_eq!(0, normalized.errors);
    let validated = validate_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(4, validated.claimed);
    assert_eq!(4, validated.completed);
    assert_eq!(0, validated.needs_review);
    assert_eq!(0, validated.failed);
    let published = publish_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    assert_eq!(4, published.claimed);
    assert_eq!(4, published.completed);
    assert_eq!(0, published.failed);
    assert_eq!(0, published.errors);

    let items = WorkItemStore
        .list(&scenario.pool, Some(IngestionState::Published), 10, 0)
        .await
        .unwrap();
    assert_eq!(4, items.len());
    let mut observed_extensions = BTreeMap::new();
    let mut observed_sidecars = BTreeMap::new();
    let mut canonical_artwork = 0_usize;
    for item in items {
        let stored_manifest = WorkItemStore
            .get_manifest(&scenario.pool, item.id)
            .await
            .unwrap()
            .unwrap();
        let manifest: ReleaseManifest = serde_json::from_value(stored_manifest.payload).unwrap();
        assert_eq!(3, manifest.schema_version);
        for file in &manifest.files {
            if file.kind == melodee_domain::ObservedFileKind::Audio {
                let extension = Path::new(&file.relative_path)
                    .extension()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                *observed_extensions.entry(extension).or_insert(0_usize) += 1;
            }
        }
        for sidecar in &manifest.normalization.as_ref().unwrap().parsed_sidecars {
            *observed_sidecars
                .entry(sidecar.sidecar_type.clone())
                .or_insert(0_usize) += 1;
        }
        let publication = manifest.publication.as_ref().unwrap();
        canonical_artwork += publication
            .canonical_assets
            .iter()
            .filter(|asset| asset.kind == melodee_domain::CanonicalAssetKind::Artwork)
            .count();
        assert!(
            publication
                .canonical_assets
                .iter()
                .all(|asset| asset.source_path != "linked-outside.mp3")
        );
    }
    assert_eq!(
        BTreeMap::from([
            ("flac".to_owned(), 1),
            ("mp3".to_owned(), 1),
            ("ogg".to_owned(), 1),
            ("wav".to_owned(), 1),
        ]),
        observed_extensions
    );
    for sidecar_type in ["cue", "lrc", "m3u", "nfo", "sfv"] {
        assert_eq!(Some(&4), observed_sidecars.get(sidecar_type));
    }
    assert_eq!(4, canonical_artwork);
    assert_eq!(4, regular_files_named(&storage_root, "manifest.json"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".incoming"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".copying"));
    assert_eq!(source_before, source_hashes(&scenario.root));
    assert_eq!(
        b"outside source sentinel",
        fs::read(&outside).unwrap().as_slice()
    );
    scenario.finish().await;
    fs::remove_file(outside).unwrap();
}
