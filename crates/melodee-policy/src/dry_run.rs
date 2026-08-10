//! Catalog dry-run: evaluates Releases against the policy ruleset without
//! performing destructive actions, producing aggregate reports for operators.
//! Supports the catalog-policy-dry-run capability (PRD-POLICY-006).

use std::sync::Arc;
use time::OffsetDateTime;

use crate::PolicyPassStats;
use crate::engine::{PolicyEngine, PolicyEvaluation, PolicyInput};

/// Result of a dry-run pass over a batch of Releases.
#[derive(Debug, Clone)]
pub struct PolicyDryRun {
    pub results: Vec<PolicyDryRunResult>,
    pub stats: PolicyPassStats,
}

/// A single dry-run result: either a decision or an error.
#[derive(Debug, Clone)]
pub enum PolicyDryRunResult {
    Decision {
        release_id: melodee_domain::ReleaseId,
        evaluation: Box<PolicyEvaluation>,
    },
    Error {
        release_id: melodee_domain::ReleaseId,
        error: String,
    },
}

/// Catalog dry-run engine: evaluates releases against policy rules without
/// performing destructive actions.
#[derive(Debug, Clone)]
pub struct PolicyDryRunner {
    engine: Arc<PolicyEngine>,
}

impl PolicyDryRunner {
    pub fn new(engine: PolicyEngine) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    /// Evaluates a batch of releases in dry-run mode, returning all decisions
    /// and aggregate statistics. No releases are modified, deleted, or
    /// flagged for destructive action.
    pub fn run_dry_run(&self, inputs: Vec<PolicyInput>, _now: OffsetDateTime) -> PolicyDryRun {
        let start = std::time::Instant::now();
        let mut results = Vec::with_capacity(inputs.len());
        let mut stats = PolicyPassStats::default();

        for input in inputs {
            stats.evaluated += 1;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.engine.evaluate(&input)
            })) {
                Ok(evaluation) => {
                    stats.decisions += 1;
                    results.push(PolicyDryRunResult::Decision {
                        release_id: input.release_id,
                        evaluation: Box::new(evaluation),
                    });
                }
                Err(_) => {
                    stats.errors += 1;
                    results.push(PolicyDryRunResult::Error {
                        release_id: input.release_id,
                        error: "Evaluation panicked".to_string(),
                    });
                }
            }
        }

        stats.duration_ms = start.elapsed().as_millis();
        PolicyDryRun { results, stats }
    }
}
