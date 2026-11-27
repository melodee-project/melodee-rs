//! Durable inbound-to-staging acquisition (ADR-0145).
//!
//! Inbound bytes are never rewritten. A Release is moved atomically when
//! possible; otherwise it is copied into a hidden staging directory, verified
//! with streamed BLAKE3 tree hashing, finalized without replacement, and only
//! then removed from inbound.

use std::{
    fs,
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
};

#[cfg(feature = "test-support")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    StagingHandoff, StagingHandoffMethod, StagingHandoffStatus, WorkItemStore, WorkerId,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    config::{InboundSource, IngestionConfig},
    secure_fs::{
        create_directory_tree, open_directory, open_read, open_write_new, remove_directory_tree,
        rename_noreplace,
    },
    snapshot::snapshot_release_directory,
};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct OwnedSource {
    pub root: PathBuf,
    pub tree_hash: String,
}

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("work item source is no longer configured")]
    SourceNotConfigured,
    #[error("invalid inbound candidate path: {0}")]
    InvalidSourcePath(String),
    #[error("inbound source changed during acquisition")]
    SourceChanged,
    #[error("staging receipt is missing or incomplete: {0}")]
    IncompleteReceipt(String),
    #[error("filesystem operation failed for {path:?}: {source}")]
    Filesystem { path: PathBuf, source: io::Error },
    #[error("persistence failed: {0}")]
    Persistence(#[from] melodee_persistence::WorkStoreError),
    #[error("task join failed: {0}")]
    Task(String),
}

fn io_error(path: &Path, source: io::Error) -> AcquisitionError {
    AcquisitionError::Filesystem {
        path: path.to_path_buf(),
        source,
    }
}

/// Crash-boundary injection points for the durable handoff. Each variant names
/// the exact durable/physical boundary after which a crash is simulated.
///
/// This type and the `IngestionConfig::acquisition_test_hook` field are absent
/// unless the non-default `test-support` feature is explicitly enabled. No
/// production build can force the copy path or inject handoff faults.
#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionFaultPoint {
    /// Before durable handoff intent is persisted.
    BeforeIntent,
    /// After durable intent commits, before any filesystem change.
    AfterIntent,
    /// Midway through the verified copy (after at least one file).
    DuringCopy,
    /// After the copy completes, before post-copy source verification.
    AfterCopyBeforeVerification,
    /// After verification passes, before the no-replace finalize.
    AfterVerificationBeforeFinalization,
    /// After the final tree exists, before the durable receipt is recorded.
    AfterFinalizationBeforeReceipt,
    /// After the durable receipt, before inbound removal starts.
    AfterReceiptBeforeInboundRemoval,
    /// After inbound bytes are removed, before the removal is recorded.
    AfterInboundRemovalBeforeBookkeeping,
}

/// One-shot fault injection plus copy barrier for deterministic crash, lease
///-loss, and mutation-during-copy tests. A fault fires at most once per hook
/// instance: the crashed attempt observes the error and the retried attempt
/// converges, modeling a process restart that no longer injects the fault.
/// Write-path faults for the verified copy. Each variant models an
/// environment failure at the exact destination-write boundary.
///
/// This type is absent unless the non-default `test-support` feature is
/// explicitly enabled, so no production build can inject write faults.
#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquisitionWriteFault {
    /// The first destination write fails with ENOSPC, modeling a full
    /// staging disk mid-copy.
    NoSpaceLeft,
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct AcquisitionTestHook {
    force_copy: bool,
    pause_during_copy: bool,
    fault: Option<AcquisitionFaultPoint>,
    write_fault: Option<AcquisitionWriteFault>,
    copy_write_delay: Option<std::time::Duration>,
    state: Arc<AcquisitionTestHookState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
struct AcquisitionTestHookState {
    fault_fired: AtomicBool,
    write_fault_fired: AtomicBool,
    copy_reached: AtomicBool,
    copy_released: AtomicBool,
    notify_reached: tokio::sync::Notify,
    notify_released: tokio::sync::Notify,
}

#[cfg(feature = "test-support")]
impl AcquisitionTestHook {
    #[must_use]
    pub fn new() -> Self {
        Self {
            force_copy: false,
            pause_during_copy: false,
            fault: None,
            write_fault: None,
            copy_write_delay: None,
            state: Arc::new(AcquisitionTestHookState {
                fault_fired: AtomicBool::new(false),
                write_fault_fired: AtomicBool::new(false),
                copy_reached: AtomicBool::new(false),
                copy_released: AtomicBool::new(false),
                notify_reached: tokio::sync::Notify::new(),
                notify_released: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Force the verified-copy branch even when rename would succeed. This is
    /// the cross-filesystem substitute for CI hosts with one filesystem.
    #[must_use]
    pub fn forcing_copy(mut self) -> Self {
        self.force_copy = true;
        self
    }

    /// Inject a one-shot fault at the given crash boundary.
    #[must_use]
    pub fn with_fault(mut self, fault: AcquisitionFaultPoint) -> Self {
        self.fault = Some(fault);
        self
    }

    /// Inject a one-shot write fault on the first destination write of the
    /// verified copy, modeling a full staging disk.
    #[must_use]
    pub fn with_write_fault(mut self, fault: AcquisitionWriteFault) -> Self {
        self.write_fault = Some(fault);
        self
    }

    /// Sleep the given duration before every destination write of the
    /// verified copy, modeling slow storage.
    #[must_use]
    pub fn delaying_copy_writes(mut self, delay: std::time::Duration) -> Self {
        self.copy_write_delay = Some(delay);
        self
    }

    /// Pause the verified copy after the pre-copy source hash until
    /// `release_copy` is called, so a test can mutate the source or steal the
    /// lease at the exact copy window.
    #[must_use]
    pub fn pausing_during_copy(mut self) -> Self {
        self.pause_during_copy = true;
        self
    }

    /// Waits until the copy window is entered. Requires `pausing_during_copy`.
    pub async fn wait_until_copy_started(&self) {
        loop {
            if self.state.copy_reached.load(Ordering::SeqCst) {
                return;
            }
            let notified = self.state.notify_reached.notified();
            if self.state.copy_reached.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    /// Releases a paused copy window.
    pub fn release_copy(&self) {
        self.state.copy_released.store(true, Ordering::SeqCst);
        self.state.notify_released.notify_waiters();
    }

    fn should_fire(&self, point: AcquisitionFaultPoint) -> bool {
        self.fault == Some(point) && !self.state.fault_fired.swap(true, Ordering::SeqCst)
    }

    /// Fires the configured write fault at most once, on the first
    /// destination write, so the crashed attempt observes the error and the
    /// retried attempt converges — the same restart semantics as
    /// `should_fire`.
    fn take_write_fault(&self) -> Option<AcquisitionWriteFault> {
        if self.write_fault.is_some() && !self.state.write_fault_fired.swap(true, Ordering::SeqCst)
        {
            self.write_fault
        } else {
            None
        }
    }

    fn delay_write(&self) {
        if let Some(delay) = self.copy_write_delay {
            std::thread::sleep(delay);
        }
    }

    fn pause_copy_window(&self) {
        if !self.pause_during_copy {
            return;
        }
        self.state.copy_reached.store(true, Ordering::SeqCst);
        self.state.notify_reached.notify_waiters();
        while !self.state.copy_released.load(Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
}

#[cfg(feature = "test-support")]
impl Default for AcquisitionTestHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Injects a one-shot test fault as an I/O-style acquisition error. The error
/// carries no special variant so production error handling is unchanged.
#[cfg(feature = "test-support")]
fn inject_test_fault(
    hook: Option<&AcquisitionTestHook>,
    point: AcquisitionFaultPoint,
    path: &Path,
) -> Result<(), AcquisitionError> {
    if let Some(hook) = hook
        && hook.should_fire(point)
    {
        return Err(io_error(
            path,
            io::Error::other(format!("injected acquisition fault at {point:?}")),
        ));
    }
    Ok(())
}

/// Writes durable intent then returns the one owned source root. No media
/// inspection happens until this succeeds.
pub async fn acquire_owned_source(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item: &ReleaseWorkItem,
    expected_snapshot_signature: Option<&str>,
    now: OffsetDateTime,
) -> Result<OwnedSource, AcquisitionError> {
    let started = std::time::Instant::now();
    if !store
        .verify_inbound_path_identity(pool, item.id, worker, now)
        .await?
    {
        // An existing receipt already binds owned staging bytes. Reverify that
        // receipt without consulting or deleting the ambiguous inbound path.
        return owned_source_root(config, store, pool, item).await;
    }
    #[cfg(feature = "test-support")]
    let test_hook = config.acquisition_test_hook.clone();
    let source = find_source(config, item).ok_or(AcquisitionError::SourceNotConfigured)?;
    let inbound = resolve_candidate(source, &item.source_path)?;
    let staging_root = config.staging_root.as_ref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt(
            "MELODEE__INGEST__STAGING_ROOT is not configured".to_owned(),
        )
    })?;
    #[cfg(feature = "test-support")]
    inject_test_fault(
        test_hook.as_ref(),
        AcquisitionFaultPoint::BeforeIntent,
        &inbound,
    )?;
    let existing_handoff = store.staging_handoff(pool, item.id).await?;
    let generation = match &existing_handoff {
        Some(handoff) => handoff.source_generation.clone(),
        None => source_generation(&inbound)?,
    };
    let relative = format!("releases/{}/source", item.id);
    let handoff = store
        .begin_staging_handoff(pool, item.id, worker, &relative, &generation, now)
        .await?;
    #[cfg(feature = "test-support")]
    inject_test_fault(
        test_hook.as_ref(),
        AcquisitionFaultPoint::AfterIntent,
        &inbound,
    )?;
    let destination = staging_root.join(&handoff.staging_relative_path);

    if matches!(
        handoff.status,
        StagingHandoffStatus::Staged | StagingHandoffStatus::InboundRemoved
    ) && destination.is_dir()
    {
        let tree_hash = verify_existing_receipt(&destination, &handoff)?;
        finish_inbound_cleanup(
            store,
            pool,
            worker,
            item.id,
            &inbound,
            &source.root,
            &handoff.source_generation,
            &tree_hash,
            now + started.elapsed(),
            #[cfg(feature = "test-support")]
            test_hook.as_ref(),
        )
        .await?;
        return Ok(OwnedSource {
            root: destination,
            tree_hash,
        });
    }

    if !matches!(handoff.status, StagingHandoffStatus::Intent) {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "{} is missing for completed handoff {}",
            destination.display(),
            item.id
        )));
    }

    let inbound_for_task = inbound.clone();
    let destination_for_task = destination.clone();
    let staging_root_for_task = staging_root.clone();
    let work_item_id = item.id;
    #[cfg(feature = "test-support")]
    let hook_for_task = test_hook.clone();
    let expected_snapshot = expected_snapshot_signature.map(str::to_owned);
    let ready_marker = config.ready_marker_name.clone();
    let generation = handoff.source_generation.clone();
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let staged = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if destination_for_task
            .try_exists()
            .map_err(|e| io_error(&destination_for_task, e))?
        {
            return recover_finalized_tree(
                &inbound_for_task,
                &destination_for_task,
                &generation,
                expected_snapshot.as_deref(),
                &ready_marker,
            );
        }
        if source_generation(&inbound_for_task)? != generation {
            return Err(AcquisitionError::IncompleteReceipt(
                "inbound directory was replaced after handoff intent".to_owned(),
            ));
        }
        let snapshot = snapshot_release_directory(&inbound_for_task, &ready_marker)
            .map_err(|error| AcquisitionError::IncompleteReceipt(error.to_string()))?;
        if expected_snapshot.as_ref().is_some_and(|expected| {
            !crate::snapshot::signatures_equal(expected, &snapshot.signature)
        }) {
            return Err(AcquisitionError::SourceChanged);
        }
        stage_source(
            &inbound_for_task,
            &destination_for_task,
            &staging_root_for_task,
            work_item_id,
            #[cfg(feature = "test-support")]
            hook_for_task.as_ref(),
        )
    })
    .await
    .map_err(|error| AcquisitionError::Task(error.to_string()))??;

    // The final staged tree now exists; a crash here is recovered by the
    // adoption branch above (receipt verified against the on-disk tree).
    #[cfg(feature = "test-support")]
    inject_test_fault(
        test_hook.as_ref(),
        AcquisitionFaultPoint::AfterFinalizationBeforeReceipt,
        &destination,
    )?;
    store
        .record_staging_receipt(
            pool,
            item.id,
            worker,
            staged.method,
            &staged.tree_hash,
            now + started.elapsed(),
        )
        .await?;
    #[cfg(feature = "test-support")]
    inject_test_fault(
        test_hook.as_ref(),
        AcquisitionFaultPoint::AfterReceiptBeforeInboundRemoval,
        &inbound,
    )?;
    finish_inbound_cleanup(
        store,
        pool,
        worker,
        item.id,
        &inbound,
        &source.root,
        &handoff.source_generation,
        &staged.tree_hash,
        now + started.elapsed(),
        #[cfg(feature = "test-support")]
        test_hook.as_ref(),
    )
    .await?;
    Ok(OwnedSource {
        root: destination,
        tree_hash: staged.tree_hash,
    })
}

/// Resolves a completed acquisition only. Downstream stages never fall back
/// to the historic inbound path.
pub async fn owned_source_root(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    item: &ReleaseWorkItem,
) -> Result<OwnedSource, AcquisitionError> {
    let handoff = store.staging_handoff(pool, item.id).await?.ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("no staging handoff intent".to_owned())
    })?;
    if !matches!(
        handoff.status,
        StagingHandoffStatus::Staged | StagingHandoffStatus::InboundRemoved
    ) {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "handoff status is {:?}",
            handoff.status
        )));
    }
    let staging_root = config.staging_root.as_ref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt(
            "MELODEE__INGEST__STAGING_ROOT is not configured".to_owned(),
        )
    })?;
    let root = staging_root.join(&handoff.staging_relative_path);
    let verify_root = root.clone();
    let tree_hash = crate::admission::run(move || verify_existing_receipt(&verify_root, &handoff))
        .await
        .map_err(AcquisitionError::Task)??;
    Ok(OwnedSource { root, tree_hash })
}

/// Operator-supplied evidence for binding an ambiguous historical path.
pub struct LegacyPathReview<'a> {
    pub legacy_id: uuid::Uuid,
    pub reviewed_id: uuid::Uuid,
    pub receipt_tree_hash: &'a str,
    pub actor: &'a str,
    pub reason: &'a str,
    pub now: OffsetDateTime,
}

/// Independently verify staged evidence before recording a reviewed binding.
pub async fn resolve_legacy_inbound_path(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    review: &LegacyPathReview<'_>,
) -> Result<bool, AcquisitionError> {
    let legacy = store.get(pool, review.legacy_id).await?;
    let reviewed = store.get(pool, review.reviewed_id).await?;
    let relative = crate::source::decode_candidate_relative_path(&reviewed.source_path)
        .map_err(|_| AcquisitionError::InvalidSourcePath(reviewed.source_path.clone()))?;
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || crate::source::candidate_relative_path(&relative) != reviewed.source_path
    {
        return Err(AcquisitionError::InvalidSourcePath(reviewed.source_path));
    }
    let lossy = crate::source::legacy_candidate_relative_path(&relative);
    if lossy != legacy.source_path {
        return Err(AcquisitionError::IncompleteReceipt(
            "reviewed path does not match the historical lossy spelling".to_owned(),
        ));
    }
    let owned = owned_source_root(config, store, pool, &reviewed).await?;
    if owned.tree_hash != review.receipt_tree_hash {
        return Err(AcquisitionError::IncompleteReceipt(
            "reviewed hash does not match the independently verified staged receipt".to_owned(),
        ));
    }
    Ok(store
        .resolve_legacy_inbound_path(
            pool,
            &melodee_persistence::LegacyPathResolution {
                legacy_id: legacy.id,
                reviewed_id: reviewed.id,
                reviewed_lossy_path: &lossy,
                receipt_tree_hash: review.receipt_tree_hash,
                actor: review.actor,
                reason: review.reason,
                now: review.now,
            },
        )
        .await?)
}

/// Detects an unexpected staged-source mutation before and after processing.
pub fn verify_owned_source(source: &OwnedSource) -> Result<(), AcquisitionError> {
    let actual = tree_hash(&source.root)?;
    if actual == source.tree_hash {
        Ok(())
    } else {
        Err(AcquisitionError::IncompleteReceipt(format!(
            "staged source integrity mismatch at {}",
            source.root.display()
        )))
    }
}

/// Occurrence identity evidence for delayed cleanup. On Unix the directory's
/// device+inode identifies the occurrence: removing and recreating the path
/// yields a different inode, while mutating entries in place does not. This
/// split is deliberate — in-place mutation is the same occurrence whose
/// *content* the verified tree hash governs (a mismatch there retains the
/// bytes), whereas a replaced inode is a new occurrence that a delayed cleanup
/// must never touch. All platforms additionally compare the complete tree
/// hash before removal, so the fences are independent and both must pass.
pub fn source_generation(path: &Path) -> Result<String, AcquisitionError> {
    let directory = open_directory(path).map_err(|error| io_error(path, error))?;
    source_generation_from_directory(&directory, path)
}

pub(crate) fn source_generation_from_directory(
    directory: &std::fs::File,
    path: &Path,
) -> Result<String, AcquisitionError> {
    let metadata = directory
        .metadata()
        .map_err(|error| io_error(path, error))?;
    if !metadata.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let modified = metadata.modified().map_err(|error| io_error(path, error))?;
        Ok(format!("metadata:{modified:?}:{}", metadata.len()))
    }
}

struct StagedTree {
    method: StagingHandoffMethod,
    tree_hash: String,
}

/// An intent can outlive the filesystem operation that finalized the tree.
fn recover_finalized_tree(
    inbound: &Path,
    destination: &Path,
    generation: &str,
    expected_snapshot: Option<&str>,
    ready_marker: &str,
) -> Result<StagedTree, AcquisitionError> {
    let destination_generation = source_generation(destination)?;
    // A Unix directory keeps its device and inode across rename. Weaker
    // platform generation markers cannot prove that a lone tree is the original.
    let renamed = generation.starts_with("unix:") && destination_generation == generation;
    let proof_root = if renamed { destination } else { inbound };
    if source_generation(proof_root)? != generation {
        return Err(AcquisitionError::IncompleteReceipt(
            "final staging directory does not match the original inbound occurrence".to_owned(),
        ));
    }
    let snapshot = snapshot_release_directory(proof_root, ready_marker)
        .map_err(|error| AcquisitionError::IncompleteReceipt(error.to_string()))?;
    if (renamed && expected_snapshot.is_none())
        || expected_snapshot
            .is_some_and(|s| !crate::snapshot::signatures_equal(s, &snapshot.signature))
    {
        return Err(AcquisitionError::IncompleteReceipt(
            "final staging handoff conflicts with the settled source snapshot".to_owned(),
        ));
    }
    let staged_hash = tree_hash(destination)?;
    let method = if renamed {
        let after = snapshot_release_directory(destination, ready_marker)
            .map_err(|error| AcquisitionError::IncompleteReceipt(error.to_string()))?;
        if after.signature != snapshot.signature {
            return Err(AcquisitionError::IncompleteReceipt(
                "renamed staging directory changed during verification".to_owned(),
            ));
        }
        StagingHandoffMethod::Rename
    } else if tree_hash(inbound)? == staged_hash {
        StagingHandoffMethod::VerifiedCopy
    } else {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "existing final staging tree conflicts with inbound occurrence at {}",
            destination.display()
        )));
    };
    if source_generation(destination)? != destination_generation {
        return Err(AcquisitionError::IncompleteReceipt(
            "staging directory replaced during verification".to_owned(),
        ));
    }
    // A crash may have interrupted the directory syncs after the rename.
    if let Some(parent) = destination.parent() {
        sync_directory(parent)?;
    }
    if let Some(parent) = inbound.parent() {
        sync_directory(parent)?;
    }
    Ok(StagedTree {
        method,
        tree_hash: staged_hash,
    })
}

fn stage_source(
    inbound: &Path,
    destination: &Path,
    staging_root: &Path,
    work_item_id: uuid::Uuid,
    #[cfg(feature = "test-support")] test_hook: Option<&AcquisitionTestHook>,
) -> Result<StagedTree, AcquisitionError> {
    let parent = destination.parent().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("staging destination has no parent".to_owned())
    })?;
    create_directory_tree(parent).map_err(|error| io_error(parent, error))?;

    // A hidden partial from a crashed copy attempt of this same work item is
    // attempt scratch: the deterministic name can only belong to this item's
    // fenced worker, so removing it before a rename retry cannot touch another
    // acquisition's bytes.
    let releases = parent.parent().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("staging release parent is missing".to_owned())
    })?;
    let partial = releases.join(format!(".{work_item_id}.acquiring"));
    if partial.exists() {
        remove_partial(&partial, releases)?;
    }

    #[cfg(feature = "test-support")]
    let force_copy = test_hook.is_some_and(|hook| hook.force_copy);
    #[cfg(not(feature = "test-support"))]
    let force_copy = false;

    let rename_result = if force_copy {
        // Test-only cross-filesystem forcing: behave exactly as if the kernel
        // returned EXDEV without exposing any production switch.
        Err(io::Error::from_raw_os_error(18))
    } else {
        rename_noreplace(inbound, destination)
    };

    match rename_result {
        Ok(()) => {
            sync_directory(parent)?;
            sync_directory(staging_root)?;
            if let Some(inbound_parent) = inbound.parent() {
                sync_directory(inbound_parent)?;
            }
            Ok(StagedTree {
                method: StagingHandoffMethod::Rename,
                tree_hash: tree_hash(destination)?,
            })
        }
        Err(error) if is_cross_device(&error) => {
            fs::create_dir(&partial).map_err(|error| io_error(&partial, error))?;
            let before_hash = tree_hash(inbound)?;
            #[cfg(feature = "test-support")]
            if let Some(hook) = test_hook {
                hook.pause_copy_window();
            }
            copy_tree(
                inbound,
                &partial,
                #[cfg(feature = "test-support")]
                test_hook,
            )?;
            #[cfg(feature = "test-support")]
            inject_test_fault(
                test_hook,
                AcquisitionFaultPoint::AfterCopyBeforeVerification,
                inbound,
            )?;
            let after_hash = tree_hash(inbound)?;
            let copied_hash = tree_hash(&partial)?;
            if before_hash != after_hash || before_hash != copied_hash {
                remove_partial(&partial, releases)?;
                return Err(AcquisitionError::SourceChanged);
            }
            #[cfg(feature = "test-support")]
            inject_test_fault(
                test_hook,
                AcquisitionFaultPoint::AfterVerificationBeforeFinalization,
                &partial,
            )?;
            sync_directory(&partial)?;
            rename_noreplace(&partial, destination)
                .map_err(|error| io_error(destination, error))?;
            sync_directory(parent)?;
            sync_directory(staging_root)?;
            Ok(StagedTree {
                method: StagingHandoffMethod::VerifiedCopy,
                tree_hash: copied_hash,
            })
        }
        Err(error) => Err(io_error(inbound, error)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_inbound_cleanup(
    store: &WorkItemStore,
    pool: &PgPool,
    worker: &WorkerId,
    item_id: uuid::Uuid,
    inbound: &Path,
    inbound_root: &Path,
    expected_generation: &str,
    expected_tree_hash: &str,
    now: OffsetDateTime,
    #[cfg(feature = "test-support")] test_hook: Option<&AcquisitionTestHook>,
) -> Result<(), AcquisitionError> {
    let inbound = inbound.to_path_buf();
    let inbound_root = inbound_root.to_path_buf();
    let expected_generation = expected_generation.to_owned();
    let expected_tree_hash = expected_tree_hash.to_owned();
    #[cfg(feature = "test-support")]
    let fault_path = inbound.clone();
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| AcquisitionError::Task(error.to_string()))?;
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        cleanup_inbound(
            &inbound,
            &inbound_root,
            &expected_generation,
            &expected_tree_hash,
        )
    })
    .await
    .map_err(|error| AcquisitionError::Task(error.to_string()))??;
    // A crash here has already physically drained the matching occurrence;
    // recovery re-enters cleanup, observes AlreadyAbsent/Replaced, and records
    // the same bookkeeping idempotently.
    #[cfg(feature = "test-support")]
    inject_test_fault(
        test_hook,
        AcquisitionFaultPoint::AfterInboundRemovalBeforeBookkeeping,
        fault_path.as_path(),
    )?;
    match result {
        CleanupResult::Removed | CleanupResult::AlreadyAbsent => {
            store
                .record_inbound_removed(pool, item_id, worker, now)
                .await?;
        }
        CleanupResult::Replaced => {
            store
                .record_inbound_replaced(pool, item_id, worker, now)
                .await?;
        }
    }
    Ok(())
}

enum CleanupResult {
    Removed,
    AlreadyAbsent,
    Replaced,
}

fn cleanup_inbound(
    inbound: &Path,
    inbound_root: &Path,
    expected_generation: &str,
    expected_tree_hash: &str,
) -> Result<CleanupResult, AcquisitionError> {
    if !inbound.starts_with(inbound_root)
        || inbound.parent() != Some(inbound_root)
        || inbound.file_name().is_none()
    {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "refusing cleanup outside direct inbound root: {}",
            inbound.display()
        )));
    }
    let metadata = match fs::symlink_metadata(inbound) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CleanupResult::AlreadyAbsent);
        }
        Err(error) => return Err(io_error(inbound, error)),
    };
    // A delayed cleanup must never follow a path that was replaced with a
    // symlink (or any other non-directory).  Treat the replacement as a new
    // occurrence and retain it for the next reconciliation pass.
    if !metadata.is_dir() {
        return Ok(CleanupResult::Replaced);
    }
    if source_generation(inbound)? != expected_generation
        || tree_hash(inbound)? != expected_tree_hash
    {
        return Ok(CleanupResult::Replaced);
    }
    // Re-open and remove through verified directory descriptors after the
    // generation/hash check. The occurrence can be replaced between that
    // check and cleanup; path-based recursion would make that race harder to
    // audit and could follow an unexpected child.
    remove_directory_tree(inbound).map_err(|error| io_error(inbound, error))?;
    sync_directory(inbound_root)?;
    Ok(CleanupResult::Removed)
}

fn verify_existing_receipt(
    destination: &Path,
    handoff: &StagingHandoff,
) -> Result<String, AcquisitionError> {
    let expected = handoff.tree_hash.as_deref().ok_or_else(|| {
        AcquisitionError::IncompleteReceipt("receipt has no verified tree hash".to_owned())
    })?;
    let actual = tree_hash(destination)?;
    if actual != expected {
        return Err(AcquisitionError::IncompleteReceipt(format!(
            "final staging tree differs from durable receipt at {}",
            destination.display()
        )));
    }
    Ok(actual)
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    #[cfg(feature = "test-support")] test_hook: Option<&AcquisitionTestHook>,
) -> Result<(), AcquisitionError> {
    let mut queue = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((current_source, current_destination)) = queue.pop() {
        let mut entries = fs::read_dir(&current_source)
            .map_err(|error| io_error(&current_source, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&current_source, error))?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            let source_path = entry.path();
            let destination_path = current_destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path)
                .map_err(|error| io_error(&source_path, error))?;
            if metadata.is_dir() {
                create_directory_tree(&destination_path)
                    .map_err(|error| io_error(&destination_path, error))?;
                queue.push((source_path, destination_path));
            } else if metadata.is_file() {
                copy_regular_file(
                    &source_path,
                    &destination_path,
                    #[cfg(feature = "test-support")]
                    test_hook,
                )?;
                #[cfg(feature = "test-support")]
                inject_test_fault(test_hook, AcquisitionFaultPoint::DuringCopy, &source_path)?;
            } else {
                // Symlinks and other special files are not durable original
                // source bytes and are never followed (a symlink could escape
                // the inbound root). Skip them here; the inspection layer
                // likewise ignores symlinks (see melodee-media lib.rs), so the
                // staged tree and the manifest stay consistent.
                continue;
            }
        }
    }
    Ok(())
}

fn copy_regular_file(
    source: &Path,
    destination: &Path,
    #[cfg(feature = "test-support")] test_hook: Option<&AcquisitionTestHook>,
) -> Result<(), AcquisitionError> {
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::AcquisitionCopy,
    );
    // The metadata check in copy_tree is only a snapshot. Re-open the source
    // through the descriptor-relative no-follow boundary so a concurrent
    // replacement cannot turn a regular source into an external symlink.
    let input = open_read(source).map_err(|error| io_error(source, error))?;
    let output = open_write_new(destination).map_err(|error| io_error(destination, error))?;
    #[cfg(feature = "test-support")]
    if let Some(hook) = test_hook
        && let Some(fault) = hook.take_write_fault()
    {
        // The error arrives at the same boundary as a real write failure:
        // an `io::Error` attributed to the destination path with no special
        // variant, so production error handling is unchanged.
        let error = match fault {
            AcquisitionWriteFault::NoSpaceLeft => io::Error::from_raw_os_error(28),
        };
        return Err(io_error(destination, error));
    }
    let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, input);
    let mut writer = BufWriter::with_capacity(COPY_BUFFER_BYTES, output);
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            operation.mark_bytes_incomplete();
            io_error(source, error)
        })?;
        if read == 0 {
            break;
        }
        operation.add_read(read as u64);
        #[cfg(feature = "test-support")]
        if let Some(hook) = test_hook {
            hook.delay_write();
        }
        writer.write_all(&buffer[..read]).map_err(|error| {
            operation.mark_bytes_incomplete();
            io_error(destination, error)
        })?;
        operation.add_written(read as u64);
    }
    let output = writer.into_inner().map_err(|error| {
        operation.mark_bytes_incomplete();
        io_error(destination, error.into_error())
    })?;
    output
        .sync_all()
        .map_err(|error| io_error(destination, error))?;
    operation.finish();
    Ok(())
}

fn tree_hash(root: &Path) -> Result<String, AcquisitionError> {
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
    );
    let metadata = fs::symlink_metadata(root).map_err(|error| io_error(root, error))?;
    if !metadata.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let mut hasher = blake3::Hasher::new();
    let mut queue = vec![(root.to_path_buf(), PathBuf::new())];
    while let Some((directory, relative)) = queue.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| io_error(&directory, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&directory, error))?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let next_relative = relative.join(entry.file_name());
            let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
            if metadata.is_dir() {
                hash_path(&mut hasher, &next_relative, &metadata);
                queue.push((path, next_relative));
            } else if metadata.is_file() {
                hash_path(&mut hasher, &next_relative, &metadata);
                // Re-open through the same no-follow boundary used by copy
                // so the hash cannot be redirected after symlink_metadata.
                let file = open_read(&path).map_err(|error| io_error(&path, error))?;
                let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, file);
                let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
                loop {
                    let read = reader.read(&mut buffer).map_err(|error| {
                        operation.mark_bytes_incomplete();
                        io_error(&path, error)
                    })?;
                    if read == 0 {
                        break;
                    }
                    operation.add_read(read as u64);
                    hasher.update(&buffer[..read]);
                }
            } else {
                // Symlinks and other special files contribute no durable bytes
                // and are never followed; skipping keeps the verification hash
                // stable across same- and cross-filesystem acquisition and
                // matches the inspection layer's symlink handling.
                continue;
            }
        }
    }
    let digest = hasher.finalize().to_hex().to_string();
    operation.finish();
    Ok(digest)
}

fn hash_path(hasher: &mut blake3::Hasher, path: &Path, metadata: &fs::Metadata) {
    hash_path_parts(hasher, path, metadata.is_dir(), metadata.len());
}

fn hash_path_parts(hasher: &mut blake3::Hasher, path: &Path, is_dir: bool, length: u64) {
    hasher.update(if is_dir { b"d" } else { b"f" });
    for component in path.components() {
        if let Component::Normal(part) = component {
            hasher.update(part.to_string_lossy().as_bytes());
            hasher.update(&[0]);
        }
    }
    hasher.update(&length.to_le_bytes());
}

/// Hash a custody tree using a versioned, native-path encoding. Existing
/// staging receipts retain their legacy encoding; they are not custody proof.
pub(crate) fn tree_hash_from_pinned_directory(
    root: &std::fs::File,
) -> Result<String, AcquisitionError> {
    Ok(tree_hashes_from_pinned_directory(root)?.0)
}

/// Both digests describe the same traversal and byte reads. The legacy digest
/// is only for comparing existing receipts, never by itself for custody.
pub(crate) fn tree_hashes_from_pinned_directory(
    root: &std::fs::File,
) -> Result<(String, String), AcquisitionError> {
    tree_hashes_from_pinned_directory_with_hook(root, |_| {})
}

fn tree_hashes_from_pinned_directory_with_hook(
    root: &std::fs::File,
    mut after_file: impl FnMut(&Path),
) -> Result<(String, String), AcquisitionError> {
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::AcquisitionTreeHash,
    );
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    let directory = cap_std::fs::Dir::from_std_file(
        root.try_clone()
            .map_err(|error| io_error(Path::new("<pinned source>"), error))?,
    );
    let mut hasher = blake3::Hasher::new();
    let mut legacy_hasher = blake3::Hasher::new();
    hasher.update(b"melodee-inbound-custody-tree-v2\0");
    #[cfg(unix)]
    hasher.update(b"unix\0");
    #[cfg(windows)]
    hasher.update(b"windows\0");
    let mut queue = vec![(directory, PathBuf::new())];
    while let Some((directory, relative)) = queue.pop() {
        let mut entries = directory
            .entries()
            .map_err(|error| io_error(&relative, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&relative, error))?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let path = relative.join(&name);
            let metadata = directory
                .symlink_metadata(&name)
                .map_err(|error| io_error(&path, error))?;
            if metadata.is_dir() {
                hash_custody_path(&mut hasher, &path, true, metadata.len())?;
                hash_path_parts(&mut legacy_hasher, &path, true, metadata.len());
                let child = directory
                    .open_dir_nofollow(&name)
                    .map_err(|error| io_error(&path, error))?;
                queue.push((child, path));
            } else if metadata.is_file() {
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                let file = directory
                    .open_with(&name, &options)
                    .map_err(|error| io_error(&path, error))?;
                let opened = file.metadata().map_err(|error| io_error(&path, error))?;
                if !opened.is_file() || opened.len() != metadata.len() {
                    return Err(AcquisitionError::SourceChanged);
                }
                hash_custody_path(&mut hasher, &path, false, metadata.len())?;
                hash_path_parts(&mut legacy_hasher, &path, false, metadata.len());
                let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, file.into_std());
                let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
                let mut bytes_read = 0_u64;
                loop {
                    let read = reader.read(&mut buffer).map_err(|error| {
                        operation.mark_bytes_incomplete();
                        io_error(&path, error)
                    })?;
                    if read == 0 {
                        break;
                    }
                    operation.add_read(read as u64);
                    bytes_read = bytes_read
                        .checked_add(read as u64)
                        .ok_or(AcquisitionError::SourceChanged)?;
                    hasher.update(&buffer[..read]);
                    legacy_hasher.update(&buffer[..read]);
                }
                if bytes_read != metadata.len()
                    || reader
                        .get_ref()
                        .metadata()
                        .map_err(|error| io_error(&path, error))?
                        .len()
                        != metadata.len()
                {
                    return Err(AcquisitionError::SourceChanged);
                }
                after_file(&path);
            }
        }
    }
    let hashes = (
        format!("v2:{}", hasher.finalize().to_hex()),
        legacy_hasher.finalize().to_hex().to_string(),
    );
    operation.finish();
    Ok(hashes)
}

fn hash_custody_path(
    hasher: &mut blake3::Hasher,
    path: &Path,
    is_dir: bool,
    length: u64,
) -> Result<(), AcquisitionError> {
    hasher.update(if is_dir { b"d" } else { b"f" });
    let components = path.components().count() as u64;
    hasher.update(&components.to_le_bytes());
    for component in path.components() {
        let Component::Normal(part) = component else {
            return Err(AcquisitionError::InvalidSourcePath(
                "non-relative custody path".to_owned(),
            ));
        };
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = part.as_bytes();
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            hasher.update(&(part.encode_wide().count() as u64).to_le_bytes());
            for unit in part.encode_wide() {
                hasher.update(&unit.to_le_bytes());
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = part;
            return Err(AcquisitionError::InvalidSourcePath(
                "native custody path encoding unavailable".to_owned(),
            ));
        }
    }
    // Directory sizes are filesystem allocation metadata, not content, and
    // need not survive cross-filesystem copies. File lengths delimit content.
    hasher.update(&(if is_dir { 0 } else { length }).to_le_bytes());
    Ok(())
}

fn remove_partial(partial: &Path, releases_root: &Path) -> Result<(), AcquisitionError> {
    let metadata = fs::symlink_metadata(partial).map_err(|error| io_error(partial, error))?;
    if partial.parent() != Some(releases_root) || !metadata.is_dir() {
        return Err(AcquisitionError::InvalidSourcePath(format!(
            "refusing to remove unexpected partial staging path {}",
            partial.display()
        )));
    }
    remove_directory_tree(partial).map_err(|error| io_error(partial, error))
}

fn sync_directory(path: &Path) -> Result<(), AcquisitionError> {
    open_directory(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn is_cross_device(error: &io::Error) -> bool {
    error.raw_os_error() == Some(18)
}

fn find_source<'a>(
    config: &'a IngestionConfig,
    item: &ReleaseWorkItem,
) -> Option<&'a InboundSource> {
    config.sources.iter().find(|source| {
        source.id == item.source_library_id
            || (source.id.is_none() && item.source_library_id.is_none())
    })
}

fn resolve_candidate(
    source: &InboundSource,
    source_path: &str,
) -> Result<PathBuf, AcquisitionError> {
    let relative = crate::source::decode_candidate_relative_path(source_path)
        .map_err(|_| AcquisitionError::InvalidSourcePath(source_path.to_owned()))?;
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AcquisitionError::InvalidSourcePath(source_path.to_owned()));
    }
    let path = source.root.join(relative);
    if path.parent() != Some(source.root.as_path()) {
        return Err(AcquisitionError::InvalidSourcePath(source_path.to_owned()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "melodee-acquisition-{label}-{}",
            uuid::Uuid::now_v7().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn operation_counters_record_successful_acquisition_reads_and_copies() {
        struct TraceSubscriber;
        impl tracing::Subscriber for TraceSubscriber {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, _: &tracing::Event<'_>) {}
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }
        fn counters(name: &str) -> melodee_media::operation_metrics::OperationSnapshot {
            melodee_media::operation_metrics::snapshot()
                .into_iter()
                .find(|value| value.operation == name)
                .unwrap()
        }
        tracing::subscriber::with_default(TraceSubscriber, || {
            let root = temporary_root("operation-counts");
            let source = root.join("source");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("file"), b"abc").unwrap();
            let hash_before = counters("acquisition_tree_hash");
            let copy_before = counters("acquisition_copy");
            tree_hash(&source).unwrap();
            #[cfg(feature = "test-support")]
            copy_regular_file(&source.join("file"), &root.join("copy"), None).unwrap();
            #[cfg(not(feature = "test-support"))]
            copy_regular_file(&source.join("file"), &root.join("copy")).unwrap();
            let hash_after = counters("acquisition_tree_hash");
            let copy_after = counters("acquisition_copy");
            assert_eq!(hash_after.operations - hash_before.operations, 1);
            assert_eq!(hash_after.succeeded - hash_before.succeeded, 1);
            assert_eq!(hash_after.read_bytes - hash_before.read_bytes, 3);
            assert_eq!(copy_after.operations - copy_before.operations, 1);
            assert_eq!(copy_after.succeeded - copy_before.succeeded, 1);
            assert_eq!(copy_after.read_bytes - copy_before.read_bytes, 3);
            assert_eq!(copy_after.written_bytes - copy_before.written_bytes, 3);
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_rename_requires_the_original_directory_and_settled_snapshot() {
        let root = temporary_root("rename-evidence");
        let inbound = root.join("inbound");
        let staged = root.join("staged");
        fs::create_dir_all(&inbound).unwrap();
        fs::write(inbound.join("track"), b"original").unwrap();
        let generation = source_generation(&inbound).unwrap();
        let original = snapshot_release_directory(&inbound, ".ready")
            .unwrap()
            .signature;
        fs::rename(&inbound, &staged).unwrap();
        assert!(recover_finalized_tree(&inbound, &staged, &generation, None, ".ready").is_err());
        fs::write(staged.join("extra"), b"changed").unwrap();
        assert!(
            recover_finalized_tree(&inbound, &staged, &generation, Some(&original), ".ready")
                .is_err()
        );
        fs::remove_file(staged.join("extra")).unwrap();
        fs::rename(&staged, root.join("original")).unwrap();
        fs::create_dir(&staged).unwrap();
        assert!(
            recover_finalized_tree(&inbound, &staged, &generation, Some(&original), ".ready")
                .is_err()
        );
        fs::remove_dir(&staged).unwrap();
        std::os::unix::fs::symlink(root.join("original"), &staged).unwrap();
        assert!(
            recover_finalized_tree(&inbound, &staged, &generation, Some(&original), ".ready")
                .is_err()
        );
        assert_eq!(fs::read(root.join("original/track")).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_filesystem_stage_moves_the_complete_tree_without_changing_bytes() {
        let root = temporary_root("rename");
        let inbound = root.join("inbound/release with spaces");
        fs::create_dir_all(inbound.join("disc-01")).unwrap();
        fs::write(inbound.join("disc-01/track-01.flac"), b"audio-a").unwrap();
        fs::write(inbound.join("cover.jpg"), b"artwork-a").unwrap();
        let expected = tree_hash(&inbound).unwrap();
        let staging = root.join("staging");
        let destination = staging.join("releases/test/source");

        let staged = stage_source(
            &inbound,
            &destination,
            &staging,
            uuid::Uuid::now_v7(),
            #[cfg(feature = "test-support")]
            None,
        )
        .unwrap();

        assert_eq!(staged.method, StagingHandoffMethod::Rename);
        assert_eq!(staged.tree_hash, expected);
        assert!(!inbound.exists());
        assert_eq!(tree_hash(&destination).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn streamed_copy_preserves_nested_unicode_and_punctuation_paths() {
        let root = temporary_root("copy");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("disc 01/(live) & friends")).unwrap();
        fs::write(
            source.join("disc 01/(live) & friends/01 - cafe.flac"),
            vec![42_u8; COPY_BUFFER_BYTES + 17],
        )
        .unwrap();
        fs::create_dir_all(&destination).unwrap();
        let expected = tree_hash(&source).unwrap();

        copy_tree(
            &source,
            &destination,
            #[cfg(feature = "test-support")]
            None,
        )
        .unwrap();

        assert_eq!(tree_hash(&destination).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delayed_cleanup_never_removes_a_reused_inbound_path() {
        let root = temporary_root("replacement");
        let inbound_root = root.join("inbound");
        let candidate = inbound_root.join("release");
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("track.flac"), b"occurrence-a").unwrap();
        let generation = source_generation(&candidate).unwrap();
        let hash = tree_hash(&candidate).unwrap();

        fs::remove_dir_all(&candidate).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("track.flac"), b"occurrence-b").unwrap();

        assert!(matches!(
            cleanup_inbound(&candidate, &inbound_root, &generation, &hash).unwrap(),
            CleanupResult::Replaced
        ));
        assert_eq!(
            fs::read(candidate.join("track.flac")).unwrap(),
            b"occurrence-b"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn delayed_cleanup_never_follows_a_replaced_inbound_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("replacement-symlink");
        let inbound_root = root.join("inbound");
        let candidate = inbound_root.join("release");
        let outside = root.join("outside");
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("track.flac"), b"occurrence-a").unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.me"), b"must-survive").unwrap();
        let generation = source_generation(&candidate).unwrap();
        let hash = tree_hash(&candidate).unwrap();

        fs::remove_dir_all(&candidate).unwrap();
        symlink(&outside, &candidate).unwrap();

        assert!(matches!(
            cleanup_inbound(&candidate, &inbound_root, &generation, &hash).unwrap(),
            CleanupResult::Replaced
        ));
        assert!(candidate.is_symlink());
        assert_eq!(fs::read(outside.join("keep.me")).unwrap(), b"must-survive");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tree_hash_and_copy_tree_skip_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("symlinks");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("track.flac"), b"audio").unwrap();
        // An internal symlink and a dangling escape symlink must not be
        // followed, copied, or hashed.
        symlink(source.join("track.flac"), source.join("link.flac")).unwrap();
        symlink(root.join("outside"), source.join("escape")).unwrap();

        let expected = {
            let bare = root.join("bare");
            fs::create_dir_all(&bare).unwrap();
            fs::write(bare.join("track.flac"), b"audio").unwrap();
            tree_hash(&bare).unwrap()
        };

        assert_eq!(tree_hash(&source).unwrap(), expected);

        let destination = root.join("destination");
        fs::create_dir_all(&destination).unwrap();
        copy_tree(
            &source,
            &destination,
            #[cfg(feature = "test-support")]
            None,
        )
        .unwrap();
        assert!(destination.join("track.flac").is_file());
        assert!(!destination.join("link.flac").exists());
        assert!(!destination.join("escape").exists());
        assert_eq!(tree_hash(&destination).unwrap(), expected);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pinned_tree_hash_survives_path_replacement() {
        let root = temporary_root("pinned-tree-hash");
        let source = root.join("source");
        fs::create_dir_all(source.join("disc")).unwrap();
        fs::write(source.join("first.flac"), b"first").unwrap();
        fs::write(source.join("disc/second.flac"), b"second").unwrap();
        let pinned = open_directory(&source).unwrap();
        let (expected, legacy) = tree_hashes_from_pinned_directory(&pinned).unwrap();
        assert_eq!(legacy, tree_hash(&source).unwrap());
        assert_eq!(tree_hash_from_pinned_directory(&pinned).unwrap(), expected);
        fs::rename(&source, root.join("original")).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("different.flac"), b"replacement").unwrap();
        assert_eq!(tree_hash_from_pinned_directory(&pinned).unwrap(), expected);
        let replacement = open_directory(&source).unwrap();
        assert_ne!(
            tree_hash_from_pinned_directory(&replacement).unwrap(),
            expected
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn custody_hash_distinguishes_invalid_utf8_from_literal_replacement_character() {
        use std::os::unix::ffi::OsStringExt;

        let root = temporary_root("custody-raw-path-hash");
        let raw = root.join("raw");
        let unicode = root.join("unicode");
        fs::create_dir_all(&raw).unwrap();
        fs::create_dir_all(&unicode).unwrap();
        fs::write(
            raw.join(std::ffi::OsString::from_vec(b"a-\xff".to_vec())),
            b"same",
        )
        .unwrap();
        fs::write(unicode.join("a-\u{fffd}"), b"same").unwrap();
        assert_eq!(tree_hash(&raw).unwrap(), tree_hash(&unicode).unwrap());
        let (raw_hash, raw_legacy) =
            tree_hashes_from_pinned_directory(&open_directory(&raw).unwrap()).unwrap();
        let (unicode_hash, unicode_legacy) =
            tree_hashes_from_pinned_directory(&open_directory(&unicode).unwrap()).unwrap();
        assert_eq!(raw_legacy, unicode_legacy);
        assert!(raw_hash.starts_with("v2:"));
        assert_ne!(raw_hash, unicode_hash);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn custody_recheck_detects_same_size_mutation_after_file_was_read() {
        let root = temporary_root("custody-mutation");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("first"), b"before").unwrap();
        fs::write(root.join("second"), b"second").unwrap();
        let pinned = open_directory(&root).unwrap();
        let before = tree_hashes_from_pinned_directory_with_hook(&pinned, |path| {
            if path == Path::new("first") {
                fs::write(root.join("first"), b"change").unwrap();
            }
        })
        .unwrap();
        let after = tree_hashes_from_pinned_directory(&pinned).unwrap();
        assert_ne!(before.0, after.0);
        assert_ne!(before.1, after.1);
        assert_eq!(after.1, tree_hash(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn direct_copy_rejects_a_symlinked_source_at_open_time() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("copy-source-symlink");
        let source = root.join("source.flac");
        let external = root.join("external.flac");
        let destination = root.join("destination.flac");
        let external_destination = root.join("external-destination.flac");
        let symlink_destination = root.join("symlink-destination.flac");
        fs::write(&external, b"authoritative outside bytes").unwrap();
        fs::write(&external_destination, b"destination must survive").unwrap();
        symlink(&external, &source).unwrap();
        symlink(&external_destination, &symlink_destination).unwrap();

        assert!(
            copy_regular_file(
                &source,
                &destination,
                #[cfg(feature = "test-support")]
                None,
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read(&external).unwrap(), b"authoritative outside bytes");

        fs::write(&source, b"inbound bytes").unwrap();
        assert!(
            copy_regular_file(
                &source,
                &symlink_destination,
                #[cfg(feature = "test-support")]
                None,
            )
            .is_err()
        );
        assert!(symlink_destination.is_symlink());
        assert_eq!(
            fs::read(&external_destination).unwrap(),
            b"destination must survive"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn partial_cleanup_never_follows_an_internal_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("partial-symlink");
        let releases = root.join("releases");
        let partial = releases.join(".attempt.acquiring");
        let outside = root.join("outside");
        fs::create_dir_all(&partial).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("must-survive"), b"authoritative").unwrap();
        symlink(&outside, partial.join("escape")).unwrap();

        assert!(remove_partial(&partial, &releases).is_err());
        assert!(partial.is_dir());
        assert_eq!(
            fs::read(outside.join("must-survive")).unwrap(),
            b"authoritative"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn partial_cleanup_rejects_a_replaced_root_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("partial-root-symlink");
        let releases = root.join("releases");
        let partial = releases.join(".attempt.acquiring");
        let outside = root.join("outside");
        fs::create_dir_all(&releases).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("must-survive"), b"authoritative").unwrap();
        symlink(&outside, &partial).unwrap();

        assert!(remove_partial(&partial, &releases).is_err());
        assert!(partial.is_symlink());
        assert_eq!(
            fs::read(outside.join("must-survive")).unwrap(),
            b"authoritative"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
