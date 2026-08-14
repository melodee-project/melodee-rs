//! Directory processing gate (Phase 9 WP-11, PRD-SCRIPT-001/004, ADR-0135).
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
//! require the durable Phase 5 decision/audit/authorization contract); the
//! legacy `delete` action stays rewritten to `skip` (evidence:
//! design/evidence/phase-9/script-events-legacy-inventory.md §1a).

use std::{future::Future, path::Path, pin::Pin};

use crate::snapshot::CandidateSnapshot;

/// Boxed future used by the object-safe gate boundary.
pub type DirectoryGateFuture<'a> = Pin<Box<dyn Future<Output = DirectoryGateOutcome> + Send + 'a>>;

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
}
