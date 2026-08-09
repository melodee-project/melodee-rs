//! Ingestion discovery and settling services.
//!
//! A filesystem candidate is turned into a durable `ReleaseWorkItem` and promoted
//! through `discovered -> settling -> ready` when source evidence becomes stable.

#![forbid(unsafe_code)]

mod config;
mod error;
mod reconcile;
mod scheduler;
mod snapshot;
mod source;

pub use config::{
    DEFAULT_READY_MARKER, ENV_INGEST_ENABLED, ENV_READY_MARKER_NAME,
    ENV_READY_MARKER_SHORTCUT_SECONDS, ENV_RECONCILIATION_INTERVAL_SECONDS,
    ENV_SETTLING_QUIET_SECONDS, ENV_SOURCE_LIBRARY_ID, ENV_SOURCE_LIBRARY_NAME, ENV_SOURCE_ROOT,
    ENV_SOURCES_JSON, IngestionConfig, IngestionSourceParseError,
};

pub use error::{IngestionError, Result};
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

pub use crate::config::InboundSource;
