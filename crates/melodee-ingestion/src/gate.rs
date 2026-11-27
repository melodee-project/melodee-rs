//! Directory processing gate (PRD-SCRIPT-001/004, ADR-0135).
//!
//! The injection point for the `directoryProcessingStart` scripting hook:
//! the server installs a [`DirectoryProcessingGate`] that evaluates
//! operator-installed scripts; the ingestion crate owns only the boundary
//! type so it stays free of the scripting runtime. The boundary is
//! deliberately infallible — the gate's outcome type has no error variant —
//! because fail-degraded semantics are contractual (ADR-0135 §4): any script
//! or store failure inside the gate resolves to [`DirectoryGateOutcome::Allow`]
//! and is recorded in the `script_runs` audit by the implementing service.
//!
//! Deny resolves to `Skip`: the candidate is returned to settling unprocessed
//! this pass and re-evaluated on a later one, matching the legacy
//! `onDeny: skip` behavior. Physical quarantine moves are deferred (they
//! require the durable decision/audit/authorization contract); the
//! legacy `delete` action stays rewritten to `skip` (evidence:
//! legacy C# behavior audit).

use std::{future::Future, path::Path, pin::Pin, sync::Arc};

use crate::snapshot::CandidateSnapshot;

/// Boxed future used by the object-safe gate boundary.
pub type DirectoryGateFuture<'a> = Pin<Box<dyn Future<Output = DirectoryGateOutcome> + Send + 'a>>;

/// Opaque lifetime retention for protected stage access. Cloning it extends
/// the hold but grants no filesystem mutation or locking authority.
#[derive(Clone)]
pub struct StageLifetimeHold(Arc<dyn Send + Sync>);

impl std::fmt::Debug for StageLifetimeHold {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StageLifetimeHold")
            .field("holders", &Arc::strong_count(&self.0))
            .finish_non_exhaustive()
    }
}

impl StageLifetimeHold {
    pub(crate) fn from_arc(hold: Arc<dyn Send + Sync>) -> Self {
        Self(hold)
    }
}

/// Construct a lifetime-only hold for integration tests; this grants no stage
/// operation authority and must not be used as protected receipt evidence.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn test_stage_lifetime_hold(hold: Arc<dyn Send + Sync>) -> StageLifetimeHold {
    StageLifetimeHold(hold)
}

/// The gate's decision for one stable candidate directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryGateOutcome {
    /// Processing proceeds (also the fail-degraded default).
    Allow,
    /// The candidate is left unprocessed this pass (legacy `onDeny: skip`).
    Skip {
        /// Script-provided deny message, or a fixed label when absent.
        reason: String,
    },
}

/// Host-injected decision point evaluated once per claimed, still-stable
/// candidate directory immediately before media inspection (the Rust
/// pipeline point equivalent to the legacy pre-plugin-processing gate).
pub trait DirectoryProcessingGate: Send + Sync {
    fn evaluate<'a>(
        &'a self,
        candidate_root: &'a Path,
        snapshot: &'a CandidateSnapshot,
    ) -> DirectoryGateFuture<'a>;

    /// Guard-aware implementations must retain the hold until every worker
    /// and subprocess that can access stage data has actually terminated.
    /// Existing implementations fail closed without reading the path.
    fn evaluate_guarded<'a>(
        &'a self,
        _candidate_root: &'a Path,
        _snapshot: &'a CandidateSnapshot,
        _hold: StageLifetimeHold,
    ) -> DirectoryGateFuture<'a> {
        Box::pin(async {
            DirectoryGateOutcome::Skip {
                reason: "protected stage is unavailable to this script hook".to_owned(),
            }
        })
    }
}
