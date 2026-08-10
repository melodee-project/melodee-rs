//! Phase 5 acceptance tests covering classification, novelty, and policy
//! disposition logic across the PRD-CLASS, PRD-NOV, and PRD-POL requirement
//! families. These are pure domain/policy-layer tests — no database required.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use melodee_domain::{
    AudioIdentityId, Confidence, DecisionCompletionState, Disposition, EvidenceSource, PolicyActor,
    ReleaseId, ReleaseTrait, TrackNovelty, TrackNoveltyCategory,
};
use melodee_policy::{
    Classifier, NoveltyCategory, NoveltyEngine, NoveltyInput, PolicyConfig, PolicyEngine,
    PolicyInput,
};
use time::OffsetDateTime;

const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

fn confidence(bp: u16) -> Confidence {
    Confidence::new(bp).unwrap()
}

fn make_config() -> PolicyConfig {
    PolicyConfig {
        batch_size: 100,
        concurrency: 2,
        destructive_confidence_bps: 8_000,
    }
}

fn make_input(release_id: ReleaseId, title: &str, directory: &str) -> PolicyInput {
    PolicyInput {
        release_id,
        title: Some(title.to_string()),
        directory_name: Some(directory.to_string()),
        parent_directory_name: Some("/music".to_string()),
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
        actors: Vec::new(),
        novelty_category: NoveltyCategory::Unknown,
        has_corrupt_media: false,
        now: NOW,
    }
}

fn make_input_with_traits(
    release_id: ReleaseId,
    title: &str,
    traits: Vec<ReleaseTrait>,
) -> PolicyInput {
    let mut input = make_input(release_id, title, "music");
    input.structured_tag_traits = traits.clone();
    input.existing_classification_traits = traits.into_iter().collect();
    input
}

// ============================================================================
// Classification tests (PRD-CLASS-001, 002, 003)
// ============================================================================

#[test]
fn class_001_title_pattern_identifies_greatest_hits() {
    let classifier = melodee_policy::Classifier;
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: Some("Greatest Hits".to_string()),
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    assert!(classification.traits.contains(&ReleaseTrait::GreatestHits));
    assert!(classification.traits.contains(&ReleaseTrait::Compilation));
    assert!(!classification.evidence.is_empty());
}

#[test]
fn class_002_confidence_reflects_evidence_quality() {
    let classifier = melodee_policy::Classifier;
    let input_dir_only = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: None,
        directory_name: Some("Greatest Hits".to_string()),
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input_dir_only, NOW);
    assert!(classification.traits.contains(&ReleaseTrait::GreatestHits));
    assert!(melodee_policy::Classifier::strongest_source_is_directory_only(&classification));
    assert!(
        classification.confidence.basis_points() < 8_000,
        "directory-only evidence must not reach destructive threshold"
    );
}

#[test]
fn class_003_directory_evidence_cannot_trigger_destructive_action() {
    let classifier = melodee_policy::Classifier;
    // Title-based evidence (tier 4) gives 4000 bp confidence, below 8000.
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: Some("Best Of Pop".to_string()),
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    assert!(!Classifier::strongest_source_is_directory_only(
        &classification
    ));
    assert_eq!(classification.confidence.basis_points(), 4_000);
}

#[test]
fn class_004_embedded_external_id_is_high_confidence() {
    let classifier = melodee_policy::Classifier;
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: None,
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![ReleaseTrait::Bootleg],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    assert!(classification.traits.contains(&ReleaseTrait::Bootleg));
    assert_eq!(
        classification.evidence[0].source,
        EvidenceSource::EmbeddedExternalId
    );
    assert!(classification.confidence.basis_points() >= 8_000);
}

#[test]
fn class_005_no_evidence_defaults_to_unknown() {
    let classifier = melodee_policy::Classifier;
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: Some("Some Random Title".to_string()),
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    assert!(classification.traits.contains(&ReleaseTrait::Unknown));
}

#[test]
fn class_006_structured_tags_are_medium_confidence() {
    let classifier = melodee_policy::Classifier;
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: None,
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![ReleaseTrait::Live],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    assert!(classification.traits.contains(&ReleaseTrait::Live));
    assert_eq!(
        classification.evidence[0].source,
        EvidenceSource::StructuredTags
    );
    assert!(classification.confidence.basis_points() < 8_000);
}

// ============================================================================
// Novelty tests (PRD-POLICY-004, PRD-POLICY-005)
// ============================================================================

#[test]
fn nov_001_no_prior_releases_is_brand_new() {
    let engine = NoveltyEngine;
    let input = NoveltyInput {
        release_id: ReleaseId::new(),
        track_audio: vec![],
        catalog_exact_duplicates: BTreeSet::new(),
        catalog_known_identities: BTreeSet::new(),
        track_novelty: vec![],
        evaluator_version: "v1".to_string(),
        now: NOW,
    };
    let assessment = engine.assess(&input).unwrap();
    assert_eq!(assessment.category, NoveltyCategory::Unknown);
    assert_eq!(assessment.evaluation.total_track_count, 0);
}

#[test]
fn nov_002_all_exact_duplicates_verified_in_catalog_is_reissue() {
    let engine = NoveltyEngine;
    let audio_identity_id = AudioIdentityId::new();
    let track_novelty = vec![TrackNovelty {
        id: melodee_domain::TrackNoveltyId::new(),
        release_id: ReleaseId::new(),
        track_id: melodee_domain::TrackId::new(),
        audio_identity_id,
        media_asset_id: None,
        category: TrackNoveltyCategory::ExactDuplicate,
        confidence: confidence(9_000),
        evidence_kind: melodee_domain::NoveltyEvidenceKind::ByteHashMatch,
        detail: "Exact duplicate".to_string(),
        evaluated_at: NOW,
    }];
    let input = NoveltyInput {
        release_id: ReleaseId::new(),
        track_audio: vec![],
        catalog_exact_duplicates: BTreeSet::from([audio_identity_id]),
        catalog_known_identities: BTreeSet::new(),
        track_novelty,
        evaluator_version: "v1".to_string(),
        now: NOW,
    };
    let assessment = engine.assess(&input).unwrap();
    assert_eq!(assessment.category, NoveltyCategory::Reissue);
    assert!(assessment.category.is_redundant());
}

#[test]
fn nov_003_unverified_known_audio_identity_downgraded_to_uncertain() {
    let engine = NoveltyEngine;
    let audio_identity_id = AudioIdentityId::new();
    let track_novelty = vec![TrackNovelty {
        id: melodee_domain::TrackNoveltyId::new(),
        release_id: ReleaseId::new(),
        track_id: melodee_domain::TrackId::new(),
        audio_identity_id,
        media_asset_id: None,
        category: TrackNoveltyCategory::KnownAudioIdentity,
        confidence: confidence(8_000),
        evidence_kind: melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
        detail: "Known identity, different encoding".to_string(),
        evaluated_at: NOW,
    }];
    // catalog_known_identities is empty → caller-provided KnownAudioIdentity label
    // is NOT verified → downgraded to Uncertain → Unknown.
    let input = NoveltyInput {
        release_id: ReleaseId::new(),
        track_audio: vec![],
        catalog_exact_duplicates: BTreeSet::new(),
        catalog_known_identities: BTreeSet::new(),
        track_novelty,
        evaluator_version: "v1".to_string(),
        now: NOW,
    };
    let assessment = engine.assess(&input).unwrap();
    assert_eq!(assessment.category, NoveltyCategory::Unknown);
    assert!(!assessment.category.is_novel());
    assert!(assessment.category.is_unsafe_to_auto_delete());
}

#[test]
fn nov_003b_verified_known_audio_identity_is_remaster_not_novel() {
    let engine = NoveltyEngine;
    let audio_identity_id = AudioIdentityId::new();
    let track_novelty = vec![TrackNovelty {
        id: melodee_domain::TrackNoveltyId::new(),
        release_id: ReleaseId::new(),
        track_id: melodee_domain::TrackId::new(),
        audio_identity_id,
        media_asset_id: None,
        category: TrackNoveltyCategory::KnownAudioIdentity,
        confidence: confidence(8_000),
        evidence_kind: melodee_domain::NoveltyEvidenceKind::AudioIdentityMatch,
        detail: "Known identity, different encoding".to_string(),
        evaluated_at: NOW,
    }];
    // catalog_known_identities DOES contain the identity → label is verified.
    let input = NoveltyInput {
        release_id: ReleaseId::new(),
        track_audio: vec![],
        catalog_exact_duplicates: BTreeSet::new(),
        catalog_known_identities: BTreeSet::from([audio_identity_id]),
        track_novelty,
        evaluator_version: "v1".to_string(),
        now: NOW,
    };
    let assessment = engine.assess(&input).unwrap();
    assert_eq!(assessment.category, NoveltyCategory::Remaster);
    // Remaster is known content (not novel), but unsafe to auto-delete
    // (different encoding worth keeping).
    assert!(!assessment.category.is_novel());
    assert!(!assessment.category.is_redundant());
    assert!(assessment.category.is_unsafe_to_auto_delete());
}

#[test]
fn nov_004_mixed_novelty_is_variant() {
    let engine = NoveltyEngine;
    let exact_identity = AudioIdentityId::new();
    let track_novelty = vec![
        TrackNovelty {
            id: melodee_domain::TrackNoveltyId::new(),
            release_id: ReleaseId::new(),
            track_id: melodee_domain::TrackId::new(),
            audio_identity_id: exact_identity,
            media_asset_id: None,
            category: TrackNoveltyCategory::ExactDuplicate,
            confidence: confidence(9_000),
            evidence_kind: melodee_domain::NoveltyEvidenceKind::ByteHashMatch,
            detail: "Exact dup".to_string(),
            evaluated_at: NOW,
        },
        TrackNovelty {
            id: melodee_domain::TrackNoveltyId::new(),
            release_id: ReleaseId::new(),
            track_id: melodee_domain::TrackId::new(),
            audio_identity_id: AudioIdentityId::new(),
            media_asset_id: None,
            category: TrackNoveltyCategory::NewTrack,
            confidence: confidence(9_500),
            evidence_kind: melodee_domain::NoveltyEvidenceKind::NoCatalogMatch,
            detail: "New track".to_string(),
            evaluated_at: NOW,
        },
    ];
    let input = NoveltyInput {
        release_id: ReleaseId::new(),
        track_audio: vec![],
        catalog_exact_duplicates: BTreeSet::from([exact_identity]),
        catalog_known_identities: BTreeSet::new(),
        track_novelty,
        evaluator_version: "v1".to_string(),
        now: NOW,
    };
    let assessment = engine.assess(&input).unwrap();
    assert_eq!(assessment.category, NoveltyCategory::Variant);
    assert!(assessment.category.is_novel());
    assert!(assessment.category.is_unsafe_to_auto_delete());
}

// ============================================================================
// Policy disposition tests (PRD-POLICY-001 through 006)
// ============================================================================

#[test]
fn pol_001_high_confidence_greatest_hits_reissue_yields_delete_with_administrator_override() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(
        release_id,
        "Greatest Hits",
        vec![ReleaseTrait::GreatestHits],
    );
    // Embedded external ID gives 8_500 confidence (above 8_000 threshold).
    input.embedded_external_id_traits = vec![ReleaseTrait::GreatestHits];
    // Novelty must be Reissue (fully redundant) for Delete eligibility.
    input.novelty_category = NoveltyCategory::Reissue;
    // Only Administrator actors qualify as operator overrides for Delete.
    // System actor NEVER authorizes deletion.
    input.actors.push(PolicyActor::Administrator {
        actor: "ops@test".to_string(),
    });
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Delete);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-006"));
    assert!(!eval.matched_rules.iter().any(|r| r.rule_id == "POL-004"));
}

#[test]
fn pol_001a_system_actor_does_not_authorize_delete() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(
        release_id,
        "Greatest Hits",
        vec![ReleaseTrait::GreatestHits],
    );
    input.embedded_external_id_traits = vec![ReleaseTrait::GreatestHits];
    input.novelty_category = NoveltyCategory::Reissue;
    // System actor — must NOT authorize deletion.
    input.actors.push(PolicyActor::System {
        policy_rule_set_version: 1,
    });
    let eval = engine.evaluate(&input);
    // System actor — must NOT authorize deletion. POL-006 conditions may be
    // met, but without Administrator override the disposition is Review.
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-006"));
}

#[test]
fn pol_001b_greatest_hits_with_variant_novelty_does_not_delete() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(
        release_id,
        "Greatest Hits",
        vec![ReleaseTrait::GreatestHits],
    );
    input.embedded_external_id_traits = vec![ReleaseTrait::GreatestHits];
    // Variant novelty (has novel tracks) → not fully redundant → Review, not Delete.
    input.novelty_category = NoveltyCategory::Variant;
    input.actors.push(PolicyActor::Administrator {
        actor: "ops@test".to_string(),
    });
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(!eval.matched_rules.iter().any(|r| r.rule_id == "POL-006"));
}

#[test]
fn pol_001_requires_operator_override_without_actor() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(
        release_id,
        "Greatest Hits",
        vec![ReleaseTrait::GreatestHits],
    );
    input.embedded_external_id_traits = vec![ReleaseTrait::GreatestHits];
    input.novelty_category = NoveltyCategory::Reissue;
    let eval = engine.evaluate(&input);
    // No operator actor → Review instead of Delete.
    assert_eq!(eval.decision.disposition, Disposition::Review);
}

#[test]
fn pol_002_admin_evidence_triggers_review() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "Studio Album", "music");
    let eval = engine.evaluate(&input);
    // Unknown classification → Review (POL-008).
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-008"));
}

#[test]
fn pol_003_bootleg_with_sufficient_evidence_yields_ignore() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(release_id, "Bootleg Recording", vec![]);
    input.embedded_external_id_traits = vec![ReleaseTrait::Bootleg];
    input.actors.push(PolicyActor::System {
        policy_rule_set_version: 1,
    });
    let eval = engine.evaluate(&input);
    // Bootleg (POL-003) takes precedence over novelty Review (POL-004).
    assert_eq!(eval.decision.disposition, Disposition::Ignore);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-003"));
}

#[test]
fn pol_004_brand_new_release_requires_manual_review() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "A Great Title", "some_directory");
    let eval = engine.evaluate(&input);
    // Unknown classification → Review (POL-008), not POL-004 because novelty is Unknown.
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-008"));
}

#[test]
fn pol_004b_brand_new_release_novelty_requires_review() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input(release_id, "Brand New Album", "new_album_dir");
    input.novelty_category = NoveltyCategory::BrandNewRelease;
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-004"));
}

#[test]
fn pol_005_directory_only_evidence_never_produces_destructive_disposition() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    // Directory-name only (lower confidence, tier 7).
    let input = PolicyInput {
        release_id,
        title: None,
        directory_name: Some("Greatest Hits Collection".to_string()),
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
        actors: Vec::new(),
        novelty_category: NoveltyCategory::Unknown,
        has_corrupt_media: false,
        now: NOW,
    };
    let eval = engine.evaluate(&input);
    // Should not be Delete (directory evidence can't reach 8000 bp).
    assert_ne!(eval.decision.disposition, Disposition::Delete);
}

#[test]
fn pol_006_unknown_classification_defers_to_review() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "Mysterious Title", "mysterious_dir");
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-008"));
}

#[test]
fn pol_007_durable_decision_recorded_with_correct_metadata() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "Some Title", "some_dir");
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.release_id, release_id);
    assert_eq!(eval.decision.rule_set_id, engine.rule_set_id());
    assert_eq!(eval.decision.rule_set_version, engine.rule_set_version());
    assert_eq!(
        eval.decision.completion_state,
        DecisionCompletionState::Pending
    );
    assert!(eval.decision.completed_at.is_none());
    assert!(!eval.decision.matched_rule_ids.is_empty());
}

#[test]
fn pol_008_remaster_novelty_does_not_require_review_for_novelty() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input(release_id, "Remastered Album", "remaster_dir");
    input.novelty_category = NoveltyCategory::Remaster;
    let eval = engine.evaluate(&input);
    // Remaster is NOT novel (known content in different encoding).
    // Title "Remastered Album" yields Remaster trait (not Unknown), novelty is
    // Remaster (not BrandNewRelease). No destructive rules matched → Accept.
    assert_eq!(eval.decision.disposition, Disposition::Accept);
    assert!(!eval.matched_rules.iter().any(|r| r.rule_id == "POL-004"));
    assert!(!eval.matched_rules.iter().any(|r| r.rule_id == "POL-001"));
}

#[test]
fn pol_009_no_traits_yields_review_for_unknown() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "Random Album Name", "random_dir");
    let eval = engine.evaluate(&input);
    // No recognizable traits → Unknown → Review (POL-008).
    assert_eq!(eval.decision.disposition, Disposition::Review);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-008"));
}

#[test]
fn pol_007_corrupt_media_yields_quarantine() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input(release_id, "Some Title", "some_dir");
    input.has_corrupt_media = true;
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Quarantine);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-007"));
    // POL-007 takes precedence even with Greatest Hits + Administrator override.
    assert!(!eval.matched_rules.iter().any(|r| r.rule_id == "POL-006"));
}

#[test]
fn pol_007_corrupt_media_quarantine_precedence_with_greatest_hits() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let mut input = make_input_with_traits(
        release_id,
        "Greatest Hits",
        vec![ReleaseTrait::GreatestHits],
    );
    input.embedded_external_id_traits = vec![ReleaseTrait::GreatestHits];
    input.novelty_category = NoveltyCategory::Reissue;
    input.has_corrupt_media = true;
    // Even with Administrator override and Greatest Hits + Reissue,
    // corrupt media takes precedence → Quarantine, not Delete.
    input.actors.push(PolicyActor::Administrator {
        actor: "ops@test".to_string(),
    });
    let eval = engine.evaluate(&input);
    assert_eq!(eval.decision.disposition, Disposition::Quarantine);
    assert!(eval.matched_rules.iter().any(|r| r.rule_id == "POL-007"));
}

#[test]
fn pol_010_dry_run_produces_stats_without_database() {
    let config = make_config();
    let engine = PolicyEngine::new(config);
    let release_id = ReleaseId::new();
    let input = make_input(release_id, "Greatest Hits", "greatest_hits");
    let inputs = vec![
        input.clone(),
        make_input(release_id, "Greatest Hits 2", "greatest_hits_2"),
    ];
    let runner = melodee_policy::PolicyDryRunner::new(engine);
    let result = runner.run_dry_run(inputs, NOW);
    assert_eq!(result.stats.evaluated, 2);
    assert_eq!(result.stats.decisions, 2);
    assert_eq!(result.stats.errors, 0);
    assert!(result.stats.duration_ms < 60_000);
}

// ============================================================================
// Evidence hierarchy tests (PRD-CLASS-003)
// ============================================================================
#[test]
fn class_007_evidence_source_trust_hierarchy_ordering() {
    assert!(
        EvidenceSource::Administrator.trust_rank()
            < EvidenceSource::ParentDirectoryName.trust_rank()
    );
    assert!(EvidenceSource::DirectoryName.is_directory_only());
    assert!(EvidenceSource::ParentDirectoryName.is_directory_only());
    assert!(!EvidenceSource::ReleaseTitle.is_directory_only());
    assert!(!EvidenceSource::Administrator.is_directory_only());
    assert!(!EvidenceSource::EmbeddedExternalId.is_directory_only());
    assert!(!EvidenceSource::StructuredTags.is_directory_only());
    assert!(!EvidenceSource::CatalogContext.is_directory_only());
}

#[test]
fn class_008_embedded_external_id_trumps_title() {
    let classifier = melodee_policy::Classifier;
    let input = melodee_policy::ClassificationInput {
        release_id: ReleaseId::new(),
        title: Some("Greatest Hits".to_string()),
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![],
        embedded_external_id_traits: vec![ReleaseTrait::Studio],
        catalog_known_release_traits: vec![],
        existing_classification_traits: BTreeSet::new(),
    };
    let classification = classifier.classify(&input, NOW);
    // EmbeddedExternalId (trust_rank 1) should be the strongest evidence.
    let best = classification
        .evidence
        .iter()
        .min_by_key(|e| e.source.trust_rank());
    assert_eq!(
        best.expect("classification has evidence").source,
        EvidenceSource::EmbeddedExternalId
    );
}
