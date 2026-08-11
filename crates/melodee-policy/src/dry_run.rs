//! Bounded, pure catalog policy dry-run support (PRD-POLICY-008).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use melodee_domain::{Disposition, ReleaseId};
use thiserror::Error;
use time::OffsetDateTime;

use crate::PolicyPassStats;
use crate::engine::{PolicyEngine, PolicyEngineError, PolicyEvaluation, PolicyInput};

pub use melodee_domain::MAX_POLICY_TEST_SAMPLES;
/// Total nested classification facts accepted across one dry-run request.
pub const MAX_DRY_RUN_CLASSIFICATION_FACTS: usize = 10_000;
/// Total per-Track novelty facts accepted across one dry-run request.
pub const MAX_DRY_RUN_TRACK_FACTS: usize = 10_000;

/// Aggregate dry-run output. It contains proposed outcomes only and has no
/// persistence handle, so evaluating it cannot mutate catalog state.
#[derive(Debug, Clone)]
pub struct PolicyDryRun {
    pub results: Vec<PolicyDryRunResult>,
    pub stats: PolicyPassStats,
    pub matched_rule_counts: BTreeMap<String, u32>,
    pub existing_disposition_counts: BTreeMap<String, u32>,
    pub proposed_disposition_counts: BTreeMap<String, u32>,
    pub destructive_impact_count: u32,
    pub sample_release_ids: Vec<ReleaseId>,
}

#[derive(Debug, Clone)]
pub struct PolicyDryRunResult {
    pub release_id: ReleaseId,
    pub evaluation: Box<PolicyEvaluation>,
}

#[derive(Debug, Error)]
pub enum PolicyDryRunError {
    #[error("dry-run input exceeds the configured {max} Release limit")]
    TooManyReleases { max: usize },
    #[error("Release {0} appears more than once in dry-run input")]
    DuplicateRelease(ReleaseId),
    #[error("dry-run nested classification facts exceed the {max} item limit")]
    TooManyClassificationFacts { max: usize },
    #[error("dry-run nested Track facts exceed the {max} item limit")]
    TooManyTrackFacts { max: usize },
    #[error("policy evaluation failed for Release {release_id}")]
    Evaluation {
        release_id: ReleaseId,
        #[source]
        source: PolicyEngineError,
    },
    #[error("dry-run aggregate counter overflowed")]
    CounterOverflow,
}

#[derive(Debug, Clone)]
pub struct PolicyDryRunner {
    engine: Arc<PolicyEngine>,
}

impl PolicyDryRunner {
    #[must_use]
    pub fn new(engine: PolicyEngine) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    /// Evaluates at most the configured batch bound. Results are deterministic
    /// for the supplied facts/rule set; errors are typed and never caught as
    /// panics.
    pub fn run_dry_run(
        &self,
        inputs: Vec<PolicyInput>,
        _now: OffsetDateTime,
    ) -> Result<PolicyDryRun, PolicyDryRunError> {
        if inputs.len() > self.engine.batch_size() {
            return Err(PolicyDryRunError::TooManyReleases {
                max: self.engine.batch_size(),
            });
        }
        validate_nested_scope(&inputs)?;

        let started = std::time::Instant::now();
        let mut release_ids = BTreeSet::new();
        let mut results = Vec::with_capacity(inputs.len());
        let mut matched_rule_counts = BTreeMap::new();
        let mut existing_disposition_counts = BTreeMap::new();
        let mut proposed_disposition_counts = BTreeMap::new();
        let mut destructive_impact_count = 0_u32;
        let mut sample_release_ids = Vec::new();

        for input in inputs {
            let release_id = input.classification.release_id;
            if !release_ids.insert(release_id) {
                return Err(PolicyDryRunError::DuplicateRelease(release_id));
            }
            let evaluation = self
                .engine
                .evaluate(&input)
                .map_err(|source| PolicyDryRunError::Evaluation { release_id, source })?;

            for rule in &evaluation.matched_rules {
                increment(&mut matched_rule_counts, rule.rule_id.clone())?;
            }
            increment(
                &mut existing_disposition_counts,
                evaluation
                    .existing_disposition
                    .map_or_else(|| "none".to_string(), |value| value.as_str().to_string()),
            )?;
            increment(
                &mut proposed_disposition_counts,
                evaluation.decision.disposition.as_str().to_string(),
            )?;
            if evaluation.decision.disposition == Disposition::Delete {
                destructive_impact_count = destructive_impact_count
                    .checked_add(1)
                    .ok_or(PolicyDryRunError::CounterOverflow)?;
            }
            if sample_release_ids.len() < MAX_POLICY_TEST_SAMPLES {
                sample_release_ids.push(release_id);
            }
            results.push(PolicyDryRunResult {
                release_id,
                evaluation: Box::new(evaluation),
            });
        }

        Ok(PolicyDryRun {
            stats: PolicyPassStats {
                claimed: 0,
                evaluated: results.len(),
                decisions: results.len(),
                errors: 0,
                duration_ms: started.elapsed().as_millis(),
            },
            results,
            matched_rule_counts,
            existing_disposition_counts,
            proposed_disposition_counts,
            destructive_impact_count,
            sample_release_ids,
        })
    }
}

fn validate_nested_scope(inputs: &[PolicyInput]) -> Result<(), PolicyDryRunError> {
    let mut classification_facts = 0_usize;
    let mut track_facts = 0_usize;
    for input in inputs {
        classification_facts = classification_facts
            .checked_add(input.classification.traits.len())
            .and_then(|count| count.checked_add(input.classification.evidence.len()))
            .ok_or(PolicyDryRunError::CounterOverflow)?;
        if classification_facts > MAX_DRY_RUN_CLASSIFICATION_FACTS {
            return Err(PolicyDryRunError::TooManyClassificationFacts {
                max: MAX_DRY_RUN_CLASSIFICATION_FACTS,
            });
        }
        track_facts = track_facts
            .checked_add(input.novelty.track_novelty().len())
            .ok_or(PolicyDryRunError::CounterOverflow)?;
        if track_facts > MAX_DRY_RUN_TRACK_FACTS {
            return Err(PolicyDryRunError::TooManyTrackFacts {
                max: MAX_DRY_RUN_TRACK_FACTS,
            });
        }
    }
    Ok(())
}

fn increment(map: &mut BTreeMap<String, u32>, key: String) -> Result<(), PolicyDryRunError> {
    let current = map.entry(key).or_default();
    *current = current
        .checked_add(1)
        .ok_or(PolicyDryRunError::CounterOverflow)?;
    Ok(())
}
