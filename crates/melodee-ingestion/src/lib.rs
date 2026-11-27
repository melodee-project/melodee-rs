//! Ingestion discovery and settling services.
//!
//! A filesystem candidate is turned into a durable `ReleaseWorkItem` and promoted
//! through `discovered -> settling -> ready` when source evidence becomes stable.

#![forbid(unsafe_code)]

#[cfg(all(test, feature = "test-support", target_os = "linux"))]
#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[cfg(all(test, feature = "test-support", target_os = "linux"))]
#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

mod acquisition;
mod admission;
mod config;
mod custody;
mod deletion;
mod discovery_scan;
mod error;
mod gate;
mod inspect;
mod legacy_custody_verify;
mod normalize;
mod publication;
mod publication_worker;
mod reconcile;
#[cfg(test)]
mod retained_merkle;
mod scheduler;
mod secure_fs;
mod snapshot;
mod source;
#[cfg(all(target_os = "linux", test))]
mod stage_verity;
mod validate;

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use admission::saturate_for_test as saturate_ingestion_admission_for_test;

#[doc(hidden)]
pub use admission::acquire_ingestion_admission;

pub use acquisition::{
    AcquisitionError, LegacyPathReview, OwnedSource, acquire_owned_source, owned_source_root,
    resolve_legacy_inbound_path, source_generation, verify_owned_source,
};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use acquisition::{AcquisitionFaultPoint, AcquisitionTestHook, AcquisitionWriteFault};
pub use config::{
    DEFAULT_READY_MARKER, ENV_FFMPEG_PATH, ENV_FFMPEG_TIMEOUT_SECONDS, ENV_FFPROBE_PATH,
    ENV_FFPROBE_TIMEOUT_SECONDS, ENV_FPCALC_PATH, ENV_FPCALC_TIMEOUT_SECONDS, ENV_INGEST_ENABLED,
    ENV_INSPECTION_CONCURRENCY, ENV_NORMALIZATION_CONCURRENCY, ENV_PROTECTED_CUSTODY_ROOT,
    ENV_PROTECTED_PRODUCER_UID, ENV_PUBLICATION_CONCURRENCY, ENV_PUBLICATION_NFS_MIB_PER_SECOND,
    ENV_READY_MARKER_NAME, ENV_READY_MARKER_SHORTCUT_SECONDS, ENV_RECONCILIATION_INTERVAL_SECONDS,
    ENV_SETTLING_QUIET_SECONDS, ENV_SOURCE_LIBRARY_ID, ENV_SOURCE_LIBRARY_NAME, ENV_SOURCE_ROOT,
    ENV_SOURCES_JSON, ENV_STAGING_ROOT, ENV_STORAGE_ROOT, ENV_VALIDATION_CONCURRENCY,
    ENV_WORK_AREA_BASE, IngestionConfig, IngestionSourceParseError, ProtectedCustodyConfig,
    PublicationRoots, StorageRoots,
};
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub use custody::{LegacyLifecycleProvisionOutcome, provision_legacy_lifecycle_once};
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android")
))]
#[doc(hidden)]
pub use custody::{
    test_bind_retained_receipt, test_capture_provisional_staging_digest,
    test_verify_staging_baseline,
};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use custody::{
    test_capture_provisional_custody_digest, test_custody_lease_pending, test_custody_lease_wait,
    test_custody_producers_quiescent,
};
pub use deletion::{
    DeletedReleasePaths, ReleaseDeletionError, delete_purged_inbound_occurrence,
    delete_release_storage, delete_release_storage_with_holds, delete_storage_files,
    delete_storage_files_with_holds, delete_unpublished_storage,
    delete_unpublished_storage_with_holds,
};

pub use error::{IngestionError, Result};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use gate::test_stage_lifetime_hold;
pub use gate::{
    DirectoryGateFuture, DirectoryGateOutcome, DirectoryProcessingGate, StageLifetimeHold,
};
pub use inspect::{InspectionStats, inspect_ready_once, start_inspection_loop};
pub use normalize::{NormalizationStats, normalize_once, start_normalization_loop};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use publication::PublicationCopyTestBarrier;
pub use publication::{
    PublicationBundle, PublicationError, PublicationMilestone, PublicationOutcome,
    PublicationReceipt, build_publication_bundle, hash_publication_plan, publish_canonical_bundle,
};
pub use publication_worker::{
    PublicationStats, ValidationStats, publish_once, start_publication_loop, start_validation_loop,
    validate_once,
};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use reconcile::SnapshotStallTestHook;
pub use reconcile::{
    ReconcileResult, ReconcileStats, Reconciler, reconcile_all_sources, reconcile_inbound_source,
};
pub use scheduler::{SchedulerHandle, StopToken, start_reconciliation_loop};
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "android", target_vendor = "apple")
))]
#[doc(hidden)]
pub use scheduler::{
    test_retained_recheck_turn, test_retained_recheck_turn_with_mutation,
    test_retained_recheck_turn_with_root_replacement,
    test_retained_recheck_turn_without_directory_support, test_start_retained_scheduler,
};
pub use snapshot::{
    CandidateSnapshot, SnapshotCaptureError, SnapshotFile, snapshot_release_directory,
};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use source::DiscoveryResolveTestHook;
pub use source::{
    DiscoveredCandidate, InboundDiscoverySnapshot, SourceDiscoveryError, candidate_relative_path,
    discover_release_candidates,
};
pub use validate::{
    STRUCTURAL_VALIDATION_PROFILE, STRUCTURAL_VALIDATION_PROFILE_VERSION,
    StructuralValidationError, ValidatedNormalization, validate_normalized_manifest,
};

pub use crate::config::InboundSource;
