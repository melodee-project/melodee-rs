//! Core policy engine: evaluates a Release's classification and novelty against
//! the versioned ruleset to produce a disposition (PRD-POLICY-001 through 006).

use std::collections::BTreeSet;

use melodee_domain::{
    Confidence, DecisionCompletionState, Disposition, EvidenceSource, PolicyActor,
    ReleasePolicyDecision, ReleasePolicyDecisionId,
};
use time::OffsetDateTime;

use crate::classifier::{ClassificationInput, Classifier};
use crate::config::PolicyConfig;
use crate::novelty::NoveltyCategory;

/// Constructs a Confidence from a known-valid basis-point value.
/// Falls back to 0 (always valid) if the value exceeds the maximum.
fn confidence(bp: u16) -> Confidence {
    Confidence::new(bp).unwrap_or_else(|_| confidence(0))
}

/// Default policy rule set identifier for Phase 5.
pub const DEFAULT_RULE_SET_VERSION: u32 = 1;

/// Default rule set ID (deterministic for reproducibility).
pub fn default_rule_set_id() -> melodee_domain::PolicyRuleSetId {
    melodee_domain::PolicyRuleSetId::new()
}

/// All default policy rules as a static list for traceability.
pub fn default_rules() -> DefaultRules {
    DefaultRules {
        rules: vec![
            "POL-001: Destructive disposition requires confidence >= 80% and non-directory-only evidence source.",
            "POL-002: Releases with AdminTag (Administrator) evidence cannot retain default Keep disposition.",
            "POL-003: Releases classified as Bootleg or Tribute with non-directory evidence get Exclude disposition.",
            "POL-004: BrandNewRelease or Remaster novelty requires manual review before destructive action.",
            "POL-005: Directory-only evidence can never produce a destructive disposition.",
            "POL-006: Releases classified as Unknown are deferred to ManualReview.",
        ],
    }
}

#[derive(Debug, Clone)]
pub struct DefaultRules {
    pub rules: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct DefaultPolicyRuleSet {
    pub id: melodee_domain::PolicyRuleSetId,
    pub version: u32,
    pub rules: DefaultRules,
}

pub fn default_rule_set() -> DefaultPolicyRuleSet {
    DefaultPolicyRuleSet {
        id: default_rule_set_id(),
        version: DEFAULT_RULE_SET_VERSION,
        rules: default_rules(),
    }
}

/// A rule match contributing to evidence for a decision.
#[derive(Debug, Clone)]
pub struct RuleMatch {
    pub rule_id: String,
    pub description: String,
    pub confidence: Confidence,
}

/// Evaluated policy context for a Release.
#[derive(Debug, Clone)]
pub struct PolicyInput {
    pub release_id: melodee_domain::ReleaseId,
    pub title: Option<String>,
    pub directory_name: Option<String>,
    pub parent_directory_name: Option<String>,
    pub structured_tag_traits: Vec<melodee_domain::ReleaseTrait>,
    pub embedded_external_id_traits: Vec<melodee_domain::ReleaseTrait>,
    pub catalog_known_release_traits: Vec<melodee_domain::ReleaseTrait>,
    pub existing_classification_traits: BTreeSet<melodee_domain::ReleaseTrait>,
    pub actors: Vec<PolicyActor>,
    pub novelty_category: NoveltyCategory,
    pub now: OffsetDateTime,
}

/// Pure policy engine: combines classification evidence with novelty to
/// produce a disposition and matched rule IDs.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    classifier: Classifier,
    config: PolicyConfig,
    rule_set: DefaultPolicyRuleSet,
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            classifier: Classifier,
            config,
            rule_set: default_rule_set(),
        }
    }

    pub fn rule_set_id(&self) -> melodee_domain::PolicyRuleSetId {
        self.rule_set.id
    }

    pub fn rule_set_version(&self) -> u32 {
        self.rule_set.version
    }

    /// Evaluates a Release and returns its classification, decision, and
    /// matched rules.
    ///
    /// Dispositions:
    /// - `Keep`: safe to retain (does not meet destructive thresholds)
    /// - `Exclude`: classified as Bootleg/Tribute with sufficient evidence
    /// - `ManualReview`: needs human inspection (BrandNewRelease, Remaster,
    ///   Unknown classification)
    /// - `FlagForDeletion`: confident destructive evidence from non-directory
    ///   source, with operator override recorded
    ///
    /// Directory-only evidence never yields a destructive disposition
    /// (POL-005).
    pub fn evaluate(&self, input: &PolicyInput) -> PolicyEvaluation {
        let mut matched_rules: Vec<RuleMatch> = Vec::new();

        // Step 1: classify the release.
        let class_input = ClassificationInput {
            release_id: input.release_id,
            title: input.title.clone(),
            directory_name: input.directory_name.clone(),
            parent_directory_name: input.parent_directory_name.clone(),
            structured_tag_traits: input.structured_tag_traits.clone(),
            embedded_external_id_traits: input.embedded_external_id_traits.clone(),
            catalog_known_release_traits: input.catalog_known_release_traits.clone(),
            existing_classification_traits: input.existing_classification_traits.clone(),
        };
        let classification = self.classifier.classify(&class_input, input.now);

        // Step 2: analyze evidence for rule matching.
        let best_confidence = classification
            .evidence
            .iter()
            .map(|e| e.confidence)
            .max()
            .unwrap_or(confidence(0));
        let best_source = classification
            .evidence
            .iter()
            .min_by_key(|e| e.source.trust_rank())
            .map(|e| e.source);

        let evidence_source_is_directory_only = classification
            .evidence
            .iter()
            .min_by_key(|e| e.source.trust_rank())
            .is_some_and(|e| e.source.is_directory_only());

        let has_admin_evidence = classification
            .evidence
            .iter()
            .any(|e| e.source == EvidenceSource::Administrator);

        let has_bootleg_or_tribute = classification
            .traits
            .contains(&melodee_domain::ReleaseTrait::Bootleg)
            || classification
                .traits
                .contains(&melodee_domain::ReleaseTrait::Tribute);

        let has_unknown_trait = classification
            .traits
            .contains(&melodee_domain::ReleaseTrait::Unknown);

        let threshold = Confidence::new(self.config.destructive_confidence_bps)
            .unwrap_or_else(|_| confidence(8_000));

        // POL-001: Destructive disposition requires confidence >= threshold
        // and non-directory-only evidence.
        if best_confidence >= threshold && !evidence_source_is_directory_only {
            matched_rules.push(RuleMatch {
                rule_id: "POL-001".to_string(),
                description: format!(
                    "Confidence {} basis points >= threshold {} from non-directory evidence source {:?}",
                    best_confidence.basis_points(),
                    threshold.basis_points(),
                    best_source.unwrap_or(EvidenceSource::ReleaseTitle)
                ),
                confidence: best_confidence,
            });
        }

        // POL-002: Administrator evidence present.
        if has_admin_evidence {
            matched_rules.push(RuleMatch {
                rule_id: "POL-002".to_string(),
                description: "Evidence includes Administrator (AdminTag) source".to_string(),
                confidence: best_confidence,
            });
        }

        // POL-003: Bootleg/Tribute with non-directory evidence → Exclude.
        if has_bootleg_or_tribute && !evidence_source_is_directory_only {
            matched_rules.push(RuleMatch {
                rule_id: "POL-003".to_string(),
                description: format!(
                    "Classified as Bootleg/Tribute with confidence {}; non-directory evidence source {:?}",
                    best_confidence.basis_points(),
                    best_source.unwrap_or(EvidenceSource::ReleaseTitle)
                ),
                confidence: best_confidence,
            });
        }

        // POL-004: BrandNewRelease or Remaster novelty → ManualReview.
        if input.novelty_category == NoveltyCategory::BrandNewRelease
            || input.novelty_category == NoveltyCategory::Remaster
        {
            matched_rules.push(RuleMatch {
                rule_id: "POL-004".to_string(),
                description: format!(
                    "Novelty category {:?} requires manual review before destructive action",
                    input.novelty_category
                ),
                confidence: confidence(5_000),
            });
        }

        // POL-006: Unknown trait → ManualReview.
        if has_unknown_trait {
            matched_rules.push(RuleMatch {
                rule_id: "POL-006".to_string(),
                description: "Release classified as Unknown; deferring to manual review"
                    .to_string(),
                confidence: confidence(1_000),
            });
        }

        // Step 3: determine disposition from matched rules.
        let rule_ids: BTreeSet<&str> = matched_rules.iter().map(|r| r.rule_id.as_str()).collect();

        let (disposition, evidence_summary) = self.disposition_from_rules(
            &classification,
            &rule_ids,
            best_confidence,
            threshold,
            evidence_source_is_directory_only,
            input.actors.is_empty(),
            has_bootleg_or_tribute,
        );

        let matched_rule_ids: BTreeSet<String> =
            matched_rules.iter().map(|r| r.rule_id.clone()).collect();

        let actor = input
            .actors
            .first()
            .cloned()
            .unwrap_or(PolicyActor::System {
                policy_rule_set_version: self.rule_set.version,
            });

        let decision = ReleasePolicyDecision {
            id: ReleasePolicyDecisionId::new(),
            release_id: input.release_id,
            rule_set_id: self.rule_set.id,
            rule_set_version: self.rule_set.version,
            matched_rule_ids: matched_rule_ids.into_iter().collect(),
            disposition,
            evidence_summary,
            source_fingerprint: None,
            actor,
            decided_at: input.now,
            completion_state: DecisionCompletionState::Completed,
            completed_at: Some(input.now),
        };

        PolicyEvaluation {
            classification,
            decision,
            matched_rules,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn disposition_from_rules(
        &self,
        classification: &melodee_domain::ReleaseClassification,
        rule_ids: &BTreeSet<&str>,
        confidence: Confidence,
        threshold: Confidence,
        evidence_source_is_directory_only: bool,
        has_no_actor: bool,
        has_bootleg_or_tribute: bool,
    ) -> (Disposition, String) {
        // POL-004: BrandNewRelease/Remaster → ManualReview (unless Exclude
        // is also triggered by Bootleg/Tribute).
        if rule_ids.contains("POL-004") {
            if has_bootleg_or_tribute && !evidence_source_is_directory_only {
                return (
                    Disposition::Ignore,
                    format!(
                        "Bootleg/Tribute classification (confidence {}) with non-directory evidence; \
                         novelty requires review but trait is destructive",
                        confidence.basis_points()
                    ),
                );
            }
            return (
                Disposition::Review,
                format!(
                    "Novelty category requires manual review before destructive action; \
                     confidence {} basis points; rules matched: {:?}",
                    confidence.basis_points(),
                    rule_ids
                ),
            );
        }

        // POL-003: Bootleg/Tribute with non-directory evidence → Exclude.
        if rule_ids.contains("POL-003") && !evidence_source_is_directory_only {
            return (
                Disposition::Ignore,
                format!(
                    "Release classified as Bootleg/Tribute with confidence {} basis points \
                     from non-directory evidence source",
                    confidence.basis_points()
                ),
            );
        }

        // POL-001: High confidence non-directory evidence → FlagForDeletion
        // (with operator override if actor present).
        if rule_ids.contains("POL-001") {
            if has_no_actor {
                return (
                    Disposition::Review,
                    format!(
                        "Confidence {} basis points >= threshold {} but no operator override \
                         recorded; manual review required",
                        confidence.basis_points(),
                        threshold.basis_points()
                    ),
                );
            }
            return (
                Disposition::Delete,
                format!(
                    "Confidence {} basis points >= threshold {} from non-directory evidence; \
                     operator override present",
                    confidence.basis_points(),
                    threshold.basis_points()
                ),
            );
        }

        // POL-006: Unknown classification → ManualReview.
        if rule_ids.contains("POL-006") {
            return (
                Disposition::Review,
                "Release classification is Unknown; insufficient evidence for automated decision"
                    .to_string(),
            );
        }

        // Default: Keep.
        (
            Disposition::Accept,
            format!(
                "No destructive rules matched; release classified with traits {:?}; \
                 confidence {} basis points",
                classification.traits,
                confidence.basis_points()
            ),
        )
    }
}

/// Result of a single policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    pub classification: melodee_domain::ReleaseClassification,
    pub decision: ReleasePolicyDecision,
    pub matched_rules: Vec<RuleMatch>,
}
