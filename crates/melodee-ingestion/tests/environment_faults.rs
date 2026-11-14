//! Environment fault scenarios (PRD-TEST-005, PRD-TEST-007): full
//! staging disk during acquisition, slow storage during acquisition, hung
//! media tool, and media-tool ENOSPC with partial output. Each scenario
//! proves fail-closed behavior — no lost durable work, no partial
//! authoritative state, no destructive uncertainty handling — and that the
//! supported retry path converges after the fault clears.
//!
//! Staging-disk and slow-storage faults are injected through the one-shot
//! `test-support` acquisition write hook (the same restart semantics as the
//! crash-matrix points). Media-tool faults use wrapper scripts on the
//! configured tool path, which is the documented operator override seam; no
//! production behavior changes.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use melodee_ingestion::{
    AcquisitionTestHook, AcquisitionWriteFault, InboundSource, IngestionConfig,
    acquire_owned_source, inspect_ready_once, normalize_once, reconcile_all_sources,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const T0: time::OffsetDateTime = common::T0;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: PgPool,
    database: common::TestDb,
}

impl Scenario {
    async fn new(name: &str) -> Self {
        let unique = Uuid::now_v7();
        let root = std::env::temp_dir().join(format!("melodee-envfault-{name}-{unique}"));
        fs::create_dir_all(&root).expect("root");
        let database = common::TestDb::create(name).await;
        let source =
            InboundSource::from_root(root.clone(), None, Some(name.to_owned())).expect("source");
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.settling_quiet_period = Duration::ZERO;
        // `with_sources` derives the staging root as `<inbound-parent>/
        // `.melodee-staging-test`, which is shared by every parallel test
        // using a /tmp inbound root. Each scenario needs its own staging,
        // staging cleanup, and work roots to stay isolated under `cargo
        // test` parallelism.
        config.staging_root = Some(root.with_extension("staging"));
        config.work_area_base = root.with_extension("work");
        Self {
            root,
            config,
            pool: database.pool.clone(),
            database,
        }
    }

    fn staging_root(&self) -> PathBuf {
        self.config.staging_root.clone().expect("staging root")
    }

    fn staged_source(&self, item_id: Uuid) -> PathBuf {
        self.staging_root()
            .join("releases")
            .join(item_id.to_string())
            .join("source")
    }

    fn partial_source(&self, item_id: Uuid) -> PathBuf {
        self.staging_root()
            .join("releases")
            .join(format!(".{item_id}.acquiring"))
    }

    /// Writes a fixture tree to inbound and settles it to `ready`.
    async fn settle(&self, name: &str, files: &[(&str, &[u8])]) -> Uuid {
        let release = self.root.join(name);
        for (relative, bytes) in files {
            let path = release.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, bytes).expect("file");
        }
        reconcile_all_sources(&self.config, &WorkItemStore, &self.pool, T0)
            .await
            .expect("discover");
        reconcile_all_sources(
            &self.config,
            &WorkItemStore,
            &self.pool,
            T0 + Duration::seconds(1),
        )
        .await
        .expect("settle");
        WorkItemStore
            .list(&self.pool, None, 50, 0)
            .await
            .expect("list")
            .into_iter()
            .find(|item| item.source_path == name)
            .expect("ready item")
            .id
    }

    async fn item_state(&self, id: Uuid) -> String {
        sqlx::query_scalar::<_, String>("SELECT state FROM release_work_items WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("state")
    }

    async fn last_error(&self, id: Uuid) -> (String, String) {
        sqlx::query_as::<_, (String, String)>(
            "SELECT last_error_code, last_error_message FROM release_work_items WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .expect("error")
    }

    async fn handoff(&self, item_id: Uuid) -> Option<(String, Option<String>, Option<String>)> {
        sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT status, method, tree_hash FROM release_work_item_staging_handoffs
             WHERE work_item_id = $1",
        )
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await
        .expect("handoff")
    }

    async fn drop(self) {
        let root = self.root.clone();
        let staging = self.config.staging_root.clone();
        let work = self.config.work_area_base.clone();
        self.database.drop().await;
        let _ = fs::remove_dir_all(root);
        if let Some(staging) = staging {
            let _ = fs::remove_dir_all(staging);
        }
        let _ = fs::remove_dir_all(work);
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

/// Writes an executable wrapper script and returns its path.
#[cfg(unix)]
fn write_wrapper(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(dir).expect("wrapper dir");
    let path = dir.join(name);
    fs::write(&path, body).expect("wrapper script");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}

/// Claims one ready item, mirroring the crash-matrix driving pattern.
async fn claim_one(
    scenario: &Scenario,
    worker: &WorkerId,
    now: time::OffsetDateTime,
) -> melodee_domain::ReleaseWorkItem {
    let engine = WorkEngineConfig {
        claim_batch_size: 1,
        ..WorkEngineConfig::default()
    };
    WorkItemStore
        .claim(&scenario.pool, worker, &engine, now)
        .await
        .expect("claim")
        .into_iter()
        .next()
        .expect("one ready item")
}

/// Runs acquisition directly against a claimed item. The same claimed item
/// (and its held lease) can be reused across attempts, exactly like the
/// crash matrix: a crashed attempt does not release the worker's lease.
async fn acquire(
    scenario: &Scenario,
    worker: &WorkerId,
    item: &melodee_domain::ReleaseWorkItem,
    now: time::OffsetDateTime,
) -> Result<melodee_ingestion::OwnedSource, melodee_ingestion::AcquisitionError> {
    let expected = WorkItemStore
        .latest_snapshot(&scenario.pool, item.id)
        .await
        .expect("snapshot")
        .map(|snapshot| snapshot.signature);
    acquire_owned_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        worker,
        item,
        expected.as_deref(),
        now,
    )
    .await
}

/// A full staging disk fails the verified copy with an ordinary I/O
/// error attributed to the destination; the acquisition fails closed with no
/// durable receipt, no final authoritative tree, and inbound retained. After
/// the operator frees space, the supported retry converges with a complete
/// byte-identical staged tree and drained inbound.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn full_staging_disk_fails_closed_and_retry_converges() {
    let mut scenario = Scenario::new("envfault-staging-enospc").await;
    let release = "release-enospc";
    let payload: &[u8] = b"environment-fault-staging-enospc";
    scenario
        .settle(release, &[("disc1/track.flac", payload)])
        .await;
    let item_id = WorkItemStore
        .list(&scenario.pool, None, 50, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == release)
        .expect("item")
        .id;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    scenario.config.acquisition_test_hook = Some(
        AcquisitionTestHook::new()
            .forcing_copy()
            .with_write_fault(AcquisitionWriteFault::NoSpaceLeft),
    );
    let item = claim_one(&scenario, &worker, now).await;
    assert_eq!(item_id, item.id);
    let error = acquire(&scenario, &worker, &item, now)
        .await
        .expect_err("ENOSPC must fail the attempt");
    let message = error.to_string();
    assert!(
        message.contains("No space left on device"),
        "destination write error must surface the OS cause: {message}"
    );
    assert!(
        message.contains(".acquiring"),
        "error must point at the hidden attempt tree, never the final path: {message}"
    );

    // Fail-closed invariants: intent only, no receipt, no final tree, and
    // inbound untouched. Hidden attempt scratch from the crashed copy may
    // remain; it is attempt-isolated under staging and is reclaimed by the
    // retried attempt, exactly like a crash boundary.
    let (status, method, tree_hash) = scenario.handoff(item_id).await.expect("durable intent");
    assert_eq!("intent", status);
    assert!(
        method.is_none() && tree_hash.is_none(),
        "no receipt recorded"
    );
    assert!(!scenario.staged_source(item_id).exists(), "no final tree");
    assert!(
        scenario.root.join(release).exists(),
        "inbound retained for recovery"
    );

    // Operator frees space; the retried attempt converges through the same
    // forced-copy path (the one-shot fault does not re-arm).
    scenario.config.acquisition_test_hook = Some(AcquisitionTestHook::new().forcing_copy());
    acquire(&scenario, &worker, &item, now + Duration::seconds(30))
        .await
        .unwrap_or_else(|error| panic!("retry after ENOSPC converges: {error}"));
    let (status, method, tree_hash) = scenario.handoff(item_id).await.expect("receipt");
    assert_eq!("inbound_removed", status);
    assert_eq!(Some("verified_copy".to_owned()), method);
    assert!(tree_hash.is_some());
    assert_eq!(
        fs::read(scenario.staged_source(item_id).join("disc1/track.flac")).expect("staged bytes"),
        payload,
        "staged tree byte-identical after retry"
    );
    assert!(
        !scenario.root.join(release).exists(),
        "inbound drained after verified retry"
    );
    assert!(
        !scenario.partial_source(item_id).exists(),
        "crashed attempt scratch reclaimed by the retried attempt"
    );
    scenario.drop().await;
}

/// Slow storage delays every destination write of the verified copy.
/// Acquisition remains correct — bounded streaming, byte-identical result,
/// durable receipt, inbound drained — and the delay is observable in wall
/// time. The delay hook is applied on the retried attempt as well to prove
/// the slow path, not just the fast path, is durable.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn slow_storage_acquisition_converges_durably() {
    let mut scenario = Scenario::new("envfault-slow-storage").await;
    let release = "release-slow";
    // 2 MiB across two files so the copy performs multiple buffered writes.
    let big = vec![0x5A_u8; 1 << 20];
    scenario
        .settle(
            release,
            &[
                ("disc1/a.flac", big.as_slice()),
                ("disc1/b.flac", big.as_slice()),
            ],
        )
        .await;
    let item_id = WorkItemStore
        .list(&scenario.pool, None, 50, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == release)
        .expect("item")
        .id;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    scenario.config.acquisition_test_hook = Some(
        AcquisitionTestHook::new()
            .forcing_copy()
            .delaying_copy_writes(std::time::Duration::from_millis(10)),
    );
    let item = claim_one(&scenario, &worker, now).await;
    assert_eq!(item_id, item.id);
    let started = Instant::now();
    let owned = acquire(&scenario, &worker, &item, now)
        .await
        .unwrap_or_else(|error| panic!("slow storage acquisition converges: {error}"));
    let elapsed = started.elapsed();

    assert_eq!(scenario.staged_source(item_id), owned.root);
    assert_eq!(
        big,
        fs::read(owned.root.join("disc1/a.flac")).expect("staged a.flac"),
        "byte-identical through the slow path"
    );
    assert_eq!(
        big,
        fs::read(owned.root.join("disc1/b.flac")).expect("staged b.flac"),
    );
    let (status, method, tree_hash) = scenario.handoff(item_id).await.expect("receipt");
    assert_eq!("inbound_removed", status);
    assert_eq!(Some("verified_copy".to_owned()), method);
    assert!(tree_hash.is_some());
    assert!(!scenario.root.join(release).exists(), "inbound drained");
    assert!(
        elapsed >= std::time::Duration::from_millis(20),
        "write delay must be observable (took {elapsed:?})"
    );
    scenario.drop().await;
}

/// PostgreSQL unavailability (the restart window where connections
/// are refused) fails the handoff attempt closed at a persistence boundary:
/// no durable intent, no staged tree, inbound retained. After the database
/// returns, a fresh claim converges with a complete receipt and drained
/// inbound. Mid-boundary process death is covered by the crash matrix;
/// this scenario covers the service-dependency fault itself.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn postgres_unavailable_during_handoff_fails_closed_and_recovers() {
    let mut scenario = Scenario::new("envfault-pg-restart").await;
    let release = "release-pg-restart";
    let payload: &[u8] = b"environment-fault-pg-restart";
    scenario
        .settle(release, &[("disc1/track.flac", payload)])
        .await;
    let item_id = WorkItemStore
        .list(&scenario.pool, None, 50, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == release)
        .expect("item")
        .id;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    scenario.config.acquisition_test_hook = Some(AcquisitionTestHook::new().forcing_copy());
    let item = claim_one(&scenario, &worker, now).await;
    assert_eq!(item_id, item.id);

    // Pre-fetch the settling signature, then enter the restart window:
    // every pooled connection is gone and new checkouts fail, exactly as
    // when PostgreSQL refuses connections mid-restart. The failure must
    // land inside acquisition at a persistence boundary.
    let expected = WorkItemStore
        .latest_snapshot(&scenario.pool, item.id)
        .await
        .expect("snapshot")
        .map(|snapshot| snapshot.signature);
    scenario.pool.close().await;
    let error = acquire_owned_source(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &worker,
        &item,
        expected.as_deref(),
        now,
    )
    .await
    .expect_err("acquisition must fail while PostgreSQL is unavailable");
    assert!(
        matches!(error, melodee_ingestion::AcquisitionError::Persistence(_)),
        "persistence boundary failure, not a filesystem or source error: {error}"
    );

    // Observe durable state during the outage through a fresh connection:
    // no receipt, no final tree, inbound retained. (The database itself is
    // unchanged by the restart; only the application's pool was refused.)
    let url = common::database_url(&scenario.database.name);
    scenario.pool = PgPool::connect(&url).await.expect("observe pool");
    match scenario.handoff(item_id).await {
        None => {}
        Some((_, method, tree_hash)) => {
            // The intent insert may have committed just before the pool
            // drained; either way no receipt may exist.
            assert!(
                method.is_none() && tree_hash.is_none(),
                "no acquisition receipt may be recorded during the outage"
            );
        }
    }
    assert!(!scenario.staged_source(item_id).exists(), "no final tree");
    assert!(
        scenario.root.join(release).exists(),
        "inbound retained through the outage"
    );

    // PostgreSQL is back: re-claim at a time past the crashed worker's
    // lease expiry, then converge.
    let recovered_worker = WorkerId::new();
    let item = claim_one(&scenario, &recovered_worker, now + Duration::hours(2)).await;
    assert_eq!(item_id, item.id, "same durable work item, no duplicate");
    acquire(
        &scenario,
        &recovered_worker,
        &item,
        now + Duration::hours(2),
    )
    .await
    .unwrap_or_else(|error| panic!("post-restart acquisition converges: {error}"));
    let (status, method, tree_hash) = scenario.handoff(item_id).await.expect("receipt");
    assert_eq!("inbound_removed", status);
    assert_eq!(Some("verified_copy".to_owned()), method);
    assert!(tree_hash.is_some());
    assert_eq!(
        fs::read(scenario.staged_source(item_id).join("disc1/track.flac")).expect("staged bytes"),
        payload,
        "staged tree byte-identical after recovery"
    );
    assert!(!scenario.root.join(release).exists(), "inbound drained");
    scenario.drop().await;
}

/// A hung FFmpeg is killed at the configured timeout with an
/// actionable, typed failure; no orphan process survives; the durable
/// staged source is untouched; and the supported operator retry path
/// (failed -> ready -> inspect -> normalizing) converges to `validating`.
#[cfg(unix)]
#[tokio::test]
async fn hung_ffmpeg_times_out_actionably_and_retry_converges() {
    if !tools_available() {
        eprintln!("ffmpeg/ffprobe/fpcalc unavailable; skipping");
        return;
    }
    let unique = Uuid::now_v7();
    let mut scenario = Scenario::new("envfault-hung-ffmpeg").await;
    let wrapper_dir = std::env::temp_dir().join(format!("melodee-hung-ffmpeg-{unique}"));
    let real_ffmpeg = tool_path("ffmpeg");
    // Unique interval doubles as the pgrep marker on the process cmdline.
    let marker = "sleep 12345";
    let body = format!(
        "#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then exec \"{}\" -version; fi\nexec sleep 12345\n",
        real_ffmpeg.display()
    );
    scenario.config.ffmpeg_path = write_wrapper(&wrapper_dir, "ffmpeg", &body);
    scenario.config.ffmpeg_timeout = Duration::seconds(1);

    let release = "release-hung";
    let release_dir = scenario.root.join(release);
    fs::create_dir_all(&release_dir).expect("release dir");
    generate_wav(&release_dir.join("track.wav"));
    let staged_wav_hash = blake3::hash(&fs::read(release_dir.join("track.wav")).expect("wav"));
    scenario.settle(release, &[]).await;
    let item_id = WorkItemStore
        .list(&scenario.pool, None, 50, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == release)
        .expect("item")
        .id;

    inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
        None,
    )
    .await
    .expect("inspect");
    assert_eq!("normalizing", scenario.item_state(item_id).await);

    let started = Instant::now();
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("normalize returns");
    let elapsed = started.elapsed();
    assert_eq!(1, stats.failed, "hung tool fails the item");
    assert_eq!(
        "failed",
        scenario.item_state(item_id).await,
        "typed failure lands in failed"
    );
    let (code, message) = scenario.last_error(item_id).await;
    assert_eq!("normalization.tool_failed", code.as_str());
    assert!(
        message.contains("timed out"),
        "actionable timeout message: {message}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(1)
            && elapsed < std::time::Duration::from_secs(30),
        "timeout bounds wall time (took {elapsed:?})"
    );

    // No orphan sleep survives the timeout kill.
    let orphans = Command::new("pgrep")
        .args(["-f", marker])
        .output()
        .expect("pgrep");
    assert!(
        !orphans.status.success(),
        "no orphan process may survive the timeout kill: {}",
        String::from_utf8_lossy(&orphans.stdout)
    );

    // Durable staged source untouched by the failed attempt.
    let staged = scenario.staged_source(item_id);
    assert!(staged.join("track.wav").is_file(), "staged source retained");
    assert_eq!(
        staged_wav_hash,
        blake3::hash(&fs::read(staged.join("track.wav")).expect("staged wav")),
        "staged bytes unchanged"
    );

    // Retry through the supported scheduled path with healthy tools: the
    // claim at a time past next_attempt_at requeues failed -> ready and the
    // same inspection pass re-publishes the manifest to normalizing.
    scenario.config.ffmpeg_path = real_ffmpeg;
    scenario.config.ffmpeg_timeout = Duration::seconds(120);
    inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc() + Duration::seconds(120),
        None,
    )
    .await
    .expect("re-inspect");
    assert_eq!("normalizing", scenario.item_state(item_id).await);
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("retry normalize");
    let (retry_code, retry_message) = scenario.last_error(item_id).await;
    assert_eq!(
        1, stats.completed,
        "retry converges: {stats:?} last error: {retry_code}: {retry_message}"
    );
    assert_eq!("validating", scenario.item_state(item_id).await);

    let _ = fs::remove_dir_all(&wrapper_dir);
    scenario.drop().await;
}

/// FFmpeg reports ENOSPC after writing partial output. The failure is
/// typed and actionable, the partial output never escapes the attempt work
/// area into any authoritative location, the staged source is untouched, and
/// the retry converges.
#[cfg(unix)]
#[tokio::test]
async fn ffmpeg_enospc_partial_output_fails_actionably_and_retry_converges() {
    if !tools_available() {
        eprintln!("ffmpeg/ffprobe/fpcalc unavailable; skipping");
        return;
    }
    let unique = Uuid::now_v7();
    let mut scenario = Scenario::new("envfault-ffmpeg-enospc").await;
    let wrapper_dir = std::env::temp_dir().join(format!("melodee-ffmpeg-enospc-{unique}"));
    let real_ffmpeg = tool_path("ffmpeg");
    let body = format!(
        "#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then exec \"{}\" -version; fi\nout=\"\"\nfor arg in \"$@\"; do out=\"$arg\"; done\nprintf 'partial' > \"$out\" 2>/dev/null || true\necho 'Error writing trailer: No space left on device' >&2\nexit 1\n",
        real_ffmpeg.display()
    );
    scenario.config.ffmpeg_path = write_wrapper(&wrapper_dir, "ffmpeg", &body);

    let release = "release-enospc-tool";
    let release_dir = scenario.root.join(release);
    fs::create_dir_all(&release_dir).expect("release dir");
    generate_wav(&release_dir.join("track.wav"));
    scenario.settle(release, &[]).await;
    let item_id = WorkItemStore
        .list(&scenario.pool, None, 50, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == release)
        .expect("item")
        .id;

    inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
        None,
    )
    .await
    .expect("inspect");
    assert_eq!("normalizing", scenario.item_state(item_id).await);

    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("normalize returns");
    assert_eq!(1, stats.failed, "ENOSPC tool failure fails the item");
    assert_eq!("failed", scenario.item_state(item_id).await);
    let (code, message) = scenario.last_error(item_id).await;
    assert_eq!("normalization.tool_failed", code.as_str());
    assert!(
        message.contains("No space left on device"),
        "tool stderr must reach the operator: {message}"
    );

    // Partial output never reaches an authoritative location: no canonical
    // assets, no completed handoff beyond the staged receipt.
    let canonical_assets: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM media_assets")
        .fetch_one(&scenario.pool)
        .await
        .expect("media assets");
    assert_eq!(0, canonical_assets, "no partial canonical publication");
    assert!(
        scenario.staged_source(item_id).join("track.wav").is_file(),
        "staged source retained"
    );

    // Retry with healthy tools converges through the same scheduled-retry
    // claim path.
    scenario.config.ffmpeg_path = real_ffmpeg;
    inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc() + Duration::seconds(120),
        None,
    )
    .await
    .expect("re-inspect");
    let stats = normalize_once(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        &WorkEngineConfig::default(),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("retry normalize");
    assert_eq!(1, stats.completed, "retry converges: {stats:?}");
    assert_eq!("validating", scenario.item_state(item_id).await);

    let _ = fs::remove_dir_all(&wrapper_dir);
    scenario.drop().await;
}
