//! Phase 5 versioned policy rules, durable decisions, and dry-run/test support
//! (PRD-POLICY-001 through PRD-POLICY-008, SPEC section 15).
//!
//! Classification describes what Melodee believes. Policy decides what to do.
//! A policy evaluation consumes a `ReleaseClassification` and `NoveltyEvaluation`
//! (plus existing disposition context) and returns a `Disposition`. Before any
//! destructive action, a `ReleasePolicyDecision` is persisted as a durable
//! record (PRD-POLICY-007).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{
    Confidence, Disposition, DomainInvariantError, PolicyRuleSetId, PolicyTestRunId, ReleaseId,
    ReleasePolicyDecisionId,
};

/// Who (or what) is acting on a policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyActor {
    /// Automated policy engine evaluation.
    System { policy_rule_set_version: u32 },
    /// An administrator override.
    Administrator { actor: String },
}

impl PolicyActor {
    pub fn validate(&self) -> Result<(), DomainInvariantError> {
        match self {
            Self::System {
                policy_rule_set_version,
            } => {
                if *policy_rule_set_version == 0 {
                    return Err(DomainInvariantError::InvalidPolicyVersion);
                }
                Ok(())
            }
            Self::Administrator { actor } => validate_nonblank("policy decision actor", actor),
        }
    }

    #[must_use]
    pub fn is_administrator(&self) -> bool {
        matches!(self, Self::Administrator { .. })
    }

    #[must_use]
    pub fn rule_set_version(&self) -> u32 {
        match self {
            Self::System {
                policy_rule_set_version,
            } => *policy_rule_set_version,
            Self::Administrator { .. } => 0,
        }
    }
}

/// A versioned policy rule set that maps classification + novelty facts to a disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyRuleSet {
    pub id: PolicyRuleSetId,
    pub name: String,
    pub version: u32,
    pub rules: Vec<PolicyRule>,
    pub created_at: OffsetDateTime,
    pub created_by: PolicyActor,
}

impl PolicyRuleSet {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "PolicyRuleSetId")?;
        validate_nonblank("policy rule-set name", &value.name)?;
        if value.version == 0 {
            return Err(DomainInvariantError::InvalidPolicyVersion);
        }
        value.created_by.validate()?;
        for rule in &value.rules {
            rule.validate()?;
        }
        Ok(value)
    }

    /// Looks up a rule by its stable identifier.
    #[must_use]
    pub fn find_rule(&self, rule_id: &str) -> Option<&PolicyRule> {
        self.rules.iter().find(|rule| rule.id == rule_id)
    }
}

/// One rule within a rule set. The predicate is defined by `predicate_json` so rules
/// can be authored/stored without recompilation. The `rule_type` determines which
/// built-in evaluator interprets the predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    pub rule_type: PolicyRuleType,
    pub predicate_json: serde_json::Value,
    pub disposition: Disposition,
    pub confidence_threshold: Option<Confidence>,
}

impl PolicyRule {
    pub fn validate(&self) -> Result<(), DomainInvariantError> {
        validate_nonblank("policy rule id", &self.id)?;
        Ok(())
    }
}

/// Kinds of policy rules the built-in engine knows how to evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRuleType {
    /// Greatest Hits redundancy check (PRD-POLICY-003/006).
    GreatestHitsRedundancy,
    /// Corrupt/unreadable media disposition (PRD-POLICY-006).
    CorruptMedia,
    /// Administrator override — takes precedence over all automated rules.
    AdministratorOverride,
}

impl PolicyRuleType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GreatestHitsRedundancy => "greatest_hits_redundancy",
            Self::CorruptMedia => "corrupt_media",
            Self::AdministratorOverride => "administrator_override",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::GreatestHitsRedundancy,
            Self::CorruptMedia,
            Self::AdministratorOverride,
        ]
        .into_iter()
        .find(|k| k.as_str() == value)
    }
}

/// The result of evaluating one Release against a policy rule set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyEvaluation {
    pub release_id: ReleaseId,
    pub rule_set_id: PolicyRuleSetId,
    pub rule_set_version: u32,
    pub disposition: Disposition,
    pub matched_rule_ids: Vec<String>,
    pub evidence_summary: String,
    pub potential_destructive: bool,
    pub evaluated_at: OffsetDateTime,
}

impl PolicyEvaluation {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.rule_set_id.is_uuid_v7(), "PolicyRuleSetId")?;
        validate_nonblank(
            "policy evaluation evidence summary",
            &value.evidence_summary,
        )?;
        Ok(value)
    }
}

/// Completion state of a durable policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCompletionState {
    /// Decision recorded but destructive execution has not yet run.
    Pending,
    /// Destructive action (if any) has been executed.
    Completed,
    /// Execution failed; decision stands but action was not completed.
    Failed,
}

impl DecisionCompletionState {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Completed, Self::Failed];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }
}

/// Durable decision record persisted before any destructive action (PRD-POLICY-007).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleasePolicyDecision {
    pub id: ReleasePolicyDecisionId,
    pub release_id: ReleaseId,
    pub rule_set_id: PolicyRuleSetId,
    pub rule_set_version: u32,
    pub matched_rule_ids: Vec<String>,
    pub disposition: Disposition,
    pub evidence_summary: String,
    pub source_fingerprint: Option<String>,
    pub actor: PolicyActor,
    pub decided_at: OffsetDateTime,
    pub completion_state: DecisionCompletionState,
    pub completed_at: Option<OffsetDateTime>,
}

impl ReleasePolicyDecision {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ReleasePolicyDecisionId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.rule_set_id.is_uuid_v7(), "PolicyRuleSetId")?;
        if value.rule_set_version == 0 {
            return Err(DomainInvariantError::InvalidPolicyVersion);
        }
        value.actor.validate()?;
        validate_nonblank("policy decision evidence summary", &value.evidence_summary)?;
        Ok(value)
    }

    /// Marks the destructive-action execution as complete (PRD-POLICY-007).
    pub fn mark_completed(
        self,
        completed_at: OffsetDateTime,
    ) -> Result<Self, DomainInvariantError> {
        if self.completion_state != DecisionCompletionState::Pending {
            return Err(DomainInvariantError::InvalidDecisionCompletionTransition);
        }
        Ok(Self {
            completion_state: DecisionCompletionState::Completed,
            completed_at: Some(completed_at),
            ..self
        })
    }
}

/// Aggregated summary from a dry-run / test-against-catalog query
/// (PRD-POLICY-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyTestSummary {
    pub test_run_id: PolicyTestRunId,
    pub rule_set_id: PolicyRuleSetId,
    pub rule_set_version: u32,
    pub total_releases_evaluated: u32,
    pub disposition_counts: Vec<(Disposition, u32)>,
    pub destructive_impact_count: u32,
    pub sample_release_ids: Vec<ReleaseId>,
    pub ran_by: PolicyActor,
    pub ran_at: OffsetDateTime,
}

impl PolicyTestSummary {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.test_run_id.is_uuid_v7(), "PolicyTestRunId")?;
        validate_internal_id(value.rule_set_id.is_uuid_v7(), "PolicyRuleSetId")?;
        if value.rule_set_version == 0 {
            return Err(DomainInvariantError::InvalidPolicyVersion);
        }
        value.ran_by.validate()?;
        Ok(value)
    }
}
