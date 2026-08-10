//! Phase 5 policy engine: Release classification, Track novelty analysis,
//! versioned policy evaluation, and dry-run/test-against-catalog support.
//!
//! This crate owns the logic that decides what Melodee believes a Release is
//! (classification) and what Melodee wants to do with it (policy disposition).
//! It depends on `melodee-domain` for pure vocabulary and `melodee-persistence`
//! for durable catalog read/write. It does not own filesystem deletion.

#![forbid(unsafe_code)]

mod classifier;
mod config;
mod dry_run;
mod engine;
mod novelty;

pub use classifier::{
    ClassificationInput, Classifier, DEFAULT_CLASSIFIER_VERSION, DEFAULT_DESTRUCTIVE_CONFIDENCE,
};
pub use config::{
    ENV_POLICY_BATCH_SIZE, ENV_POLICY_CONCURRENCY, ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS,
    PolicyConfig, PolicyConfigError,
};
pub use dry_run::{PolicyDryRun, PolicyDryRunResult, PolicyDryRunner};
pub use engine::{
    DefaultPolicyRuleSet, DefaultRules, PolicyEngine, PolicyEvaluation, PolicyInput, default_rules,
};
pub use novelty::{
    NoveltyAssessment, NoveltyCategory, NoveltyEngine, NoveltyEngineError, NoveltyInput,
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
