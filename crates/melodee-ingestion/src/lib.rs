//! Ingestion discovery and settling services.
//!
//! A filesystem candidate is turned into a durable `ReleaseWorkItem` and promoted
//! through `discovered -> settling -> ready` when source evidence becomes stable.

#![forbid(unsafe_code)]

mod config;
mod error;
mod gate;
mod inspect;
mod normalize;
mod publication;
mod publication_worker;
mod reconcile;
mod scheduler;
mod secure_fs;
mod snapshot;
mod source;
mod validate;

pub use config::{
    DEFAULT_READY_MARKER, ENV_FFMPEG_PATH, ENV_FFMPEG_TIMEOUT_SECONDS, ENV_FFPROBE_PATH,
    ENV_FFPROBE_TIMEOUT_SECONDS, ENV_FPCALC_PATH, ENV_FPCALC_TIMEOUT_SECONDS, ENV_INGEST_ENABLED,
    ENV_INSPECTION_CONCURRENCY, ENV_NORMALIZATION_CONCURRENCY, ENV_PUBLICATION_CONCURRENCY,
    ENV_READY_MARKER_NAME, ENV_READY_MARKER_SHORTCUT_SECONDS, ENV_RECONCILIATION_INTERVAL_SECONDS,
    ENV_SETTLING_QUIET_SECONDS, ENV_SOURCE_LIBRARY_ID, ENV_SOURCE_LIBRARY_NAME, ENV_SOURCE_ROOT,
    ENV_SOURCES_JSON, ENV_STORAGE_ROOT, ENV_VALIDATION_CONCURRENCY, ENV_WORK_AREA_BASE,
    IngestionConfig, IngestionSourceParseError, PublicationRoots,
};

pub use error::{IngestionError, Result};
pub use gate::{DirectoryGateFuture, DirectoryGateOutcome, DirectoryProcessingGate};
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
pub use reconcile::{
    ReconcileResult, ReconcileStats, Reconciler, reconcile_all_sources, reconcile_inbound_source,
};
pub use scheduler::{SchedulerHandle, StopToken, start_reconciliation_loop};
pub use snapshot::{
    CandidateSnapshot, SnapshotCaptureError, SnapshotFile, snapshot_release_directory,
};
pub use source::{
    DiscoveredCandidate, InboundDiscoverySnapshot, SourceDiscoveryError, candidate_relative_path,
    discover_release_candidates,
};
pub use validate::{
    STRUCTURAL_VALIDATION_PROFILE, STRUCTURAL_VALIDATION_PROFILE_VERSION,
    StructuralValidationError, ValidatedNormalization, validate_normalized_manifest,
};

pub use crate::config::InboundSource;
