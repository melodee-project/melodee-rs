//! vertical normalization/publication, concurrency, lease, and fault scenarios.
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
    normalize_once, owned_source_root, publish_canonical_bundle, publish_once,
    reconcile_all_sources, start_inspection_loop, start_normalization_loop, start_publication_loop,
    start_validation_loop, validate_once,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: sqlx::PgPool,
    database: Option<common::TestDb>,
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
            database: Some(database),
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
                None,
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

    async fn finish(mut self) {
        self.finish_inner().await;
    }

    async fn finish_inner(&mut self) {
        let work_area = self.config.work_area_base.clone();
        let storage_root = self.config.storage_root.clone();
        let root = self.root.clone();
        if let Some(database) = self.database.take() {
            self.pool.close().await;
            database.drop().await;
        }
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(work_area);
        if let Some(storage_root) = storage_root {
            let _ = fs::remove_dir_all(storage_root);
        }
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        let Some(database) = self.database.take() else {
            return;
        };
        let pool = self.pool.clone();
        let work_area = self.config.work_area_base.clone();
        let storage_root = self.config.storage_root.clone();
        let root = self.root.clone();
        let _ = std::thread::Builder::new()
            .name("scenario-cleanup".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("cleanup runtime");
                runtime.block_on(async move {
                    // Bound the database cleanup: `PgPool::close` waits for
                    // checked-out connections, and a panicking test can leave
                    // leases held by tasks that will never return them. An
                    // unbounded wait here previously stalled the whole test
                    // process (and with it the repository gate) after any
                    // mid-scenario panic. Leaking one `melodee_test_*`
                    // database on a failure path is strictly better than
                    // hanging the suite.
                    let cleanup = async {
                        pool.close().await;
                        database.drop().await;
                    };
                    if tokio::time::timeout(std::time::Duration::from_secs(20), cleanup)
                        .await
                        .is_err()
                    {
                        eprintln!(
                            "scenario cleanup timed out; leaking the test database rather than blocking the suite"
                        );
                    }
                });
                let _ = fs::remove_dir_all(root);
                let _ = fs::remove_dir_all(work_area);
                if let Some(storage_root) = storage_root {
                    let _ = fs::remove_dir_all(storage_root);
                }
            })
            .map(|handle| handle.join());
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

/// ADR-0145 drains each acquired Release out of inbound into durable staging
/// at `<staging-root>/releases/<work-item-id>/source`, so the inbound tree is
/// empty after inspection. This mirrors `source_hashes` over the staged source
/// of every work item (keyed by the original `source_path/<file>`) so a test
/// can still assert the source bytes survive the pipeline unchanged.
async fn staged_source_hashes(
    config: &IngestionConfig,
    pool: &sqlx::PgPool,
) -> BTreeMap<PathBuf, String> {
    let mut hashes = BTreeMap::new();
    let items = WorkItemStore
        .list(pool, None, 1000, 0)
        .await
        .expect("list work items");
    for item in items {
        let owned = owned_source_root(config, &WorkItemStore, pool, &item)
            .await
            .expect("staged source");
        for file in fs::read_dir(&owned.root).expect("staged dir") {
            let file = file.expect("dir entry").path();
            if file.is_file() {
                let rel = file.strip_prefix(&owned.root).expect("strip prefix");
                hashes.insert(
                    Path::new(&item.source_path).join(rel),
                    blake3::hash(&fs::read(&file).expect("read file"))
                        .to_hex()
                        .to_string(),
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

async fn phase_diagnostics(pool: &sqlx::PgPool) -> String {
    let state_counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT state, COUNT(*)
         FROM release_work_items
         GROUP BY state
         ORDER BY state",
    )
    .fetch_all(pool)
    .await
    .expect("state counts");
    let leased_counts: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT state, lease_owner, COUNT(*)
         FROM release_work_items
         WHERE lease_owner IS NOT NULL
         GROUP BY state, lease_owner
         ORDER BY state, lease_owner",
    )
    .fetch_all(pool)
    .await
    .expect("lease counts");
    let event_counts: Vec<(Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT from_state, to_state, COUNT(*)
         FROM release_work_item_events
         GROUP BY from_state, to_state
         ORDER BY from_state, to_state",
    )
    .fetch_all(pool)
    .await
    .expect("event counts");

    format!("states={state_counts:?}; leases={leased_counts:?}; events={event_counts:?}")
}

struct BatchProgress {
    claimed: u64,
    completed: u64,
    needs_review: u64,
    adopted: u64,
    failed: u64,
    errors: u64,
    total: u64,
    target: u64,
}

async fn assert_batch_progress(
    phase: &str,
    pool: &sqlx::PgPool,
    progress: &BatchProgress,
    extra: &str,
) {
    let BatchProgress {
        claimed,
        completed,
        needs_review,
        adopted,
        failed,
        errors,
        total,
        target,
    } = *progress;
    assert_eq!(
        claimed,
        completed + needs_review + failed + errors,
        "{phase} batch accounting mismatch: claimed={claimed} completed={completed} needs_review={needs_review} adopted={adopted} failed={failed} errors={errors} total={total} target={target}; {extra}"
    );
    assert!(
        adopted <= completed,
        "{phase} batch reported adopted={adopted} greater than completed={completed}: claimed={claimed} needs_review={needs_review} failed={failed} errors={errors} total={total} target={target}; {extra}"
    );
    assert_eq!(
        0, failed,
        "{phase} batch reported unexpected failures: claimed={claimed} completed={completed} needs_review={needs_review} adopted={adopted} failed={failed} errors={errors} total={total} target={target}; {extra}"
    );
    assert_eq!(
        0, errors,
        "{phase} batch reported unexpected errors: claimed={claimed} completed={completed} needs_review={needs_review} adopted={adopted} failed={failed} errors={errors} total={total} target={target}; {extra}"
    );
    if claimed == 0 && total < target {
        let diagnostics = phase_diagnostics(pool).await;
        panic!(
            "{phase} stalled before reaching {target}: claimed={claimed} completed={completed} needs_review={needs_review} adopted={adopted} failed={failed} errors={errors} total={total}; {extra}; {diagnostics}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn stage_workers_drain_their_own_full_batch_before_the_idle_tick() {
    let mut scenario = Scenario::new("stage_drain").await;
    scenario.enable_publication();
    scenario.config.reconciliation_interval = Duration::minutes(5);
    scenario.config.inspection_concurrency = 1;
    scenario.config.normalization_concurrency = 1;
    scenario.config.validation_concurrency = 1;
    scenario.config.publication_concurrency = 1;
    for name in ["first", "second"] {
        let release = scenario.root.join(name);
        fs::create_dir(&release).unwrap();
        generate_tagged_mp3(&release.join("track.mp3"));
    }
    for now in [common::T0, common::T0 + Duration::seconds(1)] {
        reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, now)
            .await
            .unwrap();
    }

    let work = WorkEngineConfig {
        claim_batch_size: 4,
        ..WorkEngineConfig::default()
    };
    for target in [
        IngestionState::Normalizing,
        IngestionState::Validating,
        IngestionState::Staged,
        IngestionState::Published,
    ] {
        let config = scenario.config.clone();
        let pool = scenario.pool.clone();
        let worker = match target {
            IngestionState::Normalizing => {
                start_inspection_loop(config, pool, WorkItemStore, work, None)
            }
            IngestionState::Validating => {
                start_normalization_loop(config, pool, WorkItemStore, work)
            }
            IngestionState::Staged => start_validation_loop(config, pool, WorkItemStore, work),
            IngestionState::Published => start_publication_loop(config, pool, WorkItemStore, work),
            _ => unreachable!(),
        };
        let deadline = std::time::Duration::from_secs(30);
        let drained = tokio::time::timeout(deadline, async {
            loop {
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM release_work_items WHERE state = $1")
                        .bind(target.as_str())
                        .fetch_one(&scenario.pool)
                        .await
                        .unwrap();
                if count == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await;
        tokio::time::timeout(deadline, worker.stop())
            .await
            .expect("worker stopped");
        assert!(
            drained.is_ok(),
            "second Release should reach {target:?} before the idle tick"
        );
    }
    scenario.finish().await;
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

    // ADR-0145 drain proof at the strongest vertical: every acquired
    // candidate directory left inbound after its verified staging receipt.
    let remaining_candidates = fs::read_dir(&scenario.root)
        .unwrap()
        .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
        .count();
    assert_eq!(0, remaining_candidates, "inbound drained after acquisition");
    let (receipts, drained): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE status = 'inbound_removed' AND tree_hash IS NOT NULL)
         FROM release_work_item_staging_handoffs",
    )
    .fetch_one(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(
        (200, 200),
        (receipts, drained),
        "complete acquisition receipts"
    );

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
        assert_batch_progress(
            "normalization",
            &scenario.pool,
            &BatchProgress {
                claimed: stats.claimed,
                completed: stats.completed,
                needs_review: 0,
                adopted: 0,
                failed: stats.failed,
                errors: stats.errors,
                total: normalized,
                target: 200,
            },
            "normalization batches must complete without errors",
        )
        .await;
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );

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
        assert_batch_progress(
            "validation",
            &scenario.pool,
            &BatchProgress {
                claimed: stats.claimed,
                completed: stats.completed,
                needs_review: stats.needs_review,
                adopted: 0,
                failed: stats.failed,
                errors: stats.errors,
                total: validated + stats.completed + stats.needs_review,
                target: 200,
            },
            "validation batches must complete without errors",
        )
        .await;
        validated += stats.completed;
        validation_review += stats.needs_review;
        validation_failures += stats.failed;
        validation_errors += stats.errors;
        if stats.claimed == 0 {
            if validated != 200 {
                let diagnostics = phase_diagnostics(&scenario.pool).await;
                panic!(
                    "validation stalled before reaching 200: validated={validated} review={validation_review} failed={validation_failures} errors={validation_errors}; {diagnostics}"
                );
            }
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
        assert_batch_progress(
            "publication",
            &scenario.pool,
            &BatchProgress {
                claimed: stats.claimed,
                completed: stats.completed,
                needs_review: 0,
                adopted: stats.adopted,
                failed: stats.failed,
                errors: stats.errors,
                total: published + stats.completed,
                target: 200,
            },
            "publication batches must complete without errors",
        )
        .await;
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );

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

    // Reconciliation after complete processing sees zero historical
    // candidates: acquired occurrences left inbound and are inactive.
    let rescan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("post-publish reconcile");
    let (rescan_seen, rescan_new): (u64, u64) = rescan
        .iter()
        .map(|result| (result.stats.candidates_seen, result.stats.new_work_items))
        .fold((0, 0), |acc, row| (acc.0 + row.0, acc.1 + row.1));
    assert_eq!(
        (0, 0),
        (rescan_seen, rescan_new),
        "empty reconciliation after inbound drain and publication"
    );
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

/// Expires every outstanding work-item lease, simulating the ungraceful
/// death of whichever worker held it (no release, no failure record). The
/// lease/fencing model must make every such item reclaimable by a fresh
/// worker; returns how many leases were terminated.
async fn expire_outstanding_leases(pool: &sqlx::PgPool) -> u64 {
    sqlx::query(
        "UPDATE release_work_items
         SET lease_expires_at = now() - interval '1 second'
         WHERE lease_owner IS NOT NULL",
    )
    .execute(pool)
    .await
    .expect("expire outstanding leases")
    .rows_affected()
}

/// repeated-termination vertical: 200 releases cross acquisition,
/// inspection, normalization, validation, and publication while workers
/// die ungracefully between every stage boundary. Each termination round
/// claims a bounded batch with a fresh worker and abandons it (no
/// completion, no release); the next pass must reclaim the expired leases
/// and converge with exactly-once semantics. Indexing termination is
/// covered at its own service boundary by the search crash-window fencing
/// tests (`melodee-search/tests/rebuild_integration.rs`), which prove an
/// interrupted rebuild never activates a partial generation.
#[cfg(unix)]
#[tokio::test]
async fn two_hundred_releases_survive_repeated_termination_across_all_stages() {
    if !tools_available() {
        eprintln!("skipping repeated-termination acceptance: media tools unavailable");
        return;
    }
    let mut scenario = Scenario::new("normalization_termination_200").await;
    let fixture = scenario.root.join("fixture.mp3");
    generate_tagged_mp3(&fixture);
    for index in 0..200 {
        let release = scenario.root.join(format!("release-{index:03}"));
        fs::create_dir(&release).unwrap();
        fs::copy(&fixture, release.join("track.mp3")).unwrap();
    }
    fs::remove_file(fixture).unwrap();
    let storage_root = scenario.enable_publication();
    let source_before = source_hashes(&scenario.root);
    let started = Instant::now();
    let mut terminated_claims = 0_u64;
    let kill_engine = WorkEngineConfig {
        claim_batch_size: 3,
        ..WorkEngineConfig::default()
    };

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

    // Stage 1+2: acquisition handoff and inspection. Three termination
    // rounds abandon claimed ready items before the drain.
    for _ in 0..3 {
        let doomed = WorkerId::new();
        let abandoned = WorkItemStore
            .claim(
                &scenario.pool,
                &doomed,
                &kill_engine,
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("termination-round claim");
        drop(abandoned);
        terminated_claims += expire_outstanding_leases(&scenario.pool).await;
    }
    let mut inspected = 0_u64;
    loop {
        let stats = inspect_ready_once(
            &scenario.config,
            &WorkItemStore,
            &scenario.pool,
            &WorkEngineConfig::default(),
            OffsetDateTime::now_utc(),
            None,
        )
        .await
        .expect("inspection batch");
        inspected += stats.completed;
        assert_eq!(0, stats.failed, "inspection failures");
        assert_eq!(0, stats.errors, "inspection errors");
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, inspected);
    let remaining_candidates = fs::read_dir(&scenario.root)
        .unwrap()
        .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.path().is_dir()))
        .count();
    assert_eq!(0, remaining_candidates, "inbound drained after acquisition");

    // Stage 3: normalization with termination rounds between drain passes.
    for _ in 0..3 {
        let doomed = WorkerId::new();
        let abandoned = WorkItemStore
            .claim_normalizing(
                &scenario.pool,
                &doomed,
                &kill_engine,
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("normalization termination-round claim");
        drop(abandoned);
        terminated_claims += expire_outstanding_leases(&scenario.pool).await;
    }
    let mut normalized = 0_u64;
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
        assert_eq!(0, stats.failed, "normalization failures");
        assert_eq!(0, stats.errors, "normalization errors");
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, normalized);

    // Stage 4: validation.
    for _ in 0..3 {
        let doomed = WorkerId::new();
        let abandoned = WorkItemStore
            .claim_validating(
                &scenario.pool,
                &doomed,
                &kill_engine,
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("validation termination-round claim");
        drop(abandoned);
        terminated_claims += expire_outstanding_leases(&scenario.pool).await;
    }
    let mut validated = 0_u64;
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
        assert_eq!(0, stats.needs_review, "validation review");
        assert_eq!(0, stats.failed, "validation failures");
        assert_eq!(0, stats.errors, "validation errors");
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, validated);

    // Stage 5: publication.
    for _ in 0..3 {
        let doomed = WorkerId::new();
        let abandoned = WorkItemStore
            .claim_publication(
                &scenario.pool,
                &doomed,
                &kill_engine,
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("publication termination-round claim");
        drop(abandoned);
        terminated_claims += expire_outstanding_leases(&scenario.pool).await;
    }
    let mut published = 0_u64;
    let mut adopted = 0_u64;
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
        assert_eq!(0, stats.failed, "publication failures");
        assert_eq!(0, stats.errors, "publication errors");
        if stats.claimed == 0 {
            break;
        }
    }
    assert_eq!(200, published);
    assert_eq!(0, adopted);

    // Exactly-once authoritative results after every termination round.
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
    let (handoffs, manifests, duplicate_manifests, receipts, published_transitions, leased): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM release_work_item_staging_handoffs
            WHERE status = 'inbound_removed' AND tree_hash IS NOT NULL),
           (SELECT COUNT(*) FROM release_work_item_manifests),
           (SELECT COUNT(*) - COUNT(DISTINCT work_item_id) FROM release_work_item_manifests),
           (SELECT COUNT(*) FROM release_publication_receipts),
           (SELECT COUNT(*) FROM release_work_item_events
            WHERE from_state = 'publishing' AND to_state = 'published'),
           (SELECT COUNT(*) FROM release_work_items WHERE lease_owner IS NOT NULL)",
    )
    .fetch_one(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(
        (200, 200, 0, 200, 200, 0),
        (
            handoffs,
            manifests,
            duplicate_manifests,
            receipts,
            published_transitions,
            leased
        ),
        "exactly-once state after repeated termination; {}",
        phase_diagnostics(&scenario.pool).await
    );
    let final_states: Vec<(String, i64)> = sqlx::query_as(
        "SELECT state, COUNT(*) FROM release_work_items GROUP BY state ORDER BY state",
    )
    .fetch_all(&scenario.pool)
    .await
    .unwrap();
    assert_eq!(vec![("published".to_owned(), 200)], final_states);
    assert_eq!(200, regular_files_named(&storage_root, "manifest.json"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".incoming"));
    assert_eq!(0, paths_with_fragment(&storage_root, ".copying"));

    let rescan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("post-publish reconcile");
    let (rescan_seen, rescan_new): (u64, u64) = rescan
        .iter()
        .map(|result| (result.stats.candidates_seen, result.stats.new_work_items))
        .fold((0, 0), |acc, row| (acc.0 + row.0, acc.1 + row.1));
    assert_eq!(
        (0, 0),
        (rescan_seen, rescan_new),
        "empty reconciliation after inbound drain and publication"
    );

    let report = format!(
        "{{\"scenario\":\"repeated-termination-200\",\"releases\":200,\"terminated_claims\":{terminated_claims},\"normalized\":{normalized},\"validated\":{validated},\"published\":{published},\"receipts\":{receipts},\"lost\":0,\"duplicate_authoritative_result\":0,\"elapsed_ms\":{}}}",
        started.elapsed().as_millis()
    );
    eprintln!("r2.7-repeated-termination={report}");
    if std::env::var_os("MELODEE_WRITE_PERF_EVIDENCE").is_some() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/repeated-termination-200.json"
        );
        std::fs::write(path, format!("{report}\n")).expect("write termination evidence");
    }
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
        // ADR-0145 renames each Release into staging during inspection, so the
        // inbound directory name is no longer in the fpcalc input path. Mark
        // the failing sibling by its audio filename instead.
        let audio = if name == "bad" {
            "track-fail.wav"
        } else {
            "track.wav"
        };
        generate_wav(&release.join(audio));
    }
    scenario.discover_settle_and_inspect().await;
    let real_fpcalc = tool_path("fpcalc");
    let wrapper = scenario.root.join("selective-fpcalc");
    write_script(
        &wrapper,
        &format!(
            "if [ \"$1\" = '-version' ]; then exec '{}' \"$@\"; fi\ncase \"$5\" in *track-fail*) printf 'synthetic failure' >&2; exit 17;; esac\nexec '{}' \"$@\"",
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
    // ADR-0145 moves the Release into staging during inspection, so the inbound
    // `release` directory is gone; the source-change must target the durable
    // staged tree that normalization actually reads and re-verifies.
    let change_item = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.source_path == "release")
        .unwrap();
    let staged_root = owned_source_root(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &change_item,
    )
    .await
    .unwrap()
    .root;

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
    fs::write(staged_root.join("late.nfo"), b"late change").unwrap();
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );

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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
    scenario.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unpersistable_normalized_text_stops_reencoding_after_lease_expiry() {
    if !tools_available() {
        eprintln!("skipping manifest failure test: media tools unavailable");
        return;
    }
    let scenario = Scenario::new("invalid_manifest_text").await;
    let release = scenario.root.join("release");
    fs::create_dir(&release).unwrap();
    generate_tagged_mp3(&release.join("track.mp3"));
    scenario.discover_settle_and_inspect().await;

    // Inject the same PostgreSQL data exception as an unrepresentable JSONB
    // string, independently of which metadata source produced the manifest.
    sqlx::raw_sql("CREATE FUNCTION reject_normalized_text() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'invalid manifest text' USING ERRCODE = '22P05'; END $$;
        CREATE TRIGGER invalid_manifest BEFORE UPDATE OF manifest_payload ON release_work_item_manifests
        FOR EACH ROW EXECUTE FUNCTION reject_normalized_text()")
        .execute(&scenario.pool).await.unwrap();
    let now = OffsetDateTime::now_utc();
    let config = WorkEngineConfig::default();
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &config,
        now,
    )
    .await
    .unwrap();
    assert_eq!((stats.claimed, stats.failed, stats.errors), (1, 1, 0));
    let failed = WorkItemStore
        .list(&scenario.pool, Some(IngestionState::Failed), 1, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        failed.last_error_code.as_ref().map(|code| code.as_str()),
        Some("normalization.invalid_manifest_text")
    );
    assert!(failed.lease_owner.is_none() && failed.next_attempt_at.is_none());
    let later = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &config,
        now + config.lease_duration * 2,
    )
    .await
    .unwrap();
    assert_eq!(later.claimed, 0);
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
        let playlist = format!("#EXTM3U\r\n-01 Unicode Ω.{extension}\r\n");
        let playlist_bytes: Vec<u8> = std::iter::once(0xfeff)
            .chain(playlist.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        fs::write(release.join("playlist Ω.m3u"), playlist_bytes).unwrap();
        fs::write(release.join("broken.m3u"), b"bad\0path").unwrap();
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
        assert!(
            manifest
                .normalization
                .as_ref()
                .unwrap()
                .findings
                .iter()
                .any(
                    |f| f.code == "sidecar.parse_failed" && f.file.as_deref() == Some("broken.m3u")
                )
        );
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
            if sidecar.sidecar_type == "m3u" {
                let audio = manifest
                    .files
                    .iter()
                    .find(|file| file.kind == melodee_domain::ObservedFileKind::Audio)
                    .unwrap();
                assert_eq!(sidecar.parsed["entries"][0]["path"], audio.relative_path);
                assert_eq!(sidecar.parsed["entries"].as_array().unwrap().len(), 1);
            }
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
    assert_eq!(
        source_before,
        staged_source_hashes(&scenario.config, &scenario.pool).await
    );
    assert_eq!(
        b"outside source sentinel",
        fs::read(&outside).unwrap().as_slice()
    );
    scenario.finish().await;
    fs::remove_file(outside).unwrap();
}
