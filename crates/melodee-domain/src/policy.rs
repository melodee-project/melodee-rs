//! Phase 5 versioned policy rules, durable decisions, and dry-run/test support
//! (PRD-POLICY-001 through PRD-POLICY-008, SPEC section 15).
//!
//! Classification describes what Melodee believes. Policy decides what to do.
//! A policy evaluation consumes a `ReleaseClassification` and `NoveltyEvaluation`
//! (plus existing disposition context) and returns a `Disposition`. Before any
//! destructive action, a `ReleasePolicyDecision` is persisted as a durable
//! record (PRD-POLICY-007).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{is_blake3, validate_internal_id, validate_nonblank};
use crate::{
    Confidence, Disposition, DomainInvariantError, PolicyRuleSetId, PolicyTestRunId, ReleaseId,
    ReleasePolicyDecisionId,
};

/// Maximum Releases represented by one durable policy test summary.
pub const MAX_POLICY_TEST_RELEASES: usize = 10_000;
/// Maximum distinct matched-rule counters in one policy test summary.
pub const MAX_POLICY_TEST_MATCHED_RULES: usize = 1_000;
/// Maximum representative Release IDs retained by a policy test summary.
pub const MAX_POLICY_TEST_SAMPLES: usize = 20;

/// Who (or what) is acting on a policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "authority", rename_all = "snake_case")]
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

/// An explicit administrator-selected outcome. Merely naming an administrator
/// never authorizes a disposition; the actor, chosen outcome, and reason travel
/// together as one provenance record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdministratorOverride {
    pub actor: String,
    pub disposition: Disposition,
    pub reason: String,
}

impl AdministratorOverride {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_nonblank("administrator override actor", &value.actor)?;
        validate_nonblank("administrator override reason", &value.reason)?;
        Ok(value)
    }

    #[must_use]
    pub fn policy_actor(&self) -> PolicyActor {
        PolicyActor::Administrator {
            actor: self.actor.clone(),
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
        if let PolicyActor::System {
            policy_rule_set_version,
        } = &value.created_by
            && *policy_rule_set_version != value.version
        {
            return Err(DomainInvariantError::PolicyActorVersionMismatch);
        }
        let mut rule_ids = BTreeSet::new();
        for rule in &value.rules {
            rule.validate()?;
            if !rule_ids.insert(rule.id.as_str()) {
                return Err(DomainInvariantError::DuplicatePolicyRuleId);
            }
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
        if !self.predicate_json.is_object() {
            return Err(DomainInvariantError::BlankValue(
                "policy rule predicate object",
            ));
        }
        let safe_disposition = match self.rule_type {
            PolicyRuleType::GreatestHitsRedundancy => {
                matches!(self.disposition, Disposition::Ignore | Disposition::Delete)
            }
            PolicyRuleType::GreatestHitsNovelty => {
                matches!(self.disposition, Disposition::Accept | Disposition::Review)
            }
            PolicyRuleType::GreatestHitsUncertain => matches!(
                self.disposition,
                Disposition::Review | Disposition::Quarantine
            ),
            PolicyRuleType::CorruptMedia => self.disposition == Disposition::Quarantine,
            PolicyRuleType::UnknownClassification => self.disposition == Disposition::Review,
            PolicyRuleType::TraitDisposition => !self.disposition.is_destructive(),
            PolicyRuleType::AdministratorOverride => true,
        };
        if !safe_disposition {
            return Err(DomainInvariantError::InvalidPolicyRuleDisposition);
        }
        if self.disposition == Disposition::Delete && self.confidence_threshold.is_none() {
            return Err(DomainInvariantError::InvalidPolicyRuleDisposition);
        }
        Ok(())
    }
}

/// Kinds of policy rules the built-in engine knows how to evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRuleType {
    /// Greatest Hits redundancy check (PRD-POLICY-003/006).
    GreatestHitsRedundancy,
    /// Greatest Hits containing new or alternate material.
    GreatestHitsNovelty,
    /// Greatest Hits containing any uncertain Track evidence.
    GreatestHitsUncertain,
    /// Corrupt/unreadable media disposition (PRD-POLICY-006).
    CorruptMedia,
    /// Administrator override — takes precedence over all automated rules.
    AdministratorOverride,
    /// Unknown classification requires operator review.
    UnknownClassification,
    /// Explicit operator-configured outcome for named non-Greatest-Hits traits.
    TraitDisposition,
}

impl PolicyRuleType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GreatestHitsRedundancy => "greatest_hits_redundancy",
            Self::GreatestHitsNovelty => "greatest_hits_novelty",
            Self::GreatestHitsUncertain => "greatest_hits_uncertain",
            Self::CorruptMedia => "corrupt_media",
            Self::AdministratorOverride => "administrator_override",
            Self::UnknownClassification => "unknown_classification",
            Self::TraitDisposition => "trait_disposition",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::GreatestHitsRedundancy,
            Self::GreatestHitsNovelty,
            Self::GreatestHitsUncertain,
            Self::CorruptMedia,
            Self::AdministratorOverride,
            Self::UnknownClassification,
            Self::TraitDisposition,
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
        if value.rule_set_version == 0 {
            return Err(DomainInvariantError::InvalidPolicyVersion);
        }
        for rule_id in &value.matched_rule_ids {
            validate_nonblank("matched policy rule id", rule_id)?;
        }
        if value.potential_destructive != value.disposition.is_destructive() {
            return Err(DomainInvariantError::InvalidPolicyRuleDisposition);
        }
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
    /// Decision is durable but still requires a state-routing commit,
    /// administrator review, or an authorized follow-up action.
    Pending,
    /// The non-destructive state route or authorized follow-up action committed.
    Completed,
    /// Required routing/action failed; the decision remains durable.
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
    /// Immutable sealed-manifest/source fingerprint (lowercase BLAKE3 hex).
    pub source_fingerprint: String,
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
        if let PolicyActor::System {
            policy_rule_set_version,
        } = &value.actor
            && *policy_rule_set_version != value.rule_set_version
        {
            return Err(DomainInvariantError::PolicyActorVersionMismatch);
        }
        validate_nonblank("policy decision evidence summary", &value.evidence_summary)?;
        for rule_id in &value.matched_rule_ids {
            validate_nonblank("matched policy rule id", rule_id)?;
        }
        if !is_blake3(&value.source_fingerprint) {
            return Err(DomainInvariantError::MissingPolicySourceFingerprint);
        }
        let completion_is_consistent = match value.completion_state {
            DecisionCompletionState::Pending => value.completed_at.is_none(),
            DecisionCompletionState::Completed | DecisionCompletionState::Failed => {
                value.completed_at.is_some_and(|at| at >= value.decided_at)
            }
        };
        if !completion_is_consistent {
            return Err(DomainInvariantError::InvalidDecisionCompletionState);
        }
        Ok(value)
    }

    /// Marks the required state route or authorized follow-up action complete.
    pub fn mark_completed(
        self,
        completed_at: OffsetDateTime,
    ) -> Result<Self, DomainInvariantError> {
        if self.completion_state != DecisionCompletionState::Pending {
            return Err(DomainInvariantError::InvalidDecisionCompletionTransition);
        }
        if completed_at < self.decided_at {
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
    /// Number of Releases matched by each stable rule identifier.
    pub matched_rule_counts: Vec<(String, u32)>,
    /// Existing durable dispositions; `None` explicitly counts Releases with
    /// no prior decision.
    pub existing_disposition_counts: Vec<(Option<Disposition>, u32)>,
    /// Proposed dispositions produced by the tested rule set.
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
        if let PolicyActor::System {
            policy_rule_set_version,
        } = &value.ran_by
            && *policy_rule_set_version != value.rule_set_version
        {
            return Err(DomainInvariantError::PolicyActorVersionMismatch);
        }
        if usize::try_from(value.total_releases_evaluated).map_err(|_| {
            DomainInvariantError::PolicyTestScopeExceeded {
                dimension: "Releases",
                limit: MAX_POLICY_TEST_RELEASES,
            }
        })? > MAX_POLICY_TEST_RELEASES
        {
            return Err(DomainInvariantError::PolicyTestScopeExceeded {
                dimension: "Releases",
                limit: MAX_POLICY_TEST_RELEASES,
            });
        }
        if value.matched_rule_counts.len() > MAX_POLICY_TEST_MATCHED_RULES {
            return Err(DomainInvariantError::PolicyTestScopeExceeded {
                dimension: "matched rules",
                limit: MAX_POLICY_TEST_MATCHED_RULES,
            });
        }
        let mut matched_rules = BTreeSet::new();
        let mut matched_total = 0_u32;
        for (rule_id, count) in &value.matched_rule_counts {
            validate_nonblank("policy test matched rule id", rule_id)?;
            if *count == 0
                || *count > value.total_releases_evaluated
                || !matched_rules.insert(rule_id.as_str())
            {
                return Err(DomainInvariantError::InvalidPolicyTestSummary);
            }
            matched_total = matched_total
                .checked_add(*count)
                .ok_or(DomainInvariantError::InvalidPolicyTestSummary)?;
        }
        if matched_total > value.total_releases_evaluated {
            return Err(DomainInvariantError::InvalidPolicyTestSummary);
        }
        let proposed_total = validate_disposition_counts(&value.disposition_counts)?;
        let existing_total =
            validate_existing_disposition_counts(&value.existing_disposition_counts)?;
        if proposed_total != value.total_releases_evaluated
            || existing_total != value.total_releases_evaluated
            || value.destructive_impact_count
                != value
                    .disposition_counts
                    .iter()
                    .find_map(|(disposition, count)| {
                        (*disposition == Disposition::Delete).then_some(*count)
                    })
                    .unwrap_or(0)
        {
            return Err(DomainInvariantError::InvalidPolicyTestSummary);
        }
        if value.sample_release_ids.len() > MAX_POLICY_TEST_SAMPLES
            || value.sample_release_ids.len()
                > usize::try_from(value.total_releases_evaluated)
                    .map_err(|_| DomainInvariantError::InvalidPolicyTestSummary)?
        {
            return Err(DomainInvariantError::PolicyTestScopeExceeded {
                dimension: "sample Releases",
                limit: MAX_POLICY_TEST_SAMPLES,
            });
        }
        let mut sample_ids = BTreeSet::new();
        for release_id in &value.sample_release_ids {
            validate_internal_id(release_id.is_uuid_v7(), "ReleaseId")?;
            if !sample_ids.insert(*release_id) {
                return Err(DomainInvariantError::InvalidPolicyTestSummary);
            }
        }
        Ok(value)
    }
}

fn validate_disposition_counts(counts: &[(Disposition, u32)]) -> Result<u32, DomainInvariantError> {
    if counts.len() > Disposition::ALL.len() {
        return Err(DomainInvariantError::InvalidPolicyTestSummary);
    }
    let mut dispositions = BTreeSet::new();
    let mut total = 0_u32;
    for (disposition, count) in counts {
        if *count == 0 || !dispositions.insert(disposition.as_str()) {
            return Err(DomainInvariantError::InvalidPolicyTestSummary);
        }
        total = total
            .checked_add(*count)
            .ok_or(DomainInvariantError::InvalidPolicyTestSummary)?;
    }
    Ok(total)
}

fn validate_existing_disposition_counts(
    counts: &[(Option<Disposition>, u32)],
) -> Result<u32, DomainInvariantError> {
    if counts.len() > Disposition::ALL.len() + 1 {
        return Err(DomainInvariantError::InvalidPolicyTestSummary);
    }
    let mut dispositions = BTreeSet::new();
    let mut total = 0_u32;
    for (disposition, count) in counts {
        let key = disposition.map_or("none", Disposition::as_str);
        if *count == 0 || !dispositions.insert(key) {
            return Err(DomainInvariantError::InvalidPolicyTestSummary);
        }
        total = total
            .checked_add(*count)
            .ok_or(DomainInvariantError::InvalidPolicyTestSummary)?;
    }
    Ok(total)
}
