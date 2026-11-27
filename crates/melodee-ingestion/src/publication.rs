//! Deterministic, convergent canonical filesystem publication (ADR-0116).

pub(crate) mod io_budget;
mod nfs;

use io_budget::{AssetIo, NfsIoBudget};

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "test-support")]
use std::sync::Arc;

use melodee_domain::{
    CANONICAL_LAYOUT_VERSION, ManifestPublication, PublicationAuthorization,
    PublicationInvariantError, PublicationJournalStep, PublicationPlan, ReleaseId, ReleaseManifest,
    SourceDisposition, ValidationOutcome,
};
use thiserror::Error;

use crate::ValidatedNormalization;

/// Feature-gated synchronization point for deterministic lease-loss tests.
///
/// This type and its worker configuration field are absent unless the
/// non-default `test-support` feature is explicitly enabled.
#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct PublicationCopyTestBarrier {
    state: Arc<PublicationCopyTestBarrierState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
struct PublicationCopyTestBarrierState {
    reached: AtomicBool,
    reached_notify: tokio::sync::Notify,
}

#[cfg(feature = "test-support")]
impl PublicationCopyTestBarrier {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(PublicationCopyTestBarrierState {
                reached: AtomicBool::new(false),
                reached_notify: tokio::sync::Notify::new(),
            }),
        }
    }

    pub async fn wait_until_reached(&self) {
        loop {
            if self.state.reached.load(Ordering::SeqCst) {
                return;
            }
            let notified = self.state.reached_notify.notified();
            if self.state.reached.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn pause_until_canceled(&self, cancellation: &AtomicBool) {
        self.state.reached.store(true, Ordering::SeqCst);
        self.state.reached_notify.notify_waiters();
        while !cancellation.load(Ordering::SeqCst) {
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
}

#[cfg(feature = "test-support")]
impl Default for PublicationCopyTestBarrier {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete immutable input for one filesystem publication attempt.
#[derive(Debug, Clone)]
pub struct PublicationBundle {
    pub manifest: ReleaseManifest,
    pub plan: PublicationPlan,
    pub plan_hash: String,
    pub work_root: PathBuf,
}

/// Whether this call installed the final directory or adopted a prior result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationOutcome {
    Published,
    Adopted,
}

/// Verified evidence returned only after the final directory is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub outcome: PublicationOutcome,
    pub target_relative_path: String,
    pub manifest_relative_path: String,
    pub sealed_manifest_hash: String,
}

/// A verified filesystem milestone suitable for durable journal recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationMilestone {
    pub step: PublicationJournalStep,
    pub asset_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub byte_hash: Option<String>,
}

/// Publication-plan or filesystem safety failure.
#[derive(Debug, Error)]
pub enum PublicationError {
    #[error("publication invariants failed: {0}")]
    Invariant(#[from] PublicationInvariantError),

    #[error("publication serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("input semantic manifest hash does not match the normalized manifest")]
    InputManifestHashMismatch,

    #[error("structural validation requires operator review before publication")]
    NeedsReview,

    #[error("publication was canceled before finalization")]
    Canceled,

    #[error("another process is currently publishing Release {0}")]
    Busy(ReleaseId),

    #[error("canonical target conflict at {path:?}: {reason}")]
    TargetConflict { path: PathBuf, reason: String },

    #[error("symlinks are forbidden at the canonical publication boundary: {0:?}")]
    Symlink(PathBuf),

    #[error("publication path is unsafe: {0:?}")]
    UnsafePath(PathBuf),

    #[error("publication asset is not a regular file: {0:?}")]
    NotRegularFile(PathBuf),

    #[error("publication asset size mismatch for {path:?}: expected {expected}, observed {actual}")]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },

    #[error(
        "publication asset BLAKE3 mismatch for {path:?}: expected {expected}, observed {actual}"
    )]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("failed to {operation} publication path {path:?}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Creates the immutable plan and v3 manifest after validation has passed.
pub fn build_publication_bundle(
    manifest_v2: &ReleaseManifest,
    input_manifest_hash: &str,
    release_id: ReleaseId,
    authorization: PublicationAuthorization,
    validated: ValidatedNormalization,
) -> Result<PublicationBundle, PublicationError> {
    if validated.validation.outcome != ValidationOutcome::Passed {
        return Err(PublicationError::NeedsReview);
    }
    let observed_hash = melodee_media::hash_manifest(manifest_v2)
        .map_err(|_| PublicationError::InputManifestHashMismatch)?;
    if observed_hash != input_manifest_hash {
        return Err(PublicationError::InputManifestHashMismatch);
    }
    let target_relative_path = PublicationPlan::canonical_target_path(release_id);
    let plan = PublicationPlan {
        layout_version: CANONICAL_LAYOUT_VERSION,
        release_id,
        input_manifest_hash: input_manifest_hash.to_owned(),
        authorization: authorization.clone(),
        manifest_relative_path: format!("{target_relative_path}/manifest.json"),
        target_relative_path,
        assets: validated.assets,
    };
    plan.validate()?;
    let plan_hash = hash_publication_plan(&plan)?;
    let publication = ManifestPublication {
        release_id,
        validation: validated.validation,
        authorization,
        source_disposition: SourceDisposition::Retained,
        layout_version: CANONICAL_LAYOUT_VERSION,
        plan_hash: plan_hash.clone(),
        canonical_assets: plan.assets.clone(),
    };
    let mut manifest = manifest_v2.clone();
    manifest.upgrade_for_publication(publication, &plan)?;
    Ok(PublicationBundle {
        manifest,
        plan,
        plan_hash,
        work_root: validated.work_root,
    })
}

/// Hashes exactly the immutable facts named by ADR-0116.
pub fn hash_publication_plan(plan: &PublicationPlan) -> Result<String, PublicationError> {
    plan.validate()?;
    plan.plan_hash().map_err(PublicationError::from)
}

/// Copies, verifies, seals, fsyncs, and atomically publishes one canonical bundle.
///
/// Linux NFS uses an exclusive directory reservation and installs the manifest last.
/// Other filesystems use a kernel-enforced no-replace rename. If a final path
/// already exists it is verified and adopted only when every immutable fact
/// and byte matches this plan.
pub fn publish_canonical_bundle(
    storage_root: &Path,
    bundle: &PublicationBundle,
    cancellation: &AtomicBool,
) -> Result<PublicationReceipt, PublicationError> {
    static BUDGET: std::sync::LazyLock<NfsIoBudget> =
        std::sync::LazyLock::new(NfsIoBudget::default);
    publish_canonical_bundle_with_observer(storage_root, bundle, cancellation, &BUDGET, |_| {})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationFaultPoint {
    AssetFsync,
    ManifestFsync,
    SealedTreeFsync,
    FinalRename,
    FinalParentFsync,
    NfsManifestInstalled,
}

pub(crate) fn publish_canonical_bundle_with_observer(
    storage_root: &Path,
    bundle: &PublicationBundle,
    cancellation: &AtomicBool,
    budget: &NfsIoBudget,
    mut observer: impl FnMut(PublicationMilestone),
) -> Result<PublicationReceipt, PublicationError> {
    publish_canonical_bundle_impl(
        storage_root,
        bundle,
        cancellation,
        &mut observer,
        &mut |_| Ok(()),
        nfs::on_nfs,
        budget,
        None,
    )
}

#[cfg(feature = "test-support")]
pub(crate) fn publish_canonical_bundle_with_observer_and_test_barrier(
    storage_root: &Path,
    bundle: &PublicationBundle,
    cancellation: &AtomicBool,
    budget: &NfsIoBudget,
    mut observer: impl FnMut(PublicationMilestone),
    barrier: Option<&PublicationCopyTestBarrier>,
) -> Result<PublicationReceipt, PublicationError> {
    publish_canonical_bundle_impl(
        storage_root,
        bundle,
        cancellation,
        &mut observer,
        &mut |_| Ok(()),
        nfs::on_nfs,
        budget,
        barrier,
    )
}

#[allow(clippy::too_many_arguments)]
fn publish_canonical_bundle_impl(
    storage_root: &Path,
    bundle: &PublicationBundle,
    cancellation: &AtomicBool,
    observer: &mut dyn FnMut(PublicationMilestone),
    fault: &mut dyn FnMut(PublicationFaultPoint) -> io::Result<()>,
    detect_nfs: fn(&File) -> io::Result<bool>,
    budget: &NfsIoBudget,
    #[cfg(feature = "test-support")] copy_barrier: Option<&PublicationCopyTestBarrier>,
    #[cfg(not(feature = "test-support"))] _copy_barrier: Option<&()>,
) -> Result<PublicationReceipt, PublicationError> {
    let manifest_bytes = preflight_bundle(bundle, cancellation)?;
    if fs::symlink_metadata(storage_root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(PublicationError::Symlink(storage_root.to_path_buf()));
    }
    ensure_directory_tree(storage_root)?;
    // Root validation already resolved an absolute path. Avoid canonicalize
    // here: it would reintroduce a symlink-following race after fd traversal.
    let storage_root = storage_root.to_path_buf();
    let final_path = checked_join(&storage_root, &bundle.plan.target_relative_path)?;
    let final_parent = final_path
        .parent()
        .ok_or_else(|| PublicationError::UnsafePath(final_path.clone()))?;
    ensure_directory_tree(final_parent)?;
    ensure_contained(&storage_root, final_parent)?;

    let release_name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| PublicationError::UnsafePath(final_path.clone()))?;
    let lock_path = final_parent.join(format!(".{release_name}.publication.lock"));
    let lock = open_lock_file(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(PublicationError::Busy(bundle.plan.release_id));
        }
        Err(std::fs::TryLockError::Error(source)) => {
            return Err(PublicationError::Io {
                operation: "lock",
                path: lock_path.clone(),
                source,
            });
        }
    }

    let nfs = detect_nfs(&lock)
        .map_err(|source| PublicationError::Io {
            operation: "identify canonical filesystem",
            path: final_parent.to_owned(),
            source,
        })?
        .then(|| nfs::Reservation::new(&final_path, &bundle.plan_hash));
    let asset_io = AssetIo {
        cancellation,
        budget: nfs.as_ref().map(|_| budget),
    };
    let resuming = match &nfs {
        Some(r) => r.resume(&final_path, bundle, &manifest_bytes, &lock, asset_io)?,
        None => false,
    };
    if !resuming && path_exists(&final_path)? {
        let receipt = verify_final_and_receipt(
            bundle,
            &final_path,
            PublicationOutcome::Adopted,
            &manifest_bytes,
            asset_io,
        )?;
        fsync_directory(&final_path)?;
        fsync_directory(final_parent)?;
        for asset in unique_assets(&bundle.plan) {
            observer(PublicationMilestone {
                step: PublicationJournalStep::AssetCopied,
                asset_path: Some(asset.canonical_path.clone()),
                size_bytes: Some(asset.size_bytes),
                byte_hash: Some(asset.byte_hash.clone()),
            });
        }
        observer(PublicationMilestone {
            step: PublicationJournalStep::BundleSealed,
            asset_path: None,
            size_bytes: None,
            byte_hash: None,
        });
        observer(PublicationMilestone {
            step: PublicationJournalStep::FinalAdopted,
            asset_path: Some(bundle.plan.manifest_relative_path.clone()),
            size_bytes: Some(manifest_bytes.len().try_into().unwrap_or(u64::MAX)),
            byte_hash: Some(receipt.sealed_manifest_hash.clone()),
        });
        return Ok(receipt);
    }
    if cancellation.load(Ordering::SeqCst) {
        return Err(PublicationError::Canceled);
    }

    let incoming_path = if let Some(r) = &nfs {
        if !resuming {
            r.reserve(&final_path, final_parent, &lock_path)?;
        }
        r.check_inventory(&final_path, &bundle.plan)?;
        final_path.clone()
    } else {
        let path = final_parent.join(format!(".{release_name}.incoming"));
        ensure_directory(&path)?;
        path
    };
    observer(PublicationMilestone {
        step: PublicationJournalStep::IncomingPrepared,
        asset_path: None,
        size_bytes: None,
        byte_hash: None,
    });
    for asset in unique_assets(&bundle.plan) {
        if cancellation.load(Ordering::SeqCst) {
            return Err(PublicationError::Canceled);
        }
        copy_verified_asset(
            bundle,
            asset,
            &incoming_path,
            nfs.as_ref(),
            asset_io,
            fault,
            #[cfg(feature = "test-support")]
            copy_barrier,
            #[cfg(not(feature = "test-support"))]
            _copy_barrier,
        )?;
        observer(PublicationMilestone {
            step: PublicationJournalStep::AssetCopied,
            asset_path: Some(asset.canonical_path.clone()),
            size_bytes: Some(asset.size_bytes),
            byte_hash: Some(asset.byte_hash.clone()),
        });
    }
    if let Some(r) = &nfs {
        if cancellation.load(Ordering::SeqCst) {
            return Err(PublicationError::Canceled);
        }
        r.check_inventory(&final_path, &bundle.plan)?;
        inject_fault(
            fault,
            PublicationFaultPoint::SealedTreeFsync,
            "fsync NFS asset directories before sealing",
            &final_path,
        )?;
        fsync_tree(&final_path)?;
    }
    write_manifest(&incoming_path, bundle, &manifest_bytes, fault, nfs.as_ref())?;
    if let Some(r) = &nfs {
        inject_fault(
            fault,
            PublicationFaultPoint::NfsManifestInstalled,
            "commit NFS manifest",
            &final_path,
        )?;
        r.finish(&final_path, &lock)?;
    } else {
        verify_sealed_bundle(
            &incoming_path,
            bundle,
            Some(&manifest_bytes),
            None,
            asset_io,
        )?;
        inject_fault(
            fault,
            PublicationFaultPoint::SealedTreeFsync,
            "fsync sealed incoming tree",
            &incoming_path,
        )?;
        fsync_tree(&incoming_path)?;
    }
    let sealed = PublicationMilestone {
        step: PublicationJournalStep::BundleSealed,
        asset_path: None,
        size_bytes: None,
        byte_hash: None,
    };
    // The NFS reservation is already at its final path. Verify it once after
    // syncing the parent, then record both milestones against that verification.
    if nfs.is_none() {
        observer(sealed.clone());
    }
    if cancellation.load(Ordering::SeqCst) {
        return Err(PublicationError::Canceled);
    }

    let install_result = if nfs.is_some() {
        Ok(())
    } else {
        inject_fault(
            fault,
            PublicationFaultPoint::FinalRename,
            "atomically rename incoming bundle without replacement",
            &final_path,
        )?;
        rename_noreplace(&incoming_path, &final_path)
    };
    match install_result {
        Ok(()) => {
            inject_fault(
                fault,
                PublicationFaultPoint::FinalParentFsync,
                "fsync canonical target parent after final rename",
                final_parent,
            )?;
            fsync_directory(final_parent)?;
            let receipt = verify_final_and_receipt(
                bundle,
                &final_path,
                PublicationOutcome::Published,
                &manifest_bytes,
                asset_io,
            )?;
            if nfs.is_some() {
                observer(sealed);
            }
            observer(PublicationMilestone {
                step: PublicationJournalStep::Finalized,
                asset_path: Some(bundle.plan.manifest_relative_path.clone()),
                size_bytes: Some(manifest_bytes.len().try_into().unwrap_or(u64::MAX)),
                byte_hash: Some(receipt.sealed_manifest_hash.clone()),
            });
            Ok(receipt)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let receipt = verify_final_and_receipt(
                bundle,
                &final_path,
                PublicationOutcome::Adopted,
                &manifest_bytes,
                asset_io,
            )?;
            observer(PublicationMilestone {
                step: PublicationJournalStep::FinalAdopted,
                asset_path: Some(bundle.plan.manifest_relative_path.clone()),
                size_bytes: Some(manifest_bytes.len().try_into().unwrap_or(u64::MAX)),
                byte_hash: Some(receipt.sealed_manifest_hash.clone()),
            });
            Ok(receipt)
        }
        Err(source) => Err(PublicationError::Io {
            operation: "atomically rename incoming bundle without replacement",
            path: final_path,
            source,
        }),
    }
}

fn preflight_bundle(
    bundle: &PublicationBundle,
    cancellation: &AtomicBool,
) -> Result<Vec<u8>, PublicationError> {
    bundle.plan.validate()?;
    if hash_publication_plan(&bundle.plan)? != bundle.plan_hash {
        return Err(PublicationError::Invariant(
            PublicationInvariantError::PlanMismatch,
        ));
    }
    let publication = bundle
        .manifest
        .publication
        .as_ref()
        .ok_or(PublicationError::Invariant(
            PublicationInvariantError::PlanMismatch,
        ))?;
    publication.validate_against(&bundle.plan)?;
    if publication.plan_hash != bundle.plan_hash {
        return Err(PublicationError::Invariant(
            PublicationInvariantError::PlanMismatch,
        ));
    }
    let work_root_metadata =
        fs::symlink_metadata(&bundle.work_root).map_err(|source| PublicationError::Io {
            operation: "inspect work root",
            path: bundle.work_root.clone(),
            source,
        })?;
    if work_root_metadata.file_type().is_symlink() {
        return Err(PublicationError::Symlink(bundle.work_root.clone()));
    }
    if !work_root_metadata.is_dir() {
        return Err(PublicationError::UnsafePath(bundle.work_root.clone()));
    }
    for asset in unique_assets(&bundle.plan) {
        if cancellation.load(Ordering::SeqCst) {
            return Err(PublicationError::Canceled);
        }
        let source = checked_join(&bundle.work_root, &asset.normalized_path)?;
        verify_file(
            &source,
            asset.size_bytes,
            &asset.byte_hash,
            AssetIo {
                cancellation,
                budget: None,
            },
        )?;
    }
    serde_json::to_vec_pretty(&bundle.manifest).map_err(PublicationError::from)
}

fn unique_assets(plan: &PublicationPlan) -> Vec<&melodee_domain::CanonicalAsset> {
    let mut assets = BTreeMap::new();
    for asset in &plan.assets {
        assets.entry(asset.canonical_path.as_str()).or_insert(asset);
    }
    assets.into_values().collect()
}

#[allow(clippy::too_many_arguments)]
fn copy_verified_asset(
    bundle: &PublicationBundle,
    asset: &melodee_domain::CanonicalAsset,
    incoming_root: &Path,
    nfs: Option<&nfs::Reservation>,
    asset_io: AssetIo<'_>,
    fault: &mut dyn FnMut(PublicationFaultPoint) -> io::Result<()>,
    #[cfg(feature = "test-support")] copy_barrier: Option<&PublicationCopyTestBarrier>,
    #[cfg(not(feature = "test-support"))] _copy_barrier: Option<&()>,
) -> Result<(), PublicationError> {
    let source = checked_join(&bundle.work_root, &asset.normalized_path)?;
    let destination = checked_join(incoming_root, &asset.canonical_path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| PublicationError::UnsafePath(destination.clone()))?;
    ensure_directory_tree(parent)?;
    ensure_contained(incoming_root, parent)?;
    if path_exists(&destination)? {
        return verify_file(&destination, asset.size_bytes, &asset.byte_hash, asset_io);
    }

    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::PublicationCopy,
    );

    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| PublicationError::UnsafePath(destination.clone()))?;
    let temporary = nfs.map_or_else(
        || parent.join(format!(".{file_name}.copying")),
        |r| r.scratch.clone(),
    );
    reject_symlink_if_present(&temporary)?;
    let mut input = open_regular_file(&source)?;
    let mut output = nfs
        .map_or_else(|| open_write_nofollow(&temporary), |r| r.create_scratch())
        .map_err(|source| PublicationError::Io {
            operation: "create incoming temporary asset",
            path: temporary.clone(),
            source,
        })?;
    if !output
        .metadata()
        .map_err(|source| PublicationError::Io {
            operation: "inspect incoming temporary asset",
            path: temporary.clone(),
            source,
        })?
        .is_file()
    {
        return Err(PublicationError::NotRegularFile(temporary));
    }

    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if asset_io.cancellation.load(Ordering::SeqCst) {
            return Err(PublicationError::Canceled);
        }
        let count = input
            .read(&mut buffer)
            .map_err(|source| PublicationError::Io {
                operation: "read normalized asset",
                path: source_path(&bundle.work_root, &asset.normalized_path),
                source,
            })?;
        if count == 0 {
            break;
        }
        operation.add_read(u64::try_from(count).unwrap_or(u64::MAX));
        asset_io.before_chunk(count)?;
        if let Err(source) = output.write_all(&buffer[..count]) {
            // write_all may have written an unknown prefix before failing.
            operation.mark_bytes_incomplete();
            return Err(PublicationError::Io {
                operation: "copy normalized asset",
                path: temporary.clone(),
                source,
            });
        }
        operation.add_written(u64::try_from(count).unwrap_or(u64::MAX));
        hasher.update(&buffer[..count]);
        size = size.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        #[cfg(feature = "test-support")]
        if let Some(barrier) = copy_barrier {
            barrier.pause_until_canceled(asset_io.cancellation);
            return Err(PublicationError::Canceled);
        }
    }
    inject_fault(
        fault,
        PublicationFaultPoint::AssetFsync,
        "fsync incoming asset",
        &temporary,
    )?;
    output.sync_all().map_err(|source| PublicationError::Io {
        operation: "fsync incoming asset",
        path: temporary.clone(),
        source,
    })?;
    let observed_hash = hasher.finalize().to_hex().to_string();
    if size != asset.size_bytes {
        return Err(PublicationError::SizeMismatch {
            path: source,
            expected: asset.size_bytes,
            actual: size,
        });
    }
    if observed_hash != asset.byte_hash {
        return Err(PublicationError::HashMismatch {
            path: source,
            expected: asset.byte_hash.clone(),
            actual: observed_hash,
        });
    }
    operation.finish();
    drop(output);
    if nfs.is_none() && path_exists(&destination)? {
        return verify_file(&destination, asset.size_bytes, &asset.byte_hash, asset_io);
    }
    match nfs.map_or_else(
        || rename_noreplace(&temporary, &destination),
        |r| r.install_file(&destination),
    ) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return verify_file(&destination, asset.size_bytes, &asset.byte_hash, asset_io);
        }
        Err(source) => {
            return Err(PublicationError::Io {
                operation: "install verified incoming asset without replacement",
                path: destination,
                source,
            });
        }
    }
    fsync_directory(parent)?;
    if nfs.is_some() {
        // The exclusive hard link names the inode we hashed while writing and
        // synced. The final receipt still requires a full canonical readback.
        return Ok(());
    }
    verify_file(&destination, asset.size_bytes, &asset.byte_hash, asset_io)
}

fn write_manifest(
    incoming_root: &Path,
    bundle: &PublicationBundle,
    manifest_bytes: &[u8],
    fault: &mut dyn FnMut(PublicationFaultPoint) -> io::Result<()>,
    nfs: Option<&nfs::Reservation>,
) -> Result<(), PublicationError> {
    let destination = incoming_root.join("manifest.json");
    if path_exists(&destination)? {
        let existing = read_regular_bytes(&destination)?;
        if existing == manifest_bytes {
            return Ok(());
        }
        return Err(PublicationError::TargetConflict {
            path: destination,
            reason: format!(
                "incoming manifest belongs to a different plan than {}",
                bundle.plan_hash
            ),
        });
    }
    let temporary = nfs.map_or_else(
        || incoming_root.join(".manifest.json.copying"),
        |r| r.scratch.clone(),
    );
    reject_symlink_if_present(&temporary)?;
    let mut file = nfs
        .map_or_else(|| open_write_nofollow(&temporary), |r| r.create_scratch())
        .map_err(|source| PublicationError::Io {
            operation: "create incoming manifest",
            path: temporary.clone(),
            source,
        })?;
    file.write_all(manifest_bytes)
        .map_err(|source| PublicationError::Io {
            operation: "write incoming manifest",
            path: temporary.clone(),
            source,
        })?;
    inject_fault(
        fault,
        PublicationFaultPoint::ManifestFsync,
        "fsync incoming manifest",
        &temporary,
    )?;
    file.sync_all().map_err(|source| PublicationError::Io {
        operation: "fsync incoming manifest",
        path: temporary.clone(),
        source,
    })?;
    drop(file);
    match nfs.map_or_else(
        || rename_noreplace(&temporary, &destination),
        |r| r.install_file(&destination),
    ) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = read_regular_bytes(&destination)?;
            if existing != manifest_bytes {
                return Err(PublicationError::TargetConflict {
                    path: destination,
                    reason: "incoming manifest raced with different bytes".to_owned(),
                });
            }
        }
        Err(source) => {
            return Err(PublicationError::Io {
                operation: "install incoming manifest without replacement",
                path: destination,
                source,
            });
        }
    }
    fsync_directory(incoming_root)
}

fn verify_final_and_receipt(
    bundle: &PublicationBundle,
    final_path: &Path,
    outcome: PublicationOutcome,
    manifest_bytes: &[u8],
    asset_io: AssetIo<'_>,
) -> Result<PublicationReceipt, PublicationError> {
    let expected = (outcome == PublicationOutcome::Published).then_some(manifest_bytes);
    let sealed_manifest_hash = verify_sealed_bundle(final_path, bundle, expected, None, asset_io)
        .map_err(|error| {
        if matches!(error, PublicationError::Canceled) {
            return error;
        }
        PublicationError::TargetConflict {
            path: final_path.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    Ok(PublicationReceipt {
        outcome,
        target_relative_path: bundle.plan.target_relative_path.clone(),
        manifest_relative_path: bundle.plan.manifest_relative_path.clone(),
        sealed_manifest_hash,
    })
}

fn verify_sealed_bundle(
    root: &Path,
    bundle: &PublicationBundle,
    expected_manifest_bytes: Option<&[u8]>,
    reservation_marker: Option<&str>,
    asset_io: AssetIo<'_>,
) -> Result<String, PublicationError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| PublicationError::Io {
        operation: "inspect sealed bundle",
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PublicationError::Symlink(root.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(PublicationError::TargetConflict {
            path: root.to_path_buf(),
            reason: "canonical Release target is not a directory".to_owned(),
        });
    }
    let manifest_path = root.join("manifest.json");
    reject_symlink_if_present(&manifest_path)?;
    let manifest_bytes = read_regular_bytes(&manifest_path)?;
    if expected_manifest_bytes.is_some_and(|expected| manifest_bytes != expected) {
        return Err(PublicationError::TargetConflict {
            path: manifest_path,
            reason: "manifest bytes do not match the immutable publication plan".to_owned(),
        });
    }
    let parsed: ReleaseManifest = serde_json::from_slice(&manifest_bytes)?;
    if parsed != bundle.manifest {
        return Err(PublicationError::TargetConflict {
            path: root.to_path_buf(),
            reason: "manifest identity or publication facts differ".to_owned(),
        });
    }

    let mut expected_files = BTreeMap::new();
    if let Some(marker) = reservation_marker {
        expected_files.insert(marker.to_owned(), (0, None));
    }
    expected_files.insert(
        "manifest.json".to_owned(),
        (manifest_bytes.len() as u64, None),
    );
    for asset in unique_assets(&bundle.plan) {
        expected_files.insert(
            asset.canonical_path.clone(),
            (asset.size_bytes, Some(asset.byte_hash.as_str())),
        );
        let path = checked_join(root, &asset.canonical_path)?;
        verify_file(&path, asset.size_bytes, &asset.byte_hash, asset_io)?;
    }
    let observed_files = collect_bundle_files(root)?;
    let expected_names = expected_files.keys().cloned().collect::<Vec<_>>();
    if observed_files != expected_names {
        return Err(PublicationError::TargetConflict {
            path: root.to_path_buf(),
            reason: format!(
                "asset inventory differs; expected {expected_names:?}, observed {observed_files:?}"
            ),
        });
    }
    Ok(blake3::hash(&manifest_bytes).to_hex().to_string())
}

fn collect_bundle_files(root: &Path) -> Result<Vec<String>, PublicationError> {
    fn walk(base: &Path, current: &Path, output: &mut Vec<String>) -> Result<(), PublicationError> {
        let entries = fs::read_dir(current).map_err(|source| PublicationError::Io {
            operation: "enumerate sealed bundle",
            path: current.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| PublicationError::Io {
                operation: "read sealed bundle entry",
                path: current.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| PublicationError::Io {
                operation: "inspect sealed bundle entry",
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(PublicationError::Symlink(path));
            }
            if metadata.is_dir() {
                walk(base, &path, output)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .map_err(|_| PublicationError::UnsafePath(path.clone()))?;
                output.push(path_to_manifest(relative)?);
            } else {
                return Err(PublicationError::NotRegularFile(path));
            }
        }
        Ok(())
    }

    let mut output = Vec::new();
    walk(root, root, &mut output)?;
    output.sort();
    Ok(output)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
    asset_io: AssetIo<'_>,
) -> Result<(), PublicationError> {
    let mut operation = melodee_media::operation_metrics::OperationGuard::start(
        melodee_media::operation_metrics::Operation::PublicationHash,
    );
    let mut file = open_regular_file(path)?;
    let metadata = file.metadata().map_err(|source| PublicationError::Io {
        operation: "inspect publication asset",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() != expected_size {
        return Err(PublicationError::SizeMismatch {
            path: path.to_path_buf(),
            expected: expected_size,
            actual: metadata.len(),
        });
    }
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let remaining = expected_size - size;
        let request = remaining.min(buffer.len() as u64) as usize;
        asset_io.before_chunk(request)?;
        // At the expected length, a one-byte probe still detects a growing file.
        let count =
            file.read(&mut buffer[..request.max(1)])
                .map_err(|source| PublicationError::Io {
                    operation: "hash publication asset",
                    path: path.to_path_buf(),
                    source,
                })?;
        if count == 0 {
            break;
        }
        operation.add_read(u64::try_from(count).unwrap_or(u64::MAX));
        if remaining == 0 {
            return Err(PublicationError::SizeMismatch {
                path: path.to_owned(),
                expected: expected_size,
                actual: size.saturating_add(1),
            });
        }
        hasher.update(&buffer[..count]);
        size = size.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    let actual = hasher.finalize().to_hex().to_string();
    if size != expected_size {
        return Err(PublicationError::SizeMismatch {
            path: path.to_path_buf(),
            expected: expected_size,
            actual: size,
        });
    }
    if actual != expected_hash {
        return Err(PublicationError::HashMismatch {
            path: path.to_path_buf(),
            expected: expected_hash.to_owned(),
            actual,
        });
    }
    operation.finish();
    Ok(())
}

fn open_regular_file(path: &Path) -> Result<File, PublicationError> {
    reject_symlink_components(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|source| PublicationError::Io {
        operation: "inspect publication asset",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PublicationError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(PublicationError::NotRegularFile(path.to_path_buf()));
    }
    let file = open_read_nofollow(path).map_err(|source| PublicationError::Io {
        operation: "open publication asset",
        path: path.to_path_buf(),
        source,
    })?;
    if !file
        .metadata()
        .map_err(|source| PublicationError::Io {
            operation: "inspect opened publication asset",
            path: path.to_path_buf(),
            source,
        })?
        .is_file()
    {
        return Err(PublicationError::NotRegularFile(path.to_path_buf()));
    }
    Ok(file)
}

fn checked_join(root: &Path, relative: &str) -> Result<PathBuf, PublicationError> {
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PublicationError::UnsafePath(relative_path.to_path_buf()));
    }
    let path = root.join(relative_path);
    ensure_contained(root, &path)?;
    Ok(path)
}

fn ensure_contained(root: &Path, path: &Path) -> Result<(), PublicationError> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(PublicationError::UnsafePath(path.to_path_buf()))
    }
}

fn ensure_directory_tree(path: &Path) -> Result<(), PublicationError> {
    if !path.is_absolute() {
        return Err(PublicationError::UnsafePath(path.to_path_buf()));
    }
    crate::secure_fs::create_directory_tree(path).map_err(|source| PublicationError::Io {
        operation: "create symlink-free directory tree",
        path: path.to_path_buf(),
        source,
    })
}

fn ensure_directory(path: &Path) -> Result<(), PublicationError> {
    ensure_directory_tree(path)
}

fn reject_symlink_if_present(path: &Path) -> Result<(), PublicationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(PublicationError::Symlink(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PublicationError::Io {
            operation: "inspect path",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn reject_symlink_components(path: &Path) -> Result<(), PublicationError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PublicationError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(PublicationError::Io {
                    operation: "inspect path component",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn path_exists(path: &Path) -> Result<bool, PublicationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(PublicationError::Symlink(path.to_path_buf()))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PublicationError::Io {
            operation: "inspect path",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn open_lock_file(path: &Path) -> Result<File, PublicationError> {
    reject_symlink_if_present(path)?;
    let file = open_lock_nofollow(path).map_err(|source| PublicationError::Io {
        operation: "open publication lock",
        path: path.to_path_buf(),
        source,
    })?;
    if !file
        .metadata()
        .map_err(|source| PublicationError::Io {
            operation: "inspect publication lock",
            path: path.to_path_buf(),
            source,
        })?
        .is_file()
    {
        return Err(PublicationError::NotRegularFile(path.to_path_buf()));
    }
    Ok(file)
}

fn fsync_tree(root: &Path) -> Result<(), PublicationError> {
    let mut directories = Vec::new();
    fn gather(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), PublicationError> {
        output.push(path.to_path_buf());
        for entry in fs::read_dir(path).map_err(|source| PublicationError::Io {
            operation: "enumerate directory for fsync",
            path: path.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| PublicationError::Io {
                operation: "read directory entry for fsync",
                path: path.to_path_buf(),
                source,
            })?;
            let child = entry.path();
            let metadata = fs::symlink_metadata(&child).map_err(|source| PublicationError::Io {
                operation: "inspect entry for fsync",
                path: child.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(PublicationError::Symlink(child));
            }
            if metadata.is_dir() {
                gather(&child, output)?;
            }
        }
        Ok(())
    }
    gather(root, &mut directories)?;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        fsync_directory(&directory)?;
    }
    Ok(())
}

fn fsync_directory(path: &Path) -> Result<(), PublicationError> {
    let directory =
        crate::secure_fs::open_directory(path).map_err(|source| PublicationError::Io {
            operation: "open directory for fsync",
            path: path.to_path_buf(),
            source,
        })?;
    match directory.sync_all() {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(PublicationError::Io {
            operation: "fsync directory",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn path_to_manifest(path: &Path) -> Result<String, PublicationError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(PublicationError::UnsafePath(path.to_path_buf()));
        };
        let value = value
            .to_str()
            .ok_or_else(|| PublicationError::UnsafePath(path.to_path_buf()))?;
        parts.push(value);
    }
    Ok(parts.join("/"))
}

fn source_path(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}

fn read_regular_bytes(path: &Path) -> Result<Vec<u8>, PublicationError> {
    let mut file = open_regular_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| PublicationError::Io {
            operation: "read regular file",
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
}

fn open_read_nofollow(path: &Path) -> io::Result<File> {
    crate::secure_fs::open_read(path)
}

fn open_write_nofollow(path: &Path) -> io::Result<File> {
    crate::secure_fs::open_write(path)
}

fn open_lock_nofollow(path: &Path) -> io::Result<File> {
    crate::secure_fs::open_lock(path)
}

fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
    crate::secure_fs::rename_noreplace(source, destination)
}

fn inject_fault(
    fault: &mut dyn FnMut(PublicationFaultPoint) -> io::Result<()>,
    point: PublicationFaultPoint,
    operation: &'static str,
    path: &Path,
) -> Result<(), PublicationError> {
    fault(point).map_err(|source| PublicationError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use std::fs;

    use melodee_domain::{
        CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind, NormalizationObservations,
        StructuralValidation,
    };
    use uuid::Uuid;

    use super::*;

    const MANIFEST_V1: &str =
        include_str!("../../melodee-domain/tests/fixtures/phase3_1_manifest_v1.json");
    const ASSET_BYTES: &[u8] = b"deterministic publication fault fixture";

    struct FaultFixture {
        root: PathBuf,
        storage_root: PathBuf,
        bundle: PublicationBundle,
    }

    impl FaultFixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "melodee-publication-fault-{label}-{}",
                Uuid::now_v7().simple()
            ));
            let work_root = root.join("work");
            let storage_root = root.join("storage");
            fs::create_dir_all(work_root.join("audio")).expect("create work tree");
            fs::write(work_root.join("audio/input.mp3"), ASSET_BYTES).expect("write work asset");

            let byte_hash = blake3::hash(ASSET_BYTES).to_hex().to_string();
            let asset = CanonicalAsset {
                kind: CanonicalAssetKind::Media,
                source_path: "input.flac".to_owned(),
                normalized_path: "audio/input.mp3".to_owned(),
                canonical_path: format!("media/{byte_hash}.mp3"),
                size_bytes: u64::try_from(ASSET_BYTES.len()).expect("fixture length"),
                byte_hash,
            };
            let mut manifest: ReleaseManifest =
                serde_json::from_str(MANIFEST_V1).expect("parse v1 fixture");
            manifest.schema_version = CURRENT_MANIFEST_VERSION;
            manifest.manifest_id = Uuid::now_v7();
            manifest.work_item_id = Uuid::now_v7();
            manifest.normalization = Some(NormalizationObservations::default());
            let input_hash = melodee_media::hash_manifest(&manifest).expect("semantic hash");
            let validated = ValidatedNormalization {
                work_root,
                validation: StructuralValidation {
                    profile_name: "phase3.structural-validation".to_owned(),
                    profile_version: 1,
                    outcome: ValidationOutcome::Passed,
                    findings: Vec::new(),
                },
                assets: vec![asset],
            };
            let bundle = build_publication_bundle(
                &manifest,
                &input_hash,
                ReleaseId::new(),
                PublicationAuthorization::phase3_structural(),
                validated,
            )
            .expect("build bundle");
            Self {
                root,
                storage_root,
                bundle,
            }
        }

        fn final_path(&self) -> PathBuf {
            self.storage_root
                .join(&self.bundle.plan.target_relative_path)
        }
    }

    impl Drop for FaultFixture {
        fn drop(&mut self) {
            if self
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("melodee-publication-fault-"))
            {
                let _ignored = fs::remove_dir_all(&self.root);
            }
        }
    }

    fn assert_fault_then_retry(point: PublicationFaultPoint) {
        let fixture = FaultFixture::new(match point {
            PublicationFaultPoint::AssetFsync => "asset-fsync",
            PublicationFaultPoint::ManifestFsync => "manifest-fsync",
            PublicationFaultPoint::SealedTreeFsync => "tree-fsync",
            PublicationFaultPoint::FinalRename => "rename",
            PublicationFaultPoint::FinalParentFsync => "final-parent-fsync",
            PublicationFaultPoint::NfsManifestInstalled => "manifest-installed",
        });
        let mut injected = false;
        let result = publish_canonical_bundle_impl(
            &fixture.storage_root,
            &fixture.bundle,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |observed| {
                if observed == point && !injected {
                    injected = true;
                    Err(io::Error::other("injected publication fault"))
                } else {
                    Ok(())
                }
            },
            |_| Ok(false),
            &NfsIoBudget::default(),
            None,
        );
        assert!(injected, "fault point was not reached");
        assert!(matches!(result, Err(PublicationError::Io { .. })));
        assert!(
            fs::symlink_metadata(fixture.final_path())
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound),
            "a failed durability step must never expose a partial final"
        );

        let receipt = publish_canonical_bundle(
            &fixture.storage_root,
            &fixture.bundle,
            &AtomicBool::new(false),
        )
        .expect("retry converges");
        assert_eq!(receipt.outcome, PublicationOutcome::Published);
        assert!(fixture.final_path().join("manifest.json").is_file());
    }

    #[test]
    fn injected_fsync_failures_leave_no_final_and_retry_converges() {
        for point in [
            PublicationFaultPoint::AssetFsync,
            PublicationFaultPoint::ManifestFsync,
            PublicationFaultPoint::SealedTreeFsync,
        ] {
            assert_fault_then_retry(point);
        }
    }

    #[test]
    fn injected_final_rename_failure_leaves_no_final_and_retry_converges() {
        assert_fault_then_retry(PublicationFaultPoint::FinalRename);
    }

    #[test]
    fn injected_post_rename_parent_fsync_failure_is_adopted_on_retry() {
        let fixture = FaultFixture::new("final-parent-fsync");
        let mut injected = false;
        let result = publish_canonical_bundle_impl(
            &fixture.storage_root,
            &fixture.bundle,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |observed| {
                if observed == PublicationFaultPoint::FinalParentFsync && !injected {
                    injected = true;
                    Err(io::Error::other("injected final-parent fsync fault"))
                } else {
                    Ok(())
                }
            },
            |_| Ok(false),
            &NfsIoBudget::default(),
            None,
        );
        assert!(injected, "fault point was not reached");
        assert!(matches!(result, Err(PublicationError::Io { .. })));
        assert!(
            fixture.final_path().join("manifest.json").is_file(),
            "the no-replace rename completed before parent durability failed"
        );

        let receipt = publish_canonical_bundle(
            &fixture.storage_root,
            &fixture.bundle,
            &AtomicBool::new(false),
        )
        .expect("retry adopts verified final");
        assert_eq!(receipt.outcome, PublicationOutcome::Adopted);
    }
    fn publish_nfs(
        f: &FaultFixture,
        cancel: &AtomicBool,
        observer: &mut dyn FnMut(PublicationMilestone),
        fault: &mut dyn FnMut(PublicationFaultPoint) -> io::Result<()>,
    ) -> Result<PublicationReceipt, PublicationError> {
        publish_canonical_bundle_impl(
            &f.storage_root,
            &f.bundle,
            cancel,
            observer,
            fault,
            |_| Ok(true),
            &NfsIoBudget::default(),
            None,
        )
    }

    #[test]
    fn fresh_nfs_charges_one_write_one_read_and_adoption_rehashes() {
        let f = FaultFixture::new("nfs-budget");
        let budget = NfsIoBudget::default();
        let publish = || {
            publish_canonical_bundle_impl(
                &f.storage_root,
                &f.bundle,
                &AtomicBool::new(false),
                &mut |_| {},
                &mut |_| Ok(()),
                |_| Ok(true),
                &budget,
                None,
            )
        };
        assert_eq!(publish().unwrap().outcome, PublicationOutcome::Published);
        let size = ASSET_BYTES.len() as u64;
        assert_eq!(budget.charged_bytes.load(Ordering::Relaxed), 2 * size);
        assert_eq!(publish().unwrap().outcome, PublicationOutcome::Adopted);
        assert_eq!(budget.charged_bytes.load(Ordering::Relaxed), 3 * size);
    }

    #[test]
    fn nfs_refuses_a_receipt_if_an_installed_asset_changes_after_sealing() {
        let f = FaultFixture::new("nfs-seal-damage");
        let mut milestones = Vec::new();
        let result = publish_nfs(
            &f,
            &AtomicBool::new(false),
            &mut |m| milestones.push(m.step),
            &mut |point| {
                if point == PublicationFaultPoint::NfsManifestInstalled {
                    fs::write(
                        f.final_path().join(&f.bundle.plan.assets[0].canonical_path),
                        vec![0; ASSET_BYTES.len()],
                    )?;
                }
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(PublicationError::TargetConflict { .. })
        ));
        assert!(!milestones.contains(&PublicationJournalStep::BundleSealed));
        assert!(!milestones.contains(&PublicationJournalStep::Finalized));
    }

    #[test]
    fn nfs_partial_is_exclusive_and_retry_publishes_manifest_last() {
        let f = FaultFixture::new("nfs-resume");
        let cancel = AtomicBool::new(false);
        let target = f.final_path();
        let result = publish_nfs(
            &f,
            &cancel,
            &mut |event| {
                if event.step == PublicationJournalStep::AssetCopied {
                    assert!(!target.join("manifest.json").exists());
                    assert!(matches!(
                        publish_nfs(&f, &AtomicBool::new(false), &mut |_| {}, &mut |_| Ok(())),
                        Err(PublicationError::Busy(_))
                    ));
                    cancel.store(true, Ordering::SeqCst);
                }
            },
            &mut |_| Ok(()),
        );
        assert!(matches!(result, Err(PublicationError::Canceled)));
        assert!(
            target
                .join(&f.bundle.plan.assets[0].canonical_path)
                .is_file()
        );
        cancel.store(false, Ordering::SeqCst);
        let result = publish_nfs(&f, &cancel, &mut |_| {}, &mut |_| Ok(())).unwrap();
        assert_eq!(result.outcome, PublicationOutcome::Published);
        assert_eq!(
            publish_nfs(&f, &cancel, &mut |_| {}, &mut |_| Ok(()))
                .unwrap()
                .outcome,
            PublicationOutcome::Adopted
        );
        assert_eq!(collect_bundle_files(&target).unwrap().len(), 2);
        assert_eq!(
            fs::read(f.bundle.work_root.join("audio/input.mp3")).unwrap(),
            ASSET_BYTES
        );
    }

    #[test]
    fn nfs_refuses_empty_foreign_directory_and_forged_marker() {
        let f = FaultFixture::new("nfs-conflict");
        let target = f.final_path();
        fs::create_dir_all(&target).unwrap();
        for forged in [false, true] {
            if forged {
                fs::write(
                    target.join(format!(".melodee-publishing-{}", f.bundle.plan_hash)),
                    b"foreign",
                )
                .unwrap();
            }
            let before = collect_bundle_files(&target).unwrap();
            assert!(matches!(
                publish_nfs(&f, &AtomicBool::new(false), &mut |_| {}, &mut |_| Ok(())),
                Err(PublicationError::TargetConflict { .. })
            ));
            assert_eq!(before, collect_bundle_files(&target).unwrap());
            if forged {
                fs::remove_file(target.join(format!(".melodee-publishing-{}", f.bundle.plan_hash)))
                    .unwrap();
            }
        }
    }

    #[test]
    fn nfs_retry_after_manifest_install_adopts_or_rejects_damage() {
        for damaged in [false, true] {
            let f = FaultFixture::new("nfs-commit");
            let cancel = AtomicBool::new(false);
            let failed = publish_nfs(&f, &cancel, &mut |_| {}, &mut |point| {
                if point == PublicationFaultPoint::NfsManifestInstalled {
                    Err(io::Error::other("simulated process loss"))
                } else {
                    Ok(())
                }
            });
            assert!(matches!(failed, Err(PublicationError::Io { .. })));
            let target = f.final_path();
            assert!(target.join("manifest.json").exists());
            let asset = target.join(&f.bundle.plan.assets[0].canonical_path);
            if damaged {
                fs::remove_file(&asset).unwrap();
            }
            let retry = publish_nfs(&f, &cancel, &mut |_| {}, &mut |_| Ok(()));
            if damaged {
                assert!(retry.is_err());
                assert!(!asset.exists());
            } else {
                assert_eq!(retry.unwrap().outcome, PublicationOutcome::Adopted);
            }
        }
    }

    #[test]
    fn nfs_retry_preserves_conflicting_bytes_and_unexpected_files() {
        let f = FaultFixture::new("nfs-partial-conflict");
        let target = f.final_path();
        let cancel = AtomicBool::new(false);
        let _result = publish_nfs(
            &f,
            &cancel,
            &mut |m| {
                if m.step == PublicationJournalStep::AssetCopied {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
            &mut |_| Ok(()),
        );
        cancel.store(false, Ordering::SeqCst);
        let asset = target.join(&f.bundle.plan.assets[0].canonical_path);
        fs::write(&asset, b"preserve").unwrap();
        fs::write(target.join("foreign"), b"also preserve").unwrap();
        assert!(matches!(
            publish_nfs(&f, &cancel, &mut |_| {}, &mut |_| Ok(())),
            Err(PublicationError::TargetConflict { .. })
        ));
        fs::remove_file(target.join("foreign")).unwrap();
        assert!(publish_nfs(&f, &cancel, &mut |_| {}, &mut |_| Ok(())).is_err());
        assert_eq!(fs::read(asset).unwrap(), b"preserve");
        assert!(!target.join("manifest.json").exists());
    }
    #[test]
    fn nfs_public_entrypoint_installs_and_adopts_verified_bundle() {
        let f = FaultFixture::new("filesystem-selected");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            publish_canonical_bundle(&f.storage_root, &f.bundle, &cancel)
                .unwrap()
                .outcome,
            PublicationOutcome::Published
        );
        assert_eq!(
            publish_canonical_bundle(&f.storage_root, &f.bundle, &cancel)
                .unwrap()
                .outcome,
            PublicationOutcome::Adopted
        );
    }
}
