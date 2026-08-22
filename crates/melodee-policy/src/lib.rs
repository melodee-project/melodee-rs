//! policy engine: Release classification, Track novelty analysis,
//! versioned policy evaluation, and dry-run/test-against-catalog support.
//!
//! This crate owns the logic that decides what Melodee believes a Release is
//! (classification) and what Melodee wants to do with it (policy disposition).
//! It depends on `melodee-domain` for pure vocabulary and `melodee-persistence`
//! for durable catalog read/write. It does not own filesystem deletion.

#![forbid(unsafe_code)]

mod catalog_context;
mod classifier;
mod config;
mod dry_run;
mod engine;
mod novelty;

pub use catalog_context::evaluate_catalog_context;
pub use classifier::{
    AdministratorClassification, ClassificationError, ClassificationInput, Classifier,
    DEFAULT_CLASSIFIER_VERSION, DEFAULT_DESTRUCTIVE_CONFIDENCE,
};
pub use config::{
    ENV_POLICY_BATCH_SIZE, ENV_POLICY_CONCURRENCY, ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS,
    ENV_POLICY_NOVEL_GREATEST_HITS_DISPOSITION, ENV_POLICY_REDUNDANT_GREATEST_HITS_DISPOSITION,
    ENV_POLICY_RULE_SET_VERSION, ENV_POLICY_UNCERTAIN_GREATEST_HITS_DISPOSITION,
    MAX_POLICY_BATCH_SIZE, MAX_POLICY_CONCURRENCY, PolicyConfig, PolicyConfigError,
};
pub use dry_run::{
    MAX_DRY_RUN_CLASSIFICATION_FACTS, MAX_DRY_RUN_TRACK_FACTS, MAX_POLICY_TEST_SAMPLES,
    PolicyDryRun, PolicyDryRunError, PolicyDryRunResult, PolicyDryRunner,
};
pub use engine::{
    DEFAULT_RULE_SET_NAME, DEFAULT_RULE_SET_VERSION, PolicyEngine, PolicyEngineError,
    PolicyEvaluation, PolicyInput, RuleMatch, default_rule_set, default_rule_set_id, default_rules,
};
pub use novelty::{
    DEFAULT_NOVELTY_EVALUATOR_VERSION, MAX_TRACKS_PER_NOVELTY_EVALUATION, NoveltyAssessment,
    NoveltyCategory, NoveltyEngine, NoveltyEngineError, NoveltyInput, NoveltyTrackFact,
};

/// Outcome of one bounded policy evaluation pass.
#[derive(Debug, Clone, Default)]
pub struct PolicyPassStats {
    pub claimed: usize,
    pub evaluated: usize,
    pub decisions: usize,
    pub errors: usize,
    pub duration_ms: u128,
}
