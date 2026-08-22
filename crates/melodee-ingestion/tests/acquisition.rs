//! Durable inbound-to-staging acquisition acceptance, crash-convergence, and
//! adversarial tests (ADR-0145; //).
//! Every test runs against real PostgreSQL. Crash boundaries are simulated
//! with the one-shot `test-support` fault hook: the crashed attempt observes
//! the injected error and the retried attempt must converge with no loss, no
//! overwrite, no duplicate acquisition, and no partial authoritative tree.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use melodee_domain::{IngestionState, ReleaseWorkItem};
#[cfg(feature = "test-support")]
use melodee_ingestion::{AcquisitionFaultPoint, AcquisitionTestHook};
use melodee_ingestion::{
    InboundSource, IngestionConfig, acquire_owned_source, owned_source_root, reconcile_all_sources,
    verify_owned_source,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

const T0: time::OffsetDateTime = common::T0;

struct Scenario {
    root: PathBuf,
    config: IngestionConfig,
    pool: PgPool,
    database: common::TestDb,
}

struct HandoffRow {
    status: String,
    method: Option<String>,
    tree_hash: Option<String>,
    staging_relative_path: String,
}

struct FixtureFile {
    relative: &'static str,
    bytes: &'static [u8],
}

impl Scenario {
    async fn new(name: &str) -> Self {
        let unique = Uuid::now_v7();
        let root = std::env::temp_dir().join(format!("melodee-acquisition-{name}-{unique}"));
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

    /// Creates a fixture tree in inbound and settles it to `ready`.
    async fn settle(&self, name: &str, files: &[FixtureFile]) -> ReleaseWorkItem {
        let release = self.root.join(name);
        for file in files {
            let path = release.join(file.relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, file.bytes).expect("file");
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
    }

    async fn claim_one(&self, worker: &WorkerId, now: time::OffsetDateTime) -> ReleaseWorkItem {
        let engine = WorkEngineConfig {
            claim_batch_size: 1,
            ..WorkEngineConfig::default()
        };
        WorkItemStore
            .claim(&self.pool, worker, &engine, now)
            .await
            .expect("claim")
            .into_iter()
            .next()
            .expect("one claimable item")
    }

    async fn acquire(
        &self,
        worker: &WorkerId,
        item: &ReleaseWorkItem,
        now: time::OffsetDateTime,
    ) -> Result<melodee_ingestion::OwnedSource, melodee_ingestion::AcquisitionError> {
        let expected = WorkItemStore
            .latest_snapshot(&self.pool, item.id)
            .await
            .expect("snapshot")
            .map(|snapshot| snapshot.signature);
        acquire_owned_source(
            &self.config,
            &WorkItemStore,
            &self.pool,
            worker,
            item,
            expected.as_deref(),
            now,
        )
        .await
    }

    /// Acquires without the settling-signature pre-check. Used only where a
    /// test deliberately mutated the source mid-copy; reconciliation owns
    /// signature refresh in production.
    #[cfg(feature = "test-support")]
    async fn acquire_unchecked(
        &self,
        worker: &WorkerId,
        item: &ReleaseWorkItem,
        now: time::OffsetDateTime,
    ) -> Result<melodee_ingestion::OwnedSource, melodee_ingestion::AcquisitionError> {
        acquire_owned_source(
            &self.config,
            &WorkItemStore,
            &self.pool,
            worker,
            item,
            None,
            now,
        )
        .await
    }

    async fn handoff(&self, item_id: Uuid) -> Option<HandoffRow> {
        sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
            "SELECT status, method, tree_hash, staging_relative_path
             FROM release_work_item_staging_handoffs WHERE work_item_id = $1",
        )
        .bind(item_id)
        .fetch_optional(&self.pool)
        .await
        .expect("handoff row")
        .map(
            |(status, method, tree_hash, staging_relative_path)| HandoffRow {
                status,
                method,
                tree_hash,
                staging_relative_path,
            },
        )
    }

    async fn handoff_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM release_work_item_staging_handoffs")
            .fetch_one(&self.pool)
            .await
            .expect("handoff count")
    }

    async fn inbound_active(&self, item_id: Uuid) -> bool {
        sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id = $1")
            .bind(item_id)
            .fetch_one(&self.pool)
            .await
            .expect("inbound_active")
    }

    fn reconnect(&self) -> PgPool {
        let url = common::admin_url().replace("/postgres", &format!("/{}", self.database.name));
        PgPool::connect_lazy(&url).expect("reconnect pool")
    }
}

/// Recursive byte-for-byte fingerprint of a tree: relative path -> BLAKE3 of
/// contents, plus explicit directory entries. Symlinks are excluded exactly
/// like the acquisition copy/hash implementation.
fn tree_fingerprint(root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(directory) = queue.pop() {
        let mut entries: Vec<_> = fs::read_dir(&directory)
            .expect("read dir")
            .collect::<Result<_, _>>()
            .expect("entries");
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            let relative = path
                .strip_prefix(root)
                .expect("relative")
                .to_string_lossy()
                .into_owned();
            if metadata.is_dir() {
                map.insert(relative, "dir".to_owned());
                queue.push(path);
            } else if metadata.is_file() {
                let bytes = fs::read(&path).expect("read file");
                map.insert(relative, blake3::hash(&bytes).to_hex().to_string());
            }
        }
    }
    map
}

fn tree_bytes(root: &Path) -> u64 {
    let mut total = 0_u64;
    let mut queue = vec![root.to_path_buf()];
    while let Some(directory) = queue.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("entry").path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                queue.push(path);
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

fn standard_files() -> Vec<FixtureFile> {
    vec![
        FixtureFile {
            relative: "track-01.wav",
            bytes: b"r0-acquisition-audio-a",
        },
        FixtureFile {
            relative: "disc 02/track-02.wav",
            bytes: b"r0-acquisition-audio-b",
        },
        FixtureFile {
            relative: "notes.nfo",
            bytes: b"release notes",
        },
    ]
}

#[cfg(feature = "test-support")]
fn arm_copy_hook(
    config: &mut IngestionConfig,
    fault: AcquisitionFaultPoint,
) -> AcquisitionTestHook {
    let hook = AcquisitionTestHook::new().forcing_copy().with_fault(fault);
    config.acquisition_test_hook = Some(hook.clone());
    hook
}

#[cfg(feature = "test-support")]
fn arm_copy_only(config: &mut IngestionConfig) {
    config.acquisition_test_hook = Some(AcquisitionTestHook::new().forcing_copy());
}

/// the forced cross-filesystem path streams, verifies, fsyncs,
/// no-replace finalizes, records a durable receipt, and only then drains
/// inbound.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn forced_cross_filesystem_acquisition_verifies_and_drains() {
    let mut scenario = Scenario::new("acq-forced-copy").await;
    let files = standard_files();
    let item = scenario.settle("release-cross", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-cross"));
    arm_copy_only(&mut scenario.config);

    let worker = WorkerId::new();
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(2)).await;
    assert_eq!(item.id, claimed.id);
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("acquisition");

    assert_eq!(scenario.staged_source(item.id), owned.root);
    verify_owned_source(&owned).expect("owned source verifies");
    let handoff = scenario.handoff(item.id).await.expect("handoff row");
    assert_eq!("inbound_removed", handoff.status);
    assert_eq!(Some("verified_copy".to_owned()), handoff.method);
    assert!(handoff.tree_hash.is_some(), "durable verified receipt");
    assert_eq!(expected, tree_fingerprint(&owned.root));
    assert!(!scenario.root.join("release-cross").exists());
    assert!(!scenario.inbound_active(item.id).await);

    scenario.database.drop().await;
}

/// crash matrix: every intent/receipt/cleanup boundary is crashed once
/// and retried. Each retry must converge with no loss, no overwrite, no
/// duplicate acquisition, no partial final tree, and eventual inbound drain.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn crash_boundaries_retry_and_converge() {
    let points = [
        ("before-intent", AcquisitionFaultPoint::BeforeIntent),
        ("after-intent", AcquisitionFaultPoint::AfterIntent),
        ("during-copy", AcquisitionFaultPoint::DuringCopy),
        (
            "after-copy",
            AcquisitionFaultPoint::AfterCopyBeforeVerification,
        ),
        (
            "after-verify",
            AcquisitionFaultPoint::AfterVerificationBeforeFinalization,
        ),
        (
            "after-finalize",
            AcquisitionFaultPoint::AfterFinalizationBeforeReceipt,
        ),
        (
            "after-receipt",
            AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval,
        ),
        (
            "after-removal",
            AcquisitionFaultPoint::AfterInboundRemovalBeforeBookkeeping,
        ),
    ];
    let mut scenario = Scenario::new("acq-crash-matrix").await;
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let mut evidence: Vec<String> = Vec::new();

    for (label, point) in points {
        let name = format!("release-{label}");
        scenario.settle(&name, &files).await;
        let expected = tree_fingerprint(&scenario.root.join(&name));
        arm_copy_hook(&mut scenario.config, point);
        let item = scenario.claim_one(&worker, now).await;
        assert_eq!(name, item.source_path);

        let crashed = scenario.acquire(&worker, &item, now).await;
        assert!(
            crashed.is_err(),
            "injected fault at {point:?} must fail the attempt"
        );

        let handoff = scenario.handoff(item.id).await;
        let staged = scenario.staged_source(item.id);
        let partial = scenario.partial_source(item.id);
        match point {
            AcquisitionFaultPoint::BeforeIntent => {
                assert!(handoff.is_none(), "no durable intent recorded");
                assert!(!staged.exists());
            }
            AcquisitionFaultPoint::AfterIntent
            | AcquisitionFaultPoint::DuringCopy
            | AcquisitionFaultPoint::AfterCopyBeforeVerification
            | AcquisitionFaultPoint::AfterVerificationBeforeFinalization => {
                let row = handoff.expect("durable intent");
                assert_eq!("intent", row.status);
                assert!(row.tree_hash.is_none(), "no receipt recorded");
                assert!(
                    !staged.exists(),
                    "no partial authoritative final tree at {point:?}"
                );
                assert!(
                    scenario.root.join(&name).exists(),
                    "source retained for recovery"
                );
            }
            AcquisitionFaultPoint::AfterFinalizationBeforeReceipt => {
                let row = handoff.expect("durable intent");
                assert_eq!("intent", row.status);
                assert!(row.tree_hash.is_none());
                assert!(staged.is_dir(), "final tree exists but has no receipt");
                assert!(scenario.root.join(&name).exists());
            }
            AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval => {
                let row = handoff.expect("receipt");
                assert_eq!("staged", row.status);
                assert!(row.tree_hash.is_some());
                assert!(
                    scenario.root.join(&name).exists(),
                    "inbound not yet removed"
                );
            }
            AcquisitionFaultPoint::AfterInboundRemovalBeforeBookkeeping => {
                let row = handoff.expect("receipt");
                assert_eq!("staged", row.status);
                assert!(
                    !scenario.root.join(&name).exists(),
                    "inbound physically drained before bookkeeping"
                );
                assert!(
                    scenario.inbound_active(item.id).await,
                    "bookkeeping not yet recorded"
                );
            }
        }

        // Retry after the simulated restart: the one-shot fault is absent.
        arm_copy_only(&mut scenario.config);
        let owned = scenario
            .acquire(&worker, &item, now)
            .await
            .unwrap_or_else(|error| panic!("retry after {point:?} converges: {error}"));
        assert_eq!(staged, owned.root);
        verify_owned_source(&owned).expect("receipt matches tree");
        let row = scenario.handoff(item.id).await.expect("completed handoff");
        assert_eq!("inbound_removed", row.status, "{point:?}");
        assert_eq!(Some("verified_copy".to_owned()), row.method);
        assert_eq!(expected, tree_fingerprint(&staged), "no loss at {point:?}");
        assert!(!scenario.root.join(&name).exists(), "inbound drained");
        assert!(!partial.exists(), "hidden attempt scratch removed");
        assert!(!scenario.inbound_active(item.id).await);
        assert_eq!(
            1,
            scenario.handoff_count().await - i64::try_from(evidence.len()).unwrap(),
            "no duplicate acquisition at {point:?}"
        );
        evidence.push(format!("{label}:converged"));
    }

    eprintln!(
        "r0-crash-matrix={{\"boundaries\":{},\"retried\":{},\"duplicates\":0}}",
        points.len(),
        evidence.len()
    );
    scenario.database.drop().await;
}

/// Source additions, byte changes, and removals during the copy
/// window fail closed; the hidden partial is removed; inbound is untouched;
/// and a retry against the settled new source converges.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn source_mutation_during_copy_fails_closed_and_recovers() {
    let mut scenario = Scenario::new("acq-copy-mutation").await;
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    for (label, mutate) in [("add", 0_u8), ("modify", 1_u8), ("remove", 2_u8)] {
        let name = format!("release-{label}");
        scenario.settle(&name, &files).await;
        let hook = AcquisitionTestHook::new()
            .forcing_copy()
            .pausing_during_copy();
        scenario.config.acquisition_test_hook = Some(hook.clone());
        let item = scenario.claim_one(&worker, now).await;

        let config = scenario.config.clone();
        let pool = scenario.pool.clone();
        let task_item = item.clone();
        let worker_for_task = worker.clone();
        let acquisition = tokio::spawn(async move {
            acquire_owned_source(
                &config,
                &WorkItemStore,
                &pool,
                &worker_for_task,
                &task_item,
                None,
                now,
            )
            .await
        });
        hook.wait_until_copy_started().await;
        match mutate {
            0 => fs::write(scenario.root.join(&name).join("late.nfo"), b"late").expect("add"),
            1 => fs::write(
                scenario.root.join(&name).join("track-01.wav"),
                b"mutated bytes",
            )
            .expect("modify"),
            _ => fs::remove_file(scenario.root.join(&name).join("notes.nfo")).expect("remove"),
        }
        hook.release_copy();
        let result = acquisition.await.expect("task");
        assert!(
            matches!(
                result,
                Err(melodee_ingestion::AcquisitionError::SourceChanged)
            ),
            "{label} mutation during copy must fail closed"
        );
        assert!(
            !scenario.partial_source(item.id).exists(),
            "partial attempt scratch removed after {label}"
        );
        assert!(
            scenario.root.join(&name).exists(),
            "inbound source retained after {label}"
        );
        let row = scenario.handoff(item.id).await.expect("intent");
        assert_eq!("intent", row.status, "no receipt from a changed tree");

        // The settled mutated tree becomes the new owned source on retry. The
        // signature pre-check belongs to the reconciliation/inspection layer
        // (it re-settles and refreshes the snapshot); the copy-window fence
        // under test is the streamed before/after tree comparison.
        arm_copy_only(&mut scenario.config);
        let expected = tree_fingerprint(&scenario.root.join(&name));
        let owned = scenario
            .acquire_unchecked(&worker, &item, now)
            .await
            .expect("retry with settled source");
        assert_eq!(expected, tree_fingerprint(&owned.root));
        assert!(!scenario.root.join(&name).exists());
    }

    scenario.database.drop().await;
}

/// a stale worker that loses its lease mid-copy cannot record the
/// receipt, cannot remove source, and its on-disk finalize is adopted only
/// after byte verification by the current owner.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn stale_worker_losing_lease_during_copy_cannot_commit() {
    let mut scenario = Scenario::new("acq-lease-loss").await;
    let files = standard_files();
    let item = scenario.settle("release-lease", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-lease"));
    let stale = WorkerId::new();
    let claimed = scenario.claim_one(&stale, T0 + Duration::seconds(2)).await;

    let hook = AcquisitionTestHook::new()
        .forcing_copy()
        .pausing_during_copy();
    scenario.config.acquisition_test_hook = Some(hook.clone());
    let config = scenario.config.clone();
    let pool = scenario.pool.clone();
    let stale_for_task = stale.clone();
    let task_item = claimed.clone();
    let attempt = tokio::spawn(async move {
        acquire_owned_source(
            &config,
            &WorkItemStore,
            &pool,
            &stale_for_task,
            &task_item,
            None,
            T0 + Duration::seconds(2),
        )
        .await
    });
    hook.wait_until_copy_started().await;

    // A second worker steals the expired lease while the copy is paused.
    let thief = WorkerId::new();
    let engine = WorkEngineConfig::default();
    let stolen_at = T0 + Duration::seconds(2) + engine.lease_duration + Duration::seconds(1);
    let stolen = WorkItemStore
        .claim(&scenario.pool, &thief, &engine, stolen_at)
        .await
        .expect("steal claim");
    assert_eq!(1, stolen.len());
    assert_eq!(item.id, stolen[0].id);
    hook.release_copy();

    let result = attempt.await.expect("task");
    assert!(
        result.is_err(),
        "stale worker must not record the receipt: {result:?}"
    );
    let row = scenario.handoff(item.id).await.expect("intent");
    assert_eq!("intent", row.status, "stale worker recorded a receipt");
    assert!(
        scenario.root.join("release-lease").exists(),
        "stale worker removed source"
    );

    // The current owner converges: it adopts the verified on-disk finalize.
    arm_copy_only(&mut scenario.config);
    let owned = scenario
        .acquire(&thief, &stolen[0], stolen_at)
        .await
        .expect("owner converges");
    assert_eq!(expected, tree_fingerprint(&owned.root));
    let row = scenario.handoff(item.id).await.expect("handoff");
    assert_eq!("inbound_removed", row.status);
    assert_eq!(1, scenario.handoff_count().await);
    assert!(!scenario.root.join("release-lease").exists());

    scenario.database.drop().await;
}

/// delayed cleanup after occurrence A is replaced by occurrence B at
/// the same inbound path never deletes B, and B becomes a distinct
/// acquisition.
#[cfg(feature = "test-support")]
#[tokio::test]
async fn delayed_cleanup_after_path_reuse_preserves_new_occurrence() {
    let mut scenario = Scenario::new("acq-path-reuse").await;
    let files = standard_files();
    let item_a = scenario.settle("reused", &files).await;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    arm_copy_hook(
        &mut scenario.config,
        AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval,
    );
    let claimed = scenario.claim_one(&worker, now).await;
    let crashed = scenario.acquire(&worker, &claimed, now).await;
    assert!(crashed.is_err(), "receipt persisted before the crash");

    // Occurrence B replaces the same inbox path with different bytes while
    // A's cleanup is still delayed.
    fs::remove_dir_all(scenario.root.join("reused")).expect("replace inbound");
    fs::create_dir_all(scenario.root.join("reused")).expect("recreate");
    fs::write(
        scenario.root.join("reused").join("track-01.wav"),
        b"occurrence-b-bytes",
    )
    .expect("b bytes");
    let occurrence_b = tree_fingerprint(&scenario.root.join("reused"));

    // A's delayed cleanup observes the generation/hash mismatch and refuses.
    arm_copy_only(&mut scenario.config);
    let owned_a = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("A recovery completes bookkeeping");
    verify_owned_source(&owned_a).expect("A staged intact");
    let row_a = scenario.handoff(item_a.id).await.expect("A handoff");
    assert_eq!("staged", row_a.status, "A records replacement, not removal");
    assert!(!scenario.inbound_active(item_a.id).await);
    assert_eq!(
        occurrence_b,
        tree_fingerprint(&scenario.root.join("reused")),
        "A's delayed cleanup must never delete B"
    );

    // The same path now settles as a distinct acquisition.
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        now + Duration::seconds(1),
    )
    .await
    .expect("rediscover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        now + Duration::seconds(2),
    )
    .await
    .expect("resettle");
    let items = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .expect("list");
    let item_b = items
        .iter()
        .find(|item| item.source_path == "reused" && item.id != item_a.id)
        .expect("distinct second occurrence")
        .clone();
    let worker_b = WorkerId::new();
    let claimed_b = scenario
        .claim_one(&worker_b, now + Duration::seconds(3))
        .await;
    assert_eq!(item_b.id, claimed_b.id);
    let owned_b = scenario
        .acquire(&worker_b, &claimed_b, now + Duration::seconds(3))
        .await
        .expect("B acquires");
    assert_eq!(occurrence_b, tree_fingerprint(&owned_b.root));
    assert_eq!(2, scenario.handoff_count().await);
    assert!(!scenario.root.join("reused").exists());

    scenario.database.drop().await;
}

/// a matching pre-existing final tree is adopted (never overwritten),
/// and a mismatching final tree fails closed with both trees untouched.
#[tokio::test]
async fn final_staging_tree_adoption_and_conflict_fail_closed() {
    let scenario = Scenario::new("acq-adoption").await;
    let files = standard_files();
    let item = scenario.settle("release-adopt", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-adopt"));
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;
    let destination = scenario.staged_source(item.id);

    // Simulate a crash after finalization but before the durable intent:
    // no handoff row, complete final tree, inbound still present.
    fs::create_dir_all(destination.parent().expect("parent")).expect("dirs");
    copy_fixture_tree(&scenario.root.join("release-adopt"), &destination);
    let first = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("matching final adopted");
    assert_eq!(expected, tree_fingerprint(&first.root));
    let row = scenario.handoff(item.id).await.expect("receipt");
    assert_eq!("inbound_removed", row.status);
    assert!(!scenario.root.join("release-adopt").exists());

    // A mismatching final tree under durable intent fails closed.
    let conflict_item = scenario.settle("release-conflict", &files).await;
    let claimed_conflict = scenario.claim_one(&worker, now).await;
    assert_eq!(conflict_item.id, claimed_conflict.id);
    let conflict_destination = scenario.staged_source(conflict_item.id);
    fs::create_dir_all(&conflict_destination).expect("conflict dir");
    fs::write(conflict_destination.join("foreign.bin"), b"foreign").expect("foreign bytes");
    let conflicted = scenario.acquire(&worker, &claimed_conflict, now).await;
    assert!(
        matches!(
            conflicted,
            Err(melodee_ingestion::AcquisitionError::IncompleteReceipt(_))
        ),
        "conflicting final must fail closed: {conflicted:?}"
    );
    assert_eq!(
        b"foreign".as_slice(),
        fs::read(conflict_destination.join("foreign.bin")).expect("foreign intact"),
        "conflicting tree is never overwritten or deleted"
    );
    assert!(
        scenario.root.join("release-conflict").exists(),
        "source retained"
    );
    let row = scenario.handoff(conflict_item.id).await.expect("intent");
    assert_eq!("intent", row.status, "no receipt from a conflict");
    assert!(row.tree_hash.is_none());

    scenario.database.drop().await;
}

/// quotes, apostrophes, semicolons, ampersands, parentheses, leading
/// dashes, and Unicode in candidate names acquire byte-for-byte.
#[tokio::test]
async fn adversarial_candidate_names_acquire_cleanly() {
    let scenario = Scenario::new("acq-names").await;
    let names = [
        "it's a \"quoted\"; & (live)",
        "-leading-dash",
        "café ΩΔ 日本語",
        "semi;colon & ampersand",
    ];
    let files = standard_files();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    for name in names {
        let item = scenario.settle(name, &files).await;
        let expected = tree_fingerprint(&scenario.root.join(name));
        let claimed = scenario.claim_one(&worker, now).await;
        assert_eq!(name, claimed.source_path);
        let owned = scenario
            .acquire(&worker, &claimed, now)
            .await
            .expect("acquire");
        assert_eq!(expected, tree_fingerprint(&owned.root), "{name}");
        assert!(!scenario.root.join(name).exists());
        let row = scenario.handoff(item.id).await.expect("handoff");
        assert_eq!(
            format!("releases/{}/source", item.id),
            row.staging_relative_path,
            "deterministic staging path derives from the work-item identity"
        );
    }
    assert_eq!(4, scenario.handoff_count().await);
    assert_eq!(
        0,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "inbound fully drained"
    );

    scenario.database.drop().await;
}

/// traversal, absolute, and nested source paths recorded in durable
/// state are rejected before any filesystem mutation.
#[tokio::test]
async fn traversal_source_paths_are_rejected_before_io() {
    let scenario = Scenario::new("acq-traversal").await;
    fs::create_dir_all(scenario.root.join("innocent")).expect("inbound dir");
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    for malicious in ["../escape", "/etc/absolute", "nested/child"] {
        let item = ReleaseWorkItem::discovered(malicious.to_owned(), None, Uuid::now_v7(), T0);
        let id = item.id;
        WorkItemStore
            .create(&scenario.pool, &item)
            .await
            .expect("create");
        WorkItemStore
            .system_transition(
                &scenario.pool,
                id,
                IngestionState::Settling,
                Some("test"),
                T0,
            )
            .await
            .expect("settling");
        WorkItemStore
            .system_transition(&scenario.pool, id, IngestionState::Ready, Some("test"), T0)
            .await
            .expect("ready");
        let claimed = scenario.claim_one(&worker, now).await;
        assert_eq!(malicious, claimed.source_path);
        let result = scenario.acquire(&worker, &claimed, now).await;
        assert!(
            matches!(
                result,
                Err(melodee_ingestion::AcquisitionError::InvalidSourcePath(_))
            ),
            "{malicious} must be rejected: {result:?}"
        );
        assert!(
            scenario.handoff(id).await.is_none(),
            "no durable intent for a rejected path"
        );
        assert!(!scenario.staged_source(id).exists());
    }
    assert_eq!(
        1,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "inbound untouched"
    );

    scenario.database.drop().await;
}

/// a non-directory collision at the deterministic staging path fails
/// closed without touching the collision or the source.
#[tokio::test]
async fn staging_collision_with_non_directory_fails_closed() {
    let scenario = Scenario::new("acq-collision").await;
    let files = standard_files();
    let item = scenario.settle("release-collide", &files).await;
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;

    let destination = scenario.staged_source(item.id);
    fs::create_dir_all(destination.parent().expect("parent")).expect("dirs");
    fs::write(&destination, b"collision-file").expect("collision");

    let result = scenario.acquire(&worker, &claimed, now).await;
    assert!(result.is_err(), "collision must fail: {result:?}");
    assert_eq!(
        b"collision-file".as_slice(),
        fs::read(&destination).expect("collision intact")
    );
    assert!(scenario.root.join("release-collide").exists());
    let row = scenario.handoff(item.id).await.expect("intent");
    assert_eq!("intent", row.status);
    assert!(row.tree_hash.is_none());

    scenario.database.drop().await;
}

/// after a restart with inbound already drained, recovery adopts the
/// durable receipt and inspection continues from the staged source.
#[tokio::test]
async fn restart_with_inbound_absent_continues_from_staging() {
    let scenario = Scenario::new("acq-restart").await;
    let release = scenario.root.join("release-restart");
    fs::create_dir_all(&release).expect("release");
    write_tagless_wav(&release.join("track.wav"));
    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    let item = WorkItemStore
        .list(&scenario.pool, None, 10, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == "release-restart")
        .expect("item");
    let expected = tree_fingerprint(&release);

    // First worker acquires fully (inbound drained) and then "crashes"
    // before inspection by simply never inspecting.
    let crashed_worker = WorkerId::new();
    let claimed = scenario
        .claim_one(&crashed_worker, T0 + Duration::seconds(2))
        .await;
    let owned = scenario
        .acquire(&crashed_worker, &claimed, T0 + Duration::seconds(2))
        .await
        .expect("acquisition");
    assert!(!release.exists(), "inbound drained before the crash");
    drop(owned);

    // Restart: a fresh pool and worker reclaim the expired lease and inspect.
    let restarted = scenario.reconnect();
    let engine = WorkEngineConfig::default();
    let restart_now = T0 + Duration::seconds(2) + engine.lease_duration + Duration::seconds(1);
    let stats = melodee_ingestion::inspect_ready_once(
        &scenario.config,
        &WorkItemStore,
        &restarted,
        &engine,
        restart_now,
        None,
    )
    .await
    .expect("inspection after restart");
    assert_eq!(1, stats.completed);
    let inspected = WorkItemStore.get(&restarted, item.id).await.expect("item");
    assert_eq!(IngestionState::Normalizing, inspected.state);
    let manifest = WorkItemStore
        .get_manifest(&restarted, item.id)
        .await
        .expect("manifest lookup")
        .expect("manifest");
    assert_eq!("release-restart", manifest.payload["source_path"]);
    let owned_after = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &inspected)
        .await
        .expect("resolver adopts durable receipt");
    assert_eq!(expected, tree_fingerprint(&owned_after.root));
    assert_eq!(1, scenario.handoff_count().await);
    restarted.close().await;

    scenario.database.drop().await;
}

/// NeedsReview and Quarantined work retains the durable staged source
/// across a restart, and the owned-source resolver keeps serving it.
#[tokio::test]
async fn needs_review_and_quarantined_retain_staged_source_across_restart() {
    let scenario = Scenario::new("acq-retention").await;
    let files = standard_files();
    let item = scenario.settle("release-review", &files).await;
    let expected = tree_fingerprint(&scenario.root.join("release-review"));
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);
    let claimed = scenario.claim_one(&worker, now).await;
    let owned = scenario
        .acquire(&worker, &claimed, now)
        .await
        .expect("acquisition");
    assert!(!scenario.root.join("release-review").exists());

    // Walk the legal worker path to NeedsReview, then operator-quarantine.
    for state in [
        IngestionState::Inspecting,
        IngestionState::Normalizing,
        IngestionState::Validating,
        IngestionState::NeedsReview,
    ] {
        WorkItemStore
            .transition(
                &scenario.pool,
                item.id,
                Some(&worker),
                melodee_domain::TransitionKind::Worker,
                state,
                Some("r0-retention"),
                now,
            )
            .await
            .expect("worker transition");
    }
    let restarted = scenario.reconnect();
    let item = WorkItemStore
        .get(&restarted, item.id)
        .await
        .expect("item after restart");
    assert_eq!(IngestionState::NeedsReview, item.state);
    let resolved = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &item)
        .await
        .expect("NeedsReview resolves staged source");
    assert_eq!(
        expected,
        tree_fingerprint(&resolved.root),
        "NeedsReview retains staged source across restart"
    );

    WorkItemStore
        .transition(
            &restarted,
            item.id,
            None,
            melodee_domain::TransitionKind::Operator,
            IngestionState::Quarantined,
            Some("r0-retention"),
            now + Duration::seconds(1),
        )
        .await
        .expect("operator quarantine");
    let quarantined = WorkItemStore
        .get(&restarted, item.id)
        .await
        .expect("quarantined item");
    assert_eq!(IngestionState::Quarantined, quarantined.state);
    let resolved = owned_source_root(&scenario.config, &WorkItemStore, &restarted, &quarantined)
        .await
        .expect("Quarantined resolves staged source");
    assert_eq!(
        expected,
        tree_fingerprint(&resolved.root),
        "Quarantined retains staged source across restart"
    );
    restarted.close().await;
    drop(owned);

    scenario.database.drop().await;
}

/// concurrent workers racing one occurrence produce exactly one
/// authoritative acquisition, receipt, and drain.
#[tokio::test]
async fn concurrent_acquisition_of_one_occurrence_is_single_authoritative() {
    let scenario = Scenario::new("acq-concurrent").await;
    let files = standard_files();
    let item = scenario.settle("release-race", &files).await;
    let now = T0 + Duration::seconds(2);

    let winner = WorkerId::new();
    let loser = WorkerId::new();
    let claimed = scenario.claim_one(&winner, now).await;
    // The loser's claim is fenced out by the live lease.
    let engine = WorkEngineConfig::default();
    let loser_claims = WorkItemStore
        .claim(&scenario.pool, &loser, &engine, now)
        .await
        .expect("loser claim");
    assert!(loser_claims.is_empty(), "lease exclusivity at claim");
    let loser_result = scenario.acquire(&loser, &item, now).await;
    assert!(
        loser_result.is_err(),
        "a worker without the lease cannot begin the handoff"
    );
    assert!(scenario.handoff(item.id).await.is_none());

    let owned = scenario
        .acquire(&winner, &claimed, now)
        .await
        .expect("winner acquires");
    verify_owned_source(&owned).expect("receipt");
    assert_eq!(1, scenario.handoff_count().await);
    let row = scenario.handoff(item.id).await.expect("handoff");
    assert_eq!("inbound_removed", row.status);
    assert!(!scenario.root.join("release-race").exists());

    scenario.database.drop().await;
}

/// 200 stable Releases on one filesystem acquire byte-for-byte, drain
/// inbound completely, leave 200 complete receipts with no duplicate
/// identity, rescan to zero historical candidates, and a new arrival
/// produces exactly one new acquisition.
#[tokio::test]
async fn two_hundred_release_acquisition_drains_inbound_and_stays_drained() {
    let scenario = Scenario::new("acq-200").await;
    let started = std::time::Instant::now();
    let worker = WorkerId::new();
    let now = T0 + Duration::seconds(2);

    let mut expected: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut total_bytes = 0_u64;
    let mut total_files = 0_u64;
    for index in 0..200 {
        let name = format!("release-{index:03}");
        let release = scenario.root.join(&name);
        fs::create_dir_all(release.join("disc 01")).expect("dirs");
        fs::write(release.join("track-01.wav"), format!("audio-{index}")).expect("audio");
        fs::write(
            release.join("disc 01/track-02.wav"),
            format!("b-side-{index}"),
        )
        .expect("b");
        if index % 3 == 0 {
            fs::write(release.join("notes.nfo"), format!("notes-{index}")).expect("nfo");
        }
        expected.insert(name.clone(), tree_fingerprint(&release));
        total_bytes += tree_bytes(&release);
        total_files += expected[&name].len() as u64;
    }

    reconcile_all_sources(&scenario.config, &WorkItemStore, &scenario.pool, T0)
        .await
        .expect("discover");
    let settled = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(1),
    )
    .await
    .expect("settle");
    assert_eq!(
        200_u64,
        settled.iter().map(|r| r.stats.candidates_seen).sum::<u64>()
    );

    let engine = WorkEngineConfig {
        claim_batch_size: 25,
        ..WorkEngineConfig::default()
    };
    let mut acquired = 0_u64;
    loop {
        let items = WorkItemStore
            .claim(&scenario.pool, &worker, &engine, now)
            .await
            .expect("claim batch");
        if items.is_empty() {
            break;
        }
        for item in &items {
            let owned = scenario.acquire(&worker, item, now).await.expect("acquire");
            assert_eq!(
                expected[&item.source_path],
                tree_fingerprint(&owned.root),
                "byte-for-byte staged tree for {}",
                item.source_path
            );
            acquired += 1;
        }
    }
    assert_eq!(200, acquired);

    assert_eq!(
        0,
        fs::read_dir(&scenario.root).expect("inbound").count(),
        "all 200 inbound candidate directories drained"
    );
    let (work_items, handoffs, distinct_items, distinct_paths, removed, renames): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM release_work_items),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(DISTINCT work_item_id) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(DISTINCT staging_relative_path) FROM release_work_item_staging_handoffs),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs
             WHERE status = 'inbound_removed' AND tree_hash IS NOT NULL),
            (SELECT COUNT(*) FROM release_work_item_staging_handoffs WHERE method = 'rename')",
    )
    .fetch_one(&scenario.pool)
    .await
    .expect("counts");
    assert_eq!(
        (200, 200, 200, 200, 200, 200),
        (
            work_items,
            handoffs,
            distinct_items,
            distinct_paths,
            removed,
            renames
        ),
        "200 work items, 200 complete rename receipts, no duplicate identity"
    );

    // A second reconciliation sees zero historical candidates.
    let rescan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(3),
    )
    .await
    .expect("second reconcile");
    let rescan_stats = rescan.iter().fold((0, 0, 0), |acc, result| {
        (
            acc.0 + result.stats.candidates_seen,
            acc.1 + result.stats.new_work_items,
            acc.2 + result.stats.missing_candidates,
        )
    });
    assert_eq!(
        (0, 0, 0),
        rescan_stats,
        "zero historical candidates after drain"
    );
    let (item_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_items")
        .fetch_one(&scenario.pool)
        .await
        .expect("count");
    assert_eq!(200, item_count);

    // Exactly one new arrival produces exactly one new acquisition.
    let arrival = scenario.root.join("release-200-new");
    fs::create_dir_all(&arrival).expect("arrival");
    fs::write(arrival.join("track.wav"), b"arrival-audio").expect("audio");
    let arrival_fingerprint = tree_fingerprint(&arrival);
    reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(4),
    )
    .await
    .expect("discover arrival");
    let arrival_scan = reconcile_all_sources(
        &scenario.config,
        &WorkItemStore,
        &scenario.pool,
        T0 + Duration::seconds(5),
    )
    .await
    .expect("settle arrival");
    assert_eq!(
        1_u64,
        arrival_scan
            .iter()
            .map(|r| r.stats.candidates_seen)
            .sum::<u64>()
    );
    let new_item = WorkItemStore
        .list(&scenario.pool, None, 500, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|item| item.source_path == "release-200-new")
        .expect("new item");
    let claimed = scenario.claim_one(&worker, T0 + Duration::seconds(6)).await;
    assert_eq!(new_item.id, claimed.id);
    let owned = scenario
        .acquire(&worker, &claimed, T0 + Duration::seconds(6))
        .await
        .expect("new acquisition");
    assert_eq!(arrival_fingerprint, tree_fingerprint(&owned.root));
    assert_eq!(201, scenario.handoff_count().await);
    assert_eq!(0, fs::read_dir(&scenario.root).expect("inbound").count());

    eprintln!(
        "r0-same-fs-acceptance={{\"releases\":200,\"files\":{total_files},\"bytes\":{total_bytes},\"handoff_method\":\"rename\",\"wall_ms\":{},\"second_scan_candidates\":0,\"new_arrivals_acquired\":1}}",
        started.elapsed().as_millis()
    );
    scenario.database.drop().await;
}

fn copy_fixture_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("mkdir");
    for entry in fs::read_dir(source).expect("read dir") {
        let entry = entry.expect("entry");
        let target = destination.join(entry.file_name());
        let metadata = entry.metadata().expect("metadata");
        if metadata.is_dir() {
            copy_fixture_tree(&entry.path(), &target);
        } else if metadata.is_file() {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
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
