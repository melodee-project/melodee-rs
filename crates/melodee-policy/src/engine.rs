//! Ordered, versioned Release policy evaluation (PRD-POLICY-001–008).

use melodee_domain::{
    AdministratorOverride, Confidence, DecisionCompletionState, Disposition, DomainInvariantError,
    PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleSetId, PolicyRuleType, ReleaseClassification,
    ReleasePolicyDecision, ReleasePolicyDecisionId, ReleaseTrait,
};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::classifier::{ClassificationError, ClassificationInput, Classifier};
use crate::config::{PolicyConfig, PolicyConfigError};
use crate::novelty::{NoveltyAssessment, NoveltyEngine, NoveltyEngineError, NoveltyInput};

pub const DEFAULT_RULE_SET_VERSION: u32 = 1;
pub const DEFAULT_RULE_SET_NAME: &str = "phase5-default-rules";

/// Stable UUIDv7 derived from the fixed default name namespace and explicit
/// rule-set version. Preference changes require a version increment.
#[must_use]
pub fn default_rule_set_id(version: u32) -> PolicyRuleSetId {
    let mut bytes = [
        0x01, 0x98, 0x9f, 0x00, 0x70, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
        0x00,
    ];
    bytes[12..].copy_from_slice(&version.to_be_bytes());
    PolicyRuleSetId::from_uuid(Uuid::from_bytes(bytes))
}

/// Builds the persisted default rule set from validated preferences. The rules
/// are evaluated in this exact order.
pub fn default_rule_set(config: &PolicyConfig) -> Result<PolicyRuleSet, PolicyEngineError> {
    config.validate()?;
    PolicyRuleSet::new(PolicyRuleSet {
        id: default_rule_set_id(config.rule_set_version),
        name: DEFAULT_RULE_SET_NAME.to_string(),
        version: config.rule_set_version,
        rules: default_rules(config),
        created_at: OffsetDateTime::UNIX_EPOCH,
        created_by: PolicyActor::System {
            policy_rule_set_version: config.rule_set_version,
        },
    })
    .map_err(PolicyEngineError::from)
}

#[must_use]
pub fn default_rules(config: &PolicyConfig) -> Vec<PolicyRule> {
    vec![
        rule(
            "POL-ADMIN-OVERRIDE",
            PolicyRuleType::AdministratorOverride,
            Disposition::Review,
            None,
            json!({}),
        ),
        rule(
            "POL-CORRUPT",
            PolicyRuleType::CorruptMedia,
            Disposition::Quarantine,
            None,
            json!({}),
        ),
        rule(
            "POL-GH-UNCERTAIN",
            PolicyRuleType::GreatestHitsUncertain,
            config.uncertain_greatest_hits_disposition,
            None,
            json!({}),
        ),
        rule(
            "POL-GH-NOVEL",
            PolicyRuleType::GreatestHitsNovelty,
            config.novel_greatest_hits_disposition,
            None,
            json!({}),
        ),
        rule(
            "POL-GH-REDUNDANT",
            PolicyRuleType::GreatestHitsRedundancy,
            config.redundant_greatest_hits_disposition,
            (config.redundant_greatest_hits_disposition == Disposition::Delete)
                .then(|| Confidence::new(config.destructive_confidence_bps).ok())
                .flatten(),
            json!({}),
        ),
        rule(
            "POL-UNKNOWN",
            PolicyRuleType::UnknownClassification,
            Disposition::Review,
            None,
            json!({}),
        ),
    ]
}

fn rule(
    id: &str,
    rule_type: PolicyRuleType,
    disposition: Disposition,
    confidence_threshold: Option<Confidence>,
    predicate_json: serde_json::Value,
) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        rule_type,
        predicate_json,
        disposition,
        confidence_threshold,
    }
}

/// Exact inputs to one durable policy decision. Classification and novelty are
/// already validated domain results for the same Release and rule set.
#[derive(Debug, Clone)]
pub struct PolicyInput {
    pub classification: ReleaseClassification,
    pub novelty: NoveltyAssessment,
    pub existing_disposition: Option<Disposition>,
    /// Immutable sealed-manifest/source BLAKE3 fingerprint.
    pub source_fingerprint: String,
    pub has_corrupt_media: bool,
    pub administrator_override: Option<AdministratorOverride>,
    pub now: OffsetDateTime,
}

/// One matched persisted rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub rule_id: String,
    pub description: String,
    pub confidence: Confidence,
}

#[derive(Debug, Error)]
pub enum PolicyEngineError {
    #[error("invalid policy configuration")]
    Config(#[from] PolicyConfigError),
    #[error("policy domain invariant failed")]
    Domain(#[from] DomainInvariantError),
    #[error("classification failed")]
    Classification(#[from] ClassificationError),
    #[error("novelty evaluation failed")]
    Novelty(#[from] NoveltyEngineError),
    #[error("policy classification and novelty refer to different Releases")]
    ReleaseMismatch,
    #[error("novelty was evaluated under a different policy rule set")]
    RuleSetMismatch,
    #[error("policy rule {rule_id} has an invalid predicate: {detail}")]
    InvalidPredicate { rule_id: String, detail: String },
    #[error("the ordered rule set must begin with the administrator override rule")]
    MissingAdministratorOverrideRule,
    #[error("the administrator override rule must have an empty predicate")]
    InvalidAdministratorOverridePredicate,
    #[error("the corrupt-media quarantine rule must be second and have an empty predicate")]
    MissingCorruptMediaSafetyRule,
    #[error("configured rule-set version {configured} does not match loaded version {loaded}")]
    ConfiguredRuleSetVersionMismatch { configured: u32, loaded: u32 },
    #[error("configured trait policy rule {rule_id} must require at least one trait")]
    EmptyTraitPredicate { rule_id: String },
}

/// Pure policy engine backed by the exact persisted rule set it evaluates.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    config: PolicyConfig,
    rule_set: PolicyRuleSet,
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Result<Self, PolicyEngineError> {
        let rule_set = default_rule_set(&config)?;
        Self::with_rule_set(config, rule_set)
    }

    pub fn with_rule_set(
        config: PolicyConfig,
        rule_set: PolicyRuleSet,
    ) -> Result<Self, PolicyEngineError> {
        config.validate()?;
        let rule_set = PolicyRuleSet::new(rule_set)?;
        if config.rule_set_version != rule_set.version {
            return Err(PolicyEngineError::ConfiguredRuleSetVersionMismatch {
                configured: config.rule_set_version,
                loaded: rule_set.version,
            });
        }
        if rule_set.rules.first().map(|rule| rule.rule_type)
            != Some(PolicyRuleType::AdministratorOverride)
        {
            return Err(PolicyEngineError::MissingAdministratorOverrideRule);
        }
        if rule_set.rules.first().is_some_and(|rule| {
            rule.predicate_json
                .as_object()
                .is_none_or(|predicate| !predicate.is_empty())
        }) {
            return Err(PolicyEngineError::InvalidAdministratorOverridePredicate);
        }
        if !rule_set.rules.get(1).is_some_and(|rule| {
            rule.rule_type == PolicyRuleType::CorruptMedia
                && rule.disposition == Disposition::Quarantine
                && rule
                    .predicate_json
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty)
        }) || rule_set
            .rules
            .iter()
            .filter(|rule| rule.rule_type == PolicyRuleType::CorruptMedia)
            .count()
            != 1
        {
            return Err(PolicyEngineError::MissingCorruptMediaSafetyRule);
        }
        for rule in &rule_set.rules {
            let predicate = parse_predicate(rule)?;
            if rule.rule_type == PolicyRuleType::TraitDisposition
                && predicate.required_traits.is_empty()
            {
                return Err(PolicyEngineError::EmptyTraitPredicate {
                    rule_id: rule.id.clone(),
                });
            }
        }
        Ok(Self { config, rule_set })
    }

    #[must_use]
    pub const fn rule_set(&self) -> &PolicyRuleSet {
        &self.rule_set
    }

    #[must_use]
    pub const fn rule_set_id(&self) -> PolicyRuleSetId {
        self.rule_set.id
    }

    #[must_use]
    pub const fn rule_set_version(&self) -> u32 {
        self.rule_set.version
    }

    #[must_use]
    pub const fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    pub fn evaluate(&self, input: &PolicyInput) -> Result<PolicyEvaluation, PolicyEngineError> {
        let classification = ReleaseClassification::new(input.classification.clone())?;
        let novelty = input.novelty.evaluation();
        if classification.release_id != novelty.release_id {
            return Err(PolicyEngineError::ReleaseMismatch);
        }
        if novelty.policy_rule_set_id != self.rule_set.id {
            return Err(PolicyEngineError::RuleSetMismatch);
        }
        let administrator_override = input
            .administrator_override
            .clone()
            .map(AdministratorOverride::new)
            .transpose()?;

        let context = EvaluationContext {
            classification: &classification,
            novelty,
            existing_disposition: input.existing_disposition,
            has_corrupt_media: input.has_corrupt_media,
            administrator_override: administrator_override.as_ref(),
        };

        let mut matched = None;
        for rule in &self.rule_set.rules {
            let predicate = parse_predicate(rule)?;
            if predicate_matches(&predicate, &context) && rule_matches(rule, &predicate, &context) {
                matched = Some((rule, predicate, describe_match(rule, &context)));
                break;
            }
        }

        let (disposition, actor, evidence_summary, matched_rules) = match matched {
            Some((rule, predicate, description)) => {
                let confidence = confidence_for_rule(rule, &predicate, &classification);
                let disposition = if rule.rule_type == PolicyRuleType::AdministratorOverride {
                    administrator_override
                        .as_ref()
                        .map_or(rule.disposition, |override_| override_.disposition)
                } else {
                    rule.disposition
                };
                let actor = administrator_override.as_ref().map_or_else(
                    || PolicyActor::System {
                        policy_rule_set_version: self.rule_set.version,
                    },
                    AdministratorOverride::policy_actor,
                );
                let summary = administrator_override.as_ref().map_or_else(
                    || description.clone(),
                    |override_| {
                        format!(
                            "Administrator '{}' selected {}: {}",
                            override_.actor,
                            override_.disposition.as_str(),
                            override_.reason
                        )
                    },
                );
                (
                    disposition,
                    actor,
                    summary,
                    vec![RuleMatch {
                        rule_id: rule.id.clone(),
                        description,
                        confidence,
                    }],
                )
            }
            None => (
                Disposition::Accept,
                PolicyActor::System {
                    policy_rule_set_version: self.rule_set.version,
                },
                "No configured policy rule matched; accepting Release".to_string(),
                Vec::new(),
            ),
        };

        let decision = ReleasePolicyDecision::new(ReleasePolicyDecision {
            id: ReleasePolicyDecisionId::new(),
            release_id: classification.release_id,
            rule_set_id: self.rule_set.id,
            rule_set_version: self.rule_set.version,
            matched_rule_ids: matched_rules
                .iter()
                .map(|rule_match| rule_match.rule_id.clone())
                .collect(),
            disposition,
            evidence_summary,
            source_fingerprint: input.source_fingerprint.clone(),
            actor,
            decided_at: input.now,
            completion_state: DecisionCompletionState::Pending,
            completed_at: None,
        })?;

        Ok(PolicyEvaluation {
            classification,
            novelty: input.novelty.clone(),
            decision,
            matched_rules,
            existing_disposition: input.existing_disposition,
        })
    }

    /// One pure orchestration entry point shared by the worker and CLI. Callers
    /// map authoritative persistence facts only; classification, novelty, and
    /// policy sequencing stays here.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_facts(
        &self,
        classification_input: &ClassificationInput,
        novelty_input: &NoveltyInput,
        existing_disposition: Option<Disposition>,
        source_fingerprint: String,
        has_corrupt_media: bool,
        administrator_override: Option<AdministratorOverride>,
        now: OffsetDateTime,
    ) -> Result<PolicyEvaluation, PolicyEngineError> {
        if classification_input.release_id != novelty_input.release_id {
            return Err(PolicyEngineError::ReleaseMismatch);
        }
        if novelty_input.policy_rule_set_id != self.rule_set.id {
            return Err(PolicyEngineError::RuleSetMismatch);
        }
        let classification = Classifier.classify(classification_input, now)?;
        let novelty = NoveltyEngine.assess(novelty_input)?;
        self.evaluate(&PolicyInput {
            classification,
            novelty,
            existing_disposition,
            source_fingerprint,
            has_corrupt_media,
            administrator_override,
            now,
        })
    }
}

struct EvaluationContext<'a> {
    classification: &'a ReleaseClassification,
    novelty: &'a melodee_domain::NoveltyEvaluation,
    existing_disposition: Option<Disposition>,
    has_corrupt_media: bool,
    administrator_override: Option<&'a AdministratorOverride>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulePredicate {
    #[serde(default)]
    required_traits: Vec<ReleaseTrait>,
    existing_dispositions: Option<Vec<Disposition>>,
}

fn parse_predicate(rule: &PolicyRule) -> Result<RulePredicate, PolicyEngineError> {
    serde_json::from_value(rule.predicate_json.clone()).map_err(|error| {
        PolicyEngineError::InvalidPredicate {
            rule_id: rule.id.clone(),
            detail: error.to_string(),
        }
    })
}

fn predicate_matches(predicate: &RulePredicate, context: &EvaluationContext<'_>) -> bool {
    predicate
        .required_traits
        .iter()
        .all(|trait_| context.classification.has_trait(*trait_))
        && predicate
            .existing_dispositions
            .as_ref()
            .is_none_or(|allowed| {
                context
                    .existing_disposition
                    .is_some_and(|existing| allowed.contains(&existing))
            })
}

fn rule_matches(
    rule: &PolicyRule,
    predicate: &RulePredicate,
    context: &EvaluationContext<'_>,
) -> bool {
    match rule.rule_type {
        PolicyRuleType::AdministratorOverride => context.administrator_override.is_some(),
        PolicyRuleType::CorruptMedia => context.has_corrupt_media,
        PolicyRuleType::GreatestHitsUncertain => {
            context.classification.has_trait(ReleaseTrait::GreatestHits)
                && context.novelty.has_uncertain_tracks()
        }
        PolicyRuleType::GreatestHitsNovelty => {
            context.classification.has_trait(ReleaseTrait::GreatestHits)
                && context.novelty.novel_track_count() > 0
        }
        PolicyRuleType::GreatestHitsRedundancy => {
            let confidence = context
                .classification
                .strongest_non_directory_confidence_for(ReleaseTrait::GreatestHits);
            context.classification.has_trait(ReleaseTrait::GreatestHits)
                && context.novelty.is_redundant()
                && confidence.is_some()
                && rule
                    .confidence_threshold
                    .is_none_or(|threshold| confidence.is_some_and(|value| value >= threshold))
        }
        PolicyRuleType::UnknownClassification => {
            context.classification.has_trait(ReleaseTrait::Unknown)
        }
        PolicyRuleType::TraitDisposition => rule.confidence_threshold.is_none_or(|threshold| {
            predicate.required_traits.iter().all(|trait_| {
                context
                    .classification
                    .strongest_non_directory_confidence_for(*trait_)
                    .is_some_and(|confidence| confidence >= threshold)
            })
        }),
    }
}

fn confidence_for_rule(
    rule: &PolicyRule,
    predicate: &RulePredicate,
    classification: &ReleaseClassification,
) -> Confidence {
    match rule.rule_type {
        PolicyRuleType::GreatestHitsRedundancy
        | PolicyRuleType::GreatestHitsNovelty
        | PolicyRuleType::GreatestHitsUncertain => classification
            .strongest_non_directory_confidence_for(ReleaseTrait::GreatestHits)
            .unwrap_or(Confidence::zero()),
        PolicyRuleType::TraitDisposition => predicate
            .required_traits
            .iter()
            .map(|trait_| {
                classification
                    .strongest_non_directory_confidence_for(*trait_)
                    .unwrap_or(Confidence::zero())
            })
            .min()
            .unwrap_or(Confidence::zero()),
        _ => classification.confidence,
    }
}

fn describe_match(rule: &PolicyRule, context: &EvaluationContext<'_>) -> String {
    match rule.rule_type {
        PolicyRuleType::AdministratorOverride => "Explicit administrator override".to_string(),
        PolicyRuleType::CorruptMedia => "Corrupt or unreadable media".to_string(),
        PolicyRuleType::GreatestHitsUncertain => format!(
            "Greatest Hits contains {} uncertain Track(s)",
            context.novelty.uncertain_count
        ),
        PolicyRuleType::GreatestHitsNovelty => format!(
            "Greatest Hits contains {} new/alternate Track(s)",
            context.novelty.novel_track_count()
        ),
        PolicyRuleType::GreatestHitsRedundancy => format!(
            "Greatest Hits contains {} redundant Track(s) and no novel or uncertain Tracks",
            context.novelty.redundant_track_count()
        ),
        PolicyRuleType::UnknownClassification => "Release classification is Unknown".to_string(),
        PolicyRuleType::TraitDisposition => {
            format!("Configured trait rule {} matched", rule.id)
        }
    }
}

/// Result of a single policy evaluation and its Pending durable decision.
/// Persistence completes safe routed outcomes atomically; Review and Delete
/// remain Pending until their required operator/action boundary succeeds.
#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    pub classification: ReleaseClassification,
    /// Exact aggregate and per-Track novelty rows used by this decision.
    pub novelty: NoveltyAssessment,
    pub decision: ReleasePolicyDecision,
    pub matched_rules: Vec<RuleMatch>,
    pub existing_disposition: Option<Disposition>,
}
