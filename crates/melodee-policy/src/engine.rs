//! Core policy engine: evaluates a Release's classification and novelty against
//! the versioned ruleset to produce a disposition (PRD-POLICY-001 through 008).

use std::collections::BTreeSet;

use melodee_domain::{
    Confidence, DecisionCompletionState, Disposition, EvidenceSource, PolicyActor,
    ReleasePolicyDecision, ReleasePolicyDecisionId,
};
use time::OffsetDateTime;

use crate::classifier::{ClassificationInput, Classifier};
use crate::config::PolicyConfig;
use crate::novelty::NoveltyCategory;

/// Default policy rule set identifier for Phase 5 (deterministic).
pub const DEFAULT_RULE_SET_VERSION: u32 = 1;

/// Default rule set name for traceability.
pub const DEFAULT_RULE_SET_NAME: &str = "phase5-default-rules";

/// All default policy rules as a static list for traceability.
pub fn default_rules() -> DefaultRules {
    DefaultRules {
        rules: vec![
            "POL-001: Novel or uncertain content requires non-destructive disposition (Review/Accept).",
            "POL-002: Administrator evidence cannot be auto-resolved; requires human Review.",
            "POL-003: Bootleg or Tribute with non-directory evidence → Ignore (exclude from catalog).",
            "POL-004: BrandNewRelease novelty → Review before any destructive action.",
            "POL-005: Directory-only evidence never yields destructive disposition.",
            "POL-006: Greatest Hits + fully redundant (Reissue) + high non-directory confidence → Delete (Administrator override only).",
            "POL-007: Corrupt or unreadable media → Quarantine.",
            "POL-008: Unknown classification → Review.",
        ],
    }
}

#[derive(Debug, Clone)]
pub struct DefaultRules {
    pub rules: Vec<&'static str>,
}

pub fn default_rule_set() -> DefaultPolicyRuleSet {
    DefaultPolicyRuleSet {
        id: default_rule_set_id(),
        name: DEFAULT_RULE_SET_NAME.to_string(),
        version: DEFAULT_RULE_SET_VERSION,
        rules: default_rules(),
    }
}

/// Deterministic rule set ID derived from the version (not random).
pub fn default_rule_set_id() -> melodee_domain::PolicyRuleSetId {
    melodee_domain::PolicyRuleSetId::from_uuid(uuid::Uuid::nil())
}

#[derive(Debug, Clone)]
pub struct DefaultPolicyRuleSet {
    pub id: melodee_domain::PolicyRuleSetId,
    pub name: String,
    pub version: u32,
    pub rules: DefaultRules,
}

/// A rule match contributing to evidence for a decision.
#[derive(Debug, Clone)]
pub struct RuleMatch {
    pub rule_id: String,
    pub description: String,
    pub confidence: Confidence,
}

/// Evaluated policy context for a Release.
///
/// `novelty_category` is the release-level assessment; `is_greatest_hits` and
/// `is_fully_redundant` are derived by the engine from the classification and
/// novelty category respectively — the engine does NOT trust caller-provided
/// flags for destructive-action gating.
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
    pub has_corrupt_media: bool,
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
    /// Disposition rules (ADR-0120, PRD-POLICY-001 through 008):
    /// - `Accept`: safe to keep (no destructive rules matched)
    /// - `Ignore`: Bootleg/Tribute with sufficient evidence (excluded from catalog)
    /// - `Quarantine`: corrupt media
    /// - `Review`: novel, remaster, unknown, admin evidence, or no operator override
    /// - `Delete`: Greatest Hits + fully redundant novelty + high non-directory confidence + Administrator override
    ///
    /// Only `Administrator` PolicyActor qualifies as an operator override.
    /// `System` actor never authorizes deletion.
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

        // The engine computes these from first principles — it does NOT
        // trust caller-provided flags for destructive-action gating.
        let is_greatest_hits = classification
            .traits
            .contains(&melodee_domain::ReleaseTrait::GreatestHits);
        let is_fully_redundant = matches!(input.novelty_category, NoveltyCategory::Reissue);

        let threshold = Confidence::new(self.config.destructive_confidence_bps)
            .unwrap_or_else(|_| confidence(8_000));

        // POL-001: Noveltity requires Review/Accept, never Delete without override.
        if input.novelty_category.is_novel() && !input.novelty_category.is_redundant() {
            matched_rules.push(RuleMatch {
                rule_id: "POL-001".to_string(),
                description: format!(
                    "Novelty category {:?} is novel; non-destructive disposition required",
                    input.novelty_category
                ),
                confidence: best_confidence,
            });
        }

        // POL-002: Administrator evidence requires Review.
        if has_admin_evidence {
            matched_rules.push(RuleMatch {
                rule_id: "POL-002".to_string(),
                description: "Evidence includes Administrator source; requires Review".to_string(),
                confidence: best_confidence,
            });
        }

        // POL-003: Bootleg/Tribute with non-directory evidence → Ignore.
        if has_bootleg_or_tribute && !evidence_source_is_directory_only {
            matched_rules.push(RuleMatch {
                rule_id: "POL-003".to_string(),
                description: format!(
                    "Classified as Bootleg/Tribute with confidence {}; non-directory evidence source",
                    best_confidence.basis_points()
                ),
                confidence: best_confidence,
            });
        }

        // POL-004: BrandNewRelease requires Review (Remaster is known content,
        // not novel — see NoveltyCategory::is_novel).
        if input.novelty_category == NoveltyCategory::BrandNewRelease {
            matched_rules.push(RuleMatch {
                rule_id: "POL-004".to_string(),
                description: format!(
                    "Novelty category {:?} requires manual review before destructive action",
                    input.novelty_category
                ),
                confidence: confidence(5_000),
            });
        }

        // POL-005: Directory-only evidence can never produce destructive disposition.
        if evidence_source_is_directory_only {
            matched_rules.push(RuleMatch {
                rule_id: "POL-005".to_string(),
                description: "Evidence is directory-only; destructive disposition prohibited"
                    .to_string(),
                confidence: best_confidence,
            });
        }

        // POL-006: Greatest Hits + fully redundant (Reissue) + high non-directory
        // confidence → Delete (requires Administrator override; System never deletes).
        if is_greatest_hits
            && is_fully_redundant
            && best_confidence >= threshold
            && !evidence_source_is_directory_only
        {
            matched_rules.push(RuleMatch {
                rule_id: "POL-006".to_string(),
                description: format!(
                    "Greatest Hits redundant collection, confidence {} >= threshold {}, \
                     non-directory evidence source {:?}",
                    best_confidence.basis_points(),
                    threshold.basis_points(),
                    best_source.unwrap_or(EvidenceSource::ReleaseTitle)
                ),
                confidence: best_confidence,
            });
        }

        // POL-008: Unknown classification → Review.
        if has_unknown_trait {
            matched_rules.push(RuleMatch {
                rule_id: "POL-008".to_string(),
                description: "Release classified as Unknown; deferring to manual review"
                    .to_string(),
                confidence: confidence(1_000),
            });
        }

        // POL-007: Corrupt/unreadable media → Quarantine (takes precedence).
        if input.has_corrupt_media {
            matched_rules.push(RuleMatch {
                rule_id: "POL-007".to_string(),
                description: "Release has corrupt or unreadable media; quarantining".to_string(),
                confidence: confidence(9_500),
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
            &input.actors,
        );

        let matched_rule_ids: BTreeSet<String> =
            matched_rules.iter().map(|r| r.rule_id.clone()).collect();

        // Decision is Pending — no destructive action taken until completion.
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
            completion_state: DecisionCompletionState::Pending,
            completed_at: None,
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
        actors: &[PolicyActor],
    ) -> (Disposition, String) {
        let has_admin_override = actors.iter().any(|a| a.is_administrator());

        // POL-007: Corrupt/unreadable media → Quarantine (highest precedence).
        if rule_ids.contains("POL-007") {
            return (
                Disposition::Quarantine,
                format!(
                    "Release has corrupt or unreadable media; quarantining. \
                     confidence {} basis points; rules matched: {:?}",
                    confidence.basis_points(),
                    rule_ids
                ),
            );
        }

        // POL-003: Bootleg/Tribute with non-directory evidence → Ignore.
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

        // POL-004: BrandNewRelease/Remaster → Review (unless Ignore already triggered).
        if rule_ids.contains("POL-004") {
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

        // POL-006: Greatest Hits + redundant + high confidence + directory-only check
        // → Delete (requires Administrator override; System never deletes).
        if rule_ids.contains("POL-006") {
            if !has_admin_override {
                return (
                    Disposition::Review,
                    format!(
                        "Confidence {} >= threshold {} for Greatest Hits redundant collection, \
                         but no Administrator override recorded; manual review required",
                        confidence.basis_points(),
                        threshold.basis_points()
                    ),
                );
            }
            return (
                Disposition::Delete,
                format!(
                    "Confidence {} >= threshold {} for Greatest Hits redundant collection; \
                     Administrator override present",
                    confidence.basis_points(),
                    threshold.basis_points()
                ),
            );
        }

        // POL-002: Administrator evidence → Review.
        if rule_ids.contains("POL-002") {
            return (
                Disposition::Review,
                "Administrator evidence present; requires manual confirmation".to_string(),
            );
        }

        // POL-008: Unknown classification → Review.
        if rule_ids.contains("POL-008") {
            return (
                Disposition::Review,
                "Release classification is Unknown; insufficient evidence for automated decision"
                    .to_string(),
            );
        }

        // POL-005: Directory-only evidence → Accept (cannot be destructive).
        if rule_ids.contains("POL-005") {
            return (
                Disposition::Accept,
                "Evidence is directory-only; cannot authorize destructive action".to_string(),
            );
        }

        // POL-001: Novel content → Review (never Accept, never auto-Delete).
        if rule_ids.contains("POL-001") {
            return (
                Disposition::Review,
                format!(
                    "Release contains novel or uncertain content; \
                     confidence {} basis points; rules matched: {:?}",
                    confidence.basis_points(),
                    rule_ids
                ),
            );
        }

        // Default: Accept (safe to retain).
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

/// Constructs a Confidence from a basis-point value, clamped to the valid range.
fn confidence(bp: u16) -> Confidence {
    Confidence::new(bp.min(Confidence::MAX_BASIS_POINTS)).unwrap_or(Confidence::zero())
}

/// Result of a single policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    pub classification: melodee_domain::ReleaseClassification,
    pub decision: ReleasePolicyDecision,
    pub matched_rules: Vec<RuleMatch>,
}
