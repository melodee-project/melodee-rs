//! Adversarial Phase 5 acceptance tests for PRD-CLASS-001–003 and
//! PRD-POLICY-001–008.

#![allow(clippy::expect_used)]

use std::collections::{BTreeSet, HashSet};

use melodee_domain::{
    AdministratorOverride, Confidence, DecisionCompletionState, Disposition, DomainInvariantError,
    MAX_POLICY_TEST_MATCHED_RULES, MAX_POLICY_TEST_RELEASES, MAX_POLICY_TEST_SAMPLES,
    MAX_RELEASE_CLASSIFICATION_EVIDENCE, MAX_RELEASE_CLASSIFICATION_TRAITS, MediaAssetId,
    PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleType, PolicyTestRunId, PolicyTestSummary,
    ReleaseClassification, ReleaseId, ReleasePolicyDecision, ReleasePolicyDecisionId, ReleaseTrait,
};
use melodee_policy::{
    AdministratorClassification, ClassificationInput, Classifier,
    DEFAULT_NOVELTY_EVALUATOR_VERSION, MAX_DRY_RUN_CLASSIFICATION_FACTS, MAX_DRY_RUN_TRACK_FACTS,
    MAX_TRACKS_PER_NOVELTY_EVALUATION, NoveltyCategory, NoveltyEngine, NoveltyEngineError,
    NoveltyInput, NoveltyTrackFact, PolicyConfig, PolicyDryRunError, PolicyDryRunner, PolicyEngine,
    PolicyEngineError, PolicyInput, default_rule_set_id,
};
use serde_json::json;
use time::OffsetDateTime;

const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

#[test]
fn policy_actor_json_is_stable_tagged_and_round_trips() {
    for (actor, expected) in [
        (
            PolicyActor::System {
                policy_rule_set_version: 7,
            },
            json!({"authority": "system", "policy_rule_set_version": 7}),
        ),
        (
            PolicyActor::Administrator {
                actor: "admin@example.test".to_string(),
            },
            json!({"authority": "administrator", "actor": "admin@example.test"}),
        ),
    ] {
        let encoded = serde_json::to_value(&actor).expect("serialize actor");
        assert_eq!(encoded, expected);
        assert_eq!(
            serde_json::from_value::<PolicyActor>(encoded).expect("deserialize actor"),
            actor
        );
    }
}

fn hash(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn config() -> PolicyConfig {
    PolicyConfig {
        batch_size: 100,
        concurrency: 2,
        ..PolicyConfig::default()
    }
}

fn classify(
    release_id: ReleaseId,
    title: Option<&str>,
    structured: Vec<ReleaseTrait>,
    embedded: Vec<ReleaseTrait>,
) -> ReleaseClassification {
    Classifier
        .classify(
            &ClassificationInput {
                release_id,
                title: title.map(str::to_string),
                directory_name: None,
                parent_directory_name: None,
                structured_tag_traits: structured,
                embedded_external_id_traits: embedded,
                catalog_known_release_traits: Vec::new(),
                administrator_traits: Vec::new(),
            },
            NOW,
        )
        .expect("classification should be valid")
}

fn fact(
    release_id: ReleaseId,
    audio_identity_id: melodee_domain::AudioIdentityId,
    byte_hash: Option<String>,
    confirmed: bool,
) -> NoveltyTrackFact {
    NoveltyTrackFact {
        release_id,
        track_id: melodee_domain::TrackId::new(),
        audio_identity_id,
        media_asset_id: Some(MediaAssetId::new()),
        byte_hash,
        audio_identity_confirmed: confirmed,
    }
}

fn novelty_input(
    engine: &PolicyEngine,
    release_id: ReleaseId,
    tracks: Vec<NoveltyTrackFact>,
) -> NoveltyInput {
    NoveltyInput {
        release_id,
        tracks,
        catalog_exact_byte_hashes: BTreeSet::new(),
        catalog_known_audio_identities: BTreeSet::new(),
        catalog_alternate_audio_identities: BTreeSet::new(),
        policy_rule_set_id: engine.rule_set_id(),
        evaluator_version: "acceptance-v1".to_string(),
        now: NOW,
    }
}

fn policy_input(
    classification: ReleaseClassification,
    novelty: melodee_policy::NoveltyAssessment,
) -> PolicyInput {
    PolicyInput {
        classification,
        novelty,
        existing_disposition: None,
        source_fingerprint: hash('a'),
        has_corrupt_media: false,
        administrator_override: None,
        now: NOW,
    }
}

fn classification_with_evidence_count(
    release_id: ReleaseId,
    count: usize,
) -> ReleaseClassification {
    let mut classification = classify(release_id, None, vec![ReleaseTrait::Studio], Vec::new());
    let template = classification.evidence[0].clone();
    classification.evidence = (0..count)
        .map(|index| melodee_domain::ClassificationEvidence {
            detail: format!("{index}: bounded structured evidence"),
            ..template.clone()
        })
        .collect();
    ReleaseClassification::new(classification).expect("bounded classification")
}

#[test]
fn classification_is_multivalued_and_every_trait_has_scoped_evidence() {
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Greatest Hits Live Box Set"),
        vec![ReleaseTrait::Remaster],
        Vec::new(),
    );
    for trait_ in [
        ReleaseTrait::GreatestHits,
        ReleaseTrait::Compilation,
        ReleaseTrait::Live,
        ReleaseTrait::BoxSet,
        ReleaseTrait::Remaster,
    ] {
        assert!(classification.has_trait(trait_));
        assert!(classification.evidence_for(trait_).next().is_some());
    }
    assert!(
        classification
            .evidence
            .iter()
            .all(|evidence| evidence.id == classification.id && evidence.release_id == release_id)
    );
    ReleaseClassification::new(classification).expect("classifier output must satisfy domain");
}

#[test]
fn classification_rejects_unbacked_traits_and_foreign_release_evidence() {
    let mut classification = classify(ReleaseId::new(), Some("Live"), Vec::new(), Vec::new());
    classification.traits.insert(ReleaseTrait::Studio);
    assert_eq!(
        ReleaseClassification::new(classification),
        Err(DomainInvariantError::IncompleteClassificationEvidence)
    );
}

#[test]
fn classification_scope_caps_match_persistence_and_fail_at_cap_plus_one() {
    assert_eq!(MAX_RELEASE_CLASSIFICATION_TRAITS, ReleaseTrait::ALL.len());
    classification_with_evidence_count(ReleaseId::new(), MAX_RELEASE_CLASSIFICATION_EVIDENCE);

    let mut over_limit = classification_with_evidence_count(ReleaseId::new(), 1);
    let template = over_limit.evidence[0].clone();
    over_limit.evidence = (0..=MAX_RELEASE_CLASSIFICATION_EVIDENCE)
        .map(|index| melodee_domain::ClassificationEvidence {
            detail: format!("{index}: over-limit evidence"),
            ..template.clone()
        })
        .collect();
    assert_eq!(
        ReleaseClassification::new(over_limit),
        Err(DomainInvariantError::ClassificationScopeExceeded {
            dimension: "evidence rows",
            limit: MAX_RELEASE_CLASSIFICATION_EVIDENCE,
        })
    );
}

#[test]
fn title_classifier_avoids_loose_greatest_hits_and_maps_remix_correctly() {
    let greatest = classify(
        ReleaseId::new(),
        Some("The Greatest Adventure"),
        Vec::new(),
        Vec::new(),
    );
    assert!(!greatest.has_trait(ReleaseTrait::GreatestHits));

    let hits = classify(
        ReleaseId::new(),
        Some("When the Beat Hits"),
        Vec::new(),
        Vec::new(),
    );
    assert!(!hits.has_trait(ReleaseTrait::GreatestHits));

    let remix = classify(
        ReleaseId::new(),
        Some("Midnight Remix"),
        Vec::new(),
        Vec::new(),
    );
    assert!(remix.has_trait(ReleaseTrait::Remix));
    assert!(!remix.has_trait(ReleaseTrait::Remaster));
}

#[test]
fn title_classifier_uses_word_and_phrase_boundaries() {
    for title in [
        "Discover",
        "Boston",
        "deliver",
        "democracy",
        "promote",
        "castaway",
        "Radiohead",
    ] {
        let classification = classify(ReleaseId::new(), Some(title), Vec::new(), Vec::new());
        assert_eq!(
            classification.traits,
            BTreeSet::from([ReleaseTrait::Unknown]),
            "unexpected trait inferred for {title}"
        );
    }

    for (title, expected) in [
        ("Greatest-Hits!", ReleaseTrait::GreatestHits),
        ("Collections, Vol. 2", ReleaseTrait::GreatestHits),
        ("Covers!", ReleaseTrait::Tribute),
        ("Demos (1999)", ReleaseTrait::Demo),
        ("Radio: Show", ReleaseTrait::RadioShow),
        ("Remixes", ReleaseTrait::Remix),
    ] {
        assert!(
            classify(ReleaseId::new(), Some(title), Vec::new(), Vec::new()).has_trait(expected),
            "expected {expected:?} for {title}"
        );
    }
}

#[test]
fn repeated_identical_classification_signals_are_deduplicated() {
    let release_id = ReleaseId::new();
    let manual = AdministratorClassification {
        trait_: ReleaseTrait::Live,
        actor: "curator@example.test".to_string(),
        reason: "Verified".to_string(),
    };
    let classification = Classifier
        .classify(
            &ClassificationInput {
                release_id,
                title: None,
                directory_name: None,
                parent_directory_name: None,
                structured_tag_traits: vec![ReleaseTrait::Live, ReleaseTrait::Live],
                embedded_external_id_traits: vec![ReleaseTrait::Studio, ReleaseTrait::Studio],
                catalog_known_release_traits: vec![ReleaseTrait::Compilation; 2],
                administrator_traits: vec![manual.clone(), manual],
            },
            NOW,
        )
        .expect("deduplicated classification");
    assert_eq!(classification.evidence.len(), 4);

    let mut invalid = classification;
    invalid.evidence.push(invalid.evidence[0].clone());
    assert_eq!(
        ReleaseClassification::new(invalid),
        Err(DomainInvariantError::DuplicateClassificationEvidence)
    );
}

#[test]
fn administrator_classification_is_explicit_highest_tier_evidence() {
    let release_id = ReleaseId::new();
    let classification = Classifier
        .classify(
            &ClassificationInput {
                release_id,
                title: Some("Studio Album".to_string()),
                directory_name: None,
                parent_directory_name: None,
                structured_tag_traits: Vec::new(),
                embedded_external_id_traits: Vec::new(),
                catalog_known_release_traits: Vec::new(),
                administrator_traits: vec![AdministratorClassification {
                    trait_: ReleaseTrait::Live,
                    actor: "curator@example.test".to_string(),
                    reason: "Verified against the recording notes".to_string(),
                }],
            },
            NOW,
        )
        .expect("manual classification");
    let evidence = classification
        .evidence_for(ReleaseTrait::Live)
        .next()
        .expect("manual evidence");
    assert_eq!(
        evidence.source,
        melodee_domain::EvidenceSource::Administrator
    );
    assert_eq!(evidence.confidence, Confidence::new(10_000).expect("valid"));
}

#[test]
fn novelty_rejects_foreign_and_duplicate_track_facts() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let mut foreign = novelty_input(
        &engine,
        release_id,
        vec![fact(
            ReleaseId::new(),
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    assert!(matches!(
        NoveltyEngine.assess(&foreign),
        Err(NoveltyEngineError::ForeignTrack { .. })
    ));

    let repeated = fact(
        release_id,
        melodee_domain::AudioIdentityId::new(),
        Some(hash('b')),
        true,
    );
    foreign.tracks = vec![repeated.clone(), repeated];
    assert!(matches!(
        NoveltyEngine.assess(&foreign),
        Err(NoveltyEngineError::DuplicateTrack(_))
    ));
}

#[test]
fn novelty_rejects_over_limit_before_materializing_results() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let repeated = fact(
        release_id,
        melodee_domain::AudioIdentityId::new(),
        Some(hash('b')),
        true,
    );
    let input = novelty_input(
        &engine,
        release_id,
        vec![repeated; MAX_TRACKS_PER_NOVELTY_EVALUATION + 1],
    );
    assert!(matches!(
        NoveltyEngine.assess(&input),
        Err(NoveltyEngineError::TooManyTracks { .. })
    ));
}

#[test]
fn novelty_catalog_fact_sets_each_fail_at_cap_plus_one() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let base = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );

    let mut exact = base.clone();
    exact.catalog_exact_byte_hashes = (0..=MAX_TRACKS_PER_NOVELTY_EVALUATION)
        .map(|index| format!("{index:064x}"))
        .collect();
    assert!(matches!(
        NoveltyEngine.assess(&exact),
        Err(NoveltyEngineError::TooManyCatalogFacts {
            dimension: "exact byte hashes",
            ..
        })
    ));

    let identities = || {
        (0..=MAX_TRACKS_PER_NOVELTY_EVALUATION)
            .map(|_| melodee_domain::AudioIdentityId::new())
            .collect::<BTreeSet<_>>()
    };
    let mut known = base.clone();
    known.catalog_known_audio_identities = identities();
    assert!(matches!(
        NoveltyEngine.assess(&known),
        Err(NoveltyEngineError::TooManyCatalogFacts {
            dimension: "known AudioIdentities",
            ..
        })
    ));
    let mut alternate = base;
    alternate.catalog_alternate_audio_identities = identities();
    assert!(matches!(
        NoveltyEngine.assess(&alternate),
        Err(NoveltyEngineError::TooManyCatalogFacts {
            dimension: "alternate AudioIdentities",
            ..
        })
    ));
}

#[test]
fn exact_duplicate_is_derived_from_bytes_not_audio_identity_membership() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let audio_identity_id = melodee_domain::AudioIdentityId::new();
    let mut input = novelty_input(
        &engine,
        release_id,
        vec![fact(release_id, audio_identity_id, Some(hash('b')), false)],
    );
    input.catalog_exact_byte_hashes.insert(hash('c'));
    let assessment = NoveltyEngine.assess(&input).expect("assessment");
    assert_eq!(assessment.evaluation().uncertain_count, 1);
    assert_eq!(assessment.evaluation().exact_duplicate_count, 0);

    input.catalog_exact_byte_hashes.insert(hash('b'));
    let assessment = NoveltyEngine.assess(&input).expect("assessment");
    assert_eq!(assessment.evaluation().exact_duplicate_count, 1);
    assert_eq!(assessment.category(), NoveltyCategory::Reissue);
}

#[test]
fn exact_and_known_audio_identity_are_both_redundant() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let known_id = melodee_domain::AudioIdentityId::new();
    let mut input = novelty_input(
        &engine,
        release_id,
        vec![
            fact(
                release_id,
                melodee_domain::AudioIdentityId::new(),
                Some(hash('b')),
                true,
            ),
            fact(release_id, known_id, Some(hash('c')), true),
        ],
    );
    input.catalog_exact_byte_hashes.insert(hash('b'));
    input.catalog_known_audio_identities.insert(known_id);
    let assessment = NoveltyEngine.assess(&input).expect("assessment");
    assert_eq!(assessment.evaluation().redundant_track_count(), 2);
    assert!(assessment.evaluation().is_redundant());
    assert_eq!(assessment.category(), NoveltyCategory::Reissue);
}

#[test]
fn all_new_tracks_reach_brand_new_release() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let input = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('d')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&input).expect("assessment");
    assert_eq!(assessment.category(), NoveltyCategory::BrandNewRelease);
    assert_eq!(assessment.evaluation().new_track_count, 1);
}

#[test]
fn greatest_hits_delete_cannot_borrow_confidence_from_studio_evidence() {
    let mut configured = config();
    configured.rule_set_version = 2;
    configured.redundant_greatest_hits_disposition = Disposition::Delete;
    let engine = PolicyEngine::new(configured).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Greatest Hits"),
        Vec::new(),
        vec![ReleaseTrait::Studio],
    );
    assert_eq!(
        classification
            .strongest_non_directory_confidence_for(ReleaseTrait::GreatestHits)
            .expect("title evidence")
            .basis_points(),
        4_000
    );
    let mut novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    novelty.catalog_exact_byte_hashes.insert(hash('b'));
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let evaluation = engine
        .evaluate(&policy_input(classification, assessment))
        .expect("policy evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Accept);
    assert!(!evaluation.decision.disposition.is_destructive());
}

#[test]
fn default_redundant_greatest_hits_preference_is_safe_ignore() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(release_id, Some("Greatest Hits"), Vec::new(), Vec::new());
    let known_id = melodee_domain::AudioIdentityId::new();
    let mut novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(release_id, known_id, Some(hash('b')), true)],
    );
    novelty.catalog_known_audio_identities.insert(known_id);
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let evaluation = engine
        .evaluate(&policy_input(classification, assessment))
        .expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Ignore);
    assert_eq!(
        evaluation.decision.actor,
        PolicyActor::System {
            policy_rule_set_version: 1,
        }
    );
}

#[test]
fn directory_only_greatest_hits_evidence_cannot_trigger_redundant_rule() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification = Classifier
        .classify(
            &ClassificationInput {
                release_id,
                title: Some("Ordinary Release".to_string()),
                directory_name: Some("Greatest Hits".to_string()),
                parent_directory_name: None,
                structured_tag_traits: Vec::new(),
                embedded_external_id_traits: Vec::new(),
                catalog_known_release_traits: Vec::new(),
                administrator_traits: Vec::new(),
            },
            NOW,
        )
        .expect("classification");
    let identity = melodee_domain::AudioIdentityId::new();
    let mut novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(release_id, identity, Some(hash('b')), true)],
    );
    novelty.catalog_known_audio_identities.insert(identity);
    let evaluation = engine
        .evaluate(&policy_input(
            classification,
            NoveltyEngine.assess(&novelty).expect("novelty"),
        ))
        .expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Accept);
}

#[test]
fn new_alternate_and_uncertain_greatest_hits_never_take_redundant_outcome() {
    for (confirmed, alternate, expected) in [
        (true, false, Disposition::Review),
        (true, true, Disposition::Review),
        (false, false, Disposition::Review),
    ] {
        let engine = PolicyEngine::new(config()).expect("engine");
        let release_id = ReleaseId::new();
        let classification = classify(
            release_id,
            Some("Greatest Hits"),
            Vec::new(),
            vec![ReleaseTrait::GreatestHits],
        );
        let identity = melodee_domain::AudioIdentityId::new();
        let mut novelty = novelty_input(
            &engine,
            release_id,
            vec![fact(release_id, identity, Some(hash('b')), confirmed)],
        );
        if alternate {
            novelty.catalog_alternate_audio_identities.insert(identity);
        }
        let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
        let evaluation = engine
            .evaluate(&policy_input(classification, assessment))
            .expect("evaluation");
        assert_eq!(evaluation.decision.disposition, expected);
        assert_ne!(evaluation.decision.disposition, Disposition::Delete);
    }
}

#[test]
fn explicit_administrator_override_uses_chosen_outcome_and_exact_actor() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Ordinary Studio Release"),
        vec![ReleaseTrait::Studio],
        Vec::new(),
    );
    let novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let mut input = policy_input(classification, assessment);
    input.administrator_override = Some(AdministratorOverride {
        actor: "curator@example.test".to_string(),
        disposition: Disposition::Quarantine,
        reason: "Rights ownership requires review".to_string(),
    });
    let evaluation = engine.evaluate(&input).expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Quarantine);
    assert_eq!(
        evaluation.decision.actor,
        PolicyActor::Administrator {
            actor: "curator@example.test".to_string()
        }
    );
    assert!(
        evaluation
            .decision
            .evidence_summary
            .contains("Rights ownership requires review")
    );
}

#[test]
fn fact_orchestration_returns_the_exact_novelty_rows_used_by_policy() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification_input = ClassificationInput {
        release_id,
        title: Some("Ordinary Studio Release".to_string()),
        directory_name: None,
        parent_directory_name: None,
        structured_tag_traits: vec![ReleaseTrait::Studio],
        embedded_external_id_traits: Vec::new(),
        catalog_known_release_traits: Vec::new(),
        administrator_traits: Vec::new(),
    };
    let track = fact(
        release_id,
        melodee_domain::AudioIdentityId::new(),
        Some(hash('b')),
        true,
    );
    let novelty_input = NoveltyInput {
        release_id,
        tracks: vec![track.clone()],
        catalog_exact_byte_hashes: BTreeSet::new(),
        catalog_known_audio_identities: BTreeSet::new(),
        catalog_alternate_audio_identities: BTreeSet::new(),
        policy_rule_set_id: engine.rule_set_id(),
        evaluator_version: DEFAULT_NOVELTY_EVALUATOR_VERSION.to_string(),
        now: NOW,
    };
    let evaluation = engine
        .evaluate_facts(
            &classification_input,
            &novelty_input,
            None,
            hash('a'),
            false,
            None,
            NOW,
        )
        .expect("orchestrated evaluation");
    assert_eq!(evaluation.classification.release_id, release_id);
    assert_eq!(evaluation.novelty.evaluation().release_id, release_id);
    assert_eq!(evaluation.novelty.track_novelty().len(), 1);
    assert_eq!(
        evaluation.novelty.track_novelty()[0].track_id,
        track.track_id
    );
    assert_eq!(evaluation.decision.release_id, release_id);
}

#[test]
fn non_greatest_hits_traits_have_no_guessed_default() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Bootleg Recording"),
        Vec::new(),
        vec![ReleaseTrait::Bootleg],
    );
    let novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let evaluation = engine
        .evaluate(&policy_input(classification, assessment))
        .expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Accept);
}

#[test]
fn rule_set_is_stable_uuid_v7_and_version_changes_identity() {
    let first = default_rule_set_id(1);
    assert!(first.is_uuid_v7());
    assert_eq!(first, default_rule_set_id(1));
    assert_ne!(first, default_rule_set_id(2));
    assert_eq!(first.to_string(), "01989f00-7000-7000-8000-000500000001");
}

#[test]
fn configured_rules_are_evaluated_in_persisted_order() {
    let mut configured = config();
    configured.rule_set_version = 7;
    let rules = vec![
        PolicyRule {
            id: "ADMIN".to_string(),
            rule_type: PolicyRuleType::AdministratorOverride,
            predicate_json: json!({}),
            disposition: Disposition::Review,
            confidence_threshold: None,
        },
        PolicyRule {
            id: "CORRUPT".to_string(),
            rule_type: PolicyRuleType::CorruptMedia,
            predicate_json: json!({}),
            disposition: Disposition::Quarantine,
            confidence_threshold: None,
        },
        PolicyRule {
            id: "LIVE-QUARANTINE".to_string(),
            rule_type: PolicyRuleType::TraitDisposition,
            predicate_json: json!({"required_traits": ["live"]}),
            disposition: Disposition::Quarantine,
            confidence_threshold: None,
        },
        PolicyRule {
            id: "LIVE-IGNORE".to_string(),
            rule_type: PolicyRuleType::TraitDisposition,
            predicate_json: json!({"required_traits": ["live"]}),
            disposition: Disposition::Ignore,
            confidence_threshold: None,
        },
    ];
    let rule_set = PolicyRuleSet {
        id: default_rule_set_id(7),
        name: "phase5-default-rules".to_string(),
        version: 7,
        rules,
        created_at: NOW,
        created_by: PolicyActor::System {
            policy_rule_set_version: 7,
        },
    };
    let engine = PolicyEngine::with_rule_set(configured, rule_set).expect("custom engine");
    let release_id = ReleaseId::new();
    let classification = classify(release_id, Some("Live at Home"), Vec::new(), Vec::new());
    let novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let evaluation = engine
        .evaluate(&policy_input(classification, assessment))
        .expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Quarantine);
    assert_eq!(
        evaluation.decision.matched_rule_ids,
        vec!["LIVE-QUARANTINE"]
    );
}

#[test]
fn configured_trait_threshold_cannot_borrow_unrelated_confidence() {
    let mut configured = config();
    configured.rule_set_version = 8;
    let rules = vec![
        PolicyRule {
            id: "ADMIN".to_string(),
            rule_type: PolicyRuleType::AdministratorOverride,
            predicate_json: json!({}),
            disposition: Disposition::Review,
            confidence_threshold: None,
        },
        PolicyRule {
            id: "CORRUPT".to_string(),
            rule_type: PolicyRuleType::CorruptMedia,
            predicate_json: json!({}),
            disposition: Disposition::Quarantine,
            confidence_threshold: None,
        },
        PolicyRule {
            id: "HIGH-CONFIDENCE-LIVE".to_string(),
            rule_type: PolicyRuleType::TraitDisposition,
            predicate_json: json!({"required_traits": ["live"]}),
            disposition: Disposition::Review,
            confidence_threshold: Some(Confidence::new(8_000).expect("confidence")),
        },
    ];
    let engine = PolicyEngine::with_rule_set(
        configured,
        PolicyRuleSet {
            id: default_rule_set_id(8),
            name: "scoped-confidence".to_string(),
            version: 8,
            rules,
            created_at: NOW,
            created_by: PolicyActor::System {
                policy_rule_set_version: 8,
            },
        },
    )
    .expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Live at Home"),
        Vec::new(),
        vec![ReleaseTrait::Studio],
    );
    assert_eq!(
        classification
            .strongest_non_directory_confidence_for(ReleaseTrait::Live)
            .expect("title evidence")
            .basis_points(),
        4_000
    );
    assert_eq!(classification.confidence.basis_points(), 8_500);
    let novelty = NoveltyEngine
        .assess(&novelty_input(
            &engine,
            release_id,
            vec![fact(
                release_id,
                melodee_domain::AudioIdentityId::new(),
                Some(hash('b')),
                true,
            )],
        ))
        .expect("novelty");
    let evaluation = engine
        .evaluate(&policy_input(classification, novelty))
        .expect("evaluation");
    assert_eq!(evaluation.decision.disposition, Disposition::Accept);
    assert!(evaluation.matched_rules.is_empty());
}

#[test]
fn executable_rule_sets_fail_closed_on_safety_and_version_gaps() {
    let admin = PolicyRule {
        id: "ADMIN".to_string(),
        rule_type: PolicyRuleType::AdministratorOverride,
        predicate_json: json!({}),
        disposition: Disposition::Review,
        confidence_threshold: None,
    };
    let corrupt = PolicyRule {
        id: "CORRUPT".to_string(),
        rule_type: PolicyRuleType::CorruptMedia,
        predicate_json: json!({}),
        disposition: Disposition::Quarantine,
        confidence_threshold: None,
    };
    let make = |rules, version| PolicyRuleSet {
        id: default_rule_set_id(version),
        name: "custom".to_string(),
        version,
        rules,
        created_at: NOW,
        created_by: PolicyActor::System {
            policy_rule_set_version: version,
        },
    };

    assert!(matches!(
        PolicyEngine::with_rule_set(config(), make(vec![admin.clone()], 1)),
        Err(PolicyEngineError::MissingCorruptMediaSafetyRule)
    ));
    let mut filtered_admin = admin.clone();
    filtered_admin.predicate_json = json!({"required_traits": ["live"]});
    assert!(matches!(
        PolicyEngine::with_rule_set(config(), make(vec![filtered_admin, corrupt.clone()], 1)),
        Err(PolicyEngineError::InvalidAdministratorOverridePredicate)
    ));
    let empty_trait = PolicyRule {
        id: "MATCH-ALL".to_string(),
        rule_type: PolicyRuleType::TraitDisposition,
        predicate_json: json!({}),
        disposition: Disposition::Accept,
        confidence_threshold: None,
    };
    assert!(matches!(
        PolicyEngine::with_rule_set(
            config(),
            make(vec![admin.clone(), corrupt.clone(), empty_trait], 1)
        ),
        Err(PolicyEngineError::EmptyTraitPredicate { .. })
    ));
    assert!(matches!(
        PolicyEngine::with_rule_set(config(), make(vec![admin, corrupt], 2)),
        Err(PolicyEngineError::ConfiguredRuleSetVersionMismatch { .. })
    ));
}

#[test]
fn policy_config_rejects_zero_bounds_and_unsafe_preferences() {
    let mut invalid = config();
    invalid.batch_size = 0;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.concurrency = 0;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.destructive_confidence_bps = 0;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.rule_set_version = 0;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.redundant_greatest_hits_disposition = Disposition::Accept;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.novel_greatest_hits_disposition = Disposition::Delete;
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.uncertain_greatest_hits_disposition = Disposition::Delete;
    assert!(invalid.validate().is_err());
}

#[test]
fn decision_requires_sealed_fingerprint_and_starts_pending() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Studio Release"),
        vec![ReleaseTrait::Studio],
        Vec::new(),
    );
    let novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let mut input = policy_input(classification, assessment);
    input.source_fingerprint = "NOT-A-HASH".to_string();
    assert!(matches!(
        engine.evaluate(&input),
        Err(melodee_policy::PolicyEngineError::Domain(
            DomainInvariantError::MissingPolicySourceFingerprint
        ))
    ));

    input.source_fingerprint = hash('f');
    let evaluation = engine.evaluate(&input).expect("evaluation");
    assert_eq!(
        evaluation.decision.completion_state,
        DecisionCompletionState::Pending
    );
    assert!(evaluation.decision.completed_at.is_none());
    assert_eq!(evaluation.decision.source_fingerprint, hash('f'));
}

#[test]
fn decision_domain_rejects_inconsistent_completion_and_actor_version() {
    let rule_set_id = default_rule_set_id(3);
    let base = ReleasePolicyDecision {
        id: ReleasePolicyDecisionId::new(),
        release_id: ReleaseId::new(),
        rule_set_id,
        rule_set_version: 3,
        matched_rule_ids: Vec::new(),
        disposition: Disposition::Accept,
        evidence_summary: "accepted".to_string(),
        source_fingerprint: hash('a'),
        actor: PolicyActor::System {
            policy_rule_set_version: 3,
        },
        decided_at: NOW,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    };
    ReleasePolicyDecision::new(base.clone()).expect("valid decision");

    let mut invalid = base.clone();
    invalid.completed_at = Some(NOW);
    assert_eq!(
        ReleasePolicyDecision::new(invalid),
        Err(DomainInvariantError::InvalidDecisionCompletionState)
    );
    let mut invalid = base;
    invalid.actor = PolicyActor::System {
        policy_rule_set_version: 2,
    };
    assert_eq!(
        ReleasePolicyDecision::new(invalid),
        Err(DomainInvariantError::PolicyActorVersionMismatch)
    );
}

#[test]
fn policy_test_summary_validates_complete_bounded_aggregates() {
    let release_ids = vec![ReleaseId::new(), ReleaseId::new()];
    let summary = PolicyTestSummary {
        test_run_id: PolicyTestRunId::new(),
        rule_set_id: default_rule_set_id(1),
        rule_set_version: 1,
        total_releases_evaluated: 2,
        matched_rule_counts: vec![("POL-GH-REDUNDANT".to_string(), 1)],
        existing_disposition_counts: vec![(None, 1), (Some(Disposition::Review), 1)],
        disposition_counts: vec![(Disposition::Accept, 1), (Disposition::Delete, 1)],
        destructive_impact_count: 1,
        sample_release_ids: release_ids,
        ran_by: PolicyActor::System {
            policy_rule_set_version: 1,
        },
        ran_at: NOW,
    };
    PolicyTestSummary::new(summary.clone()).expect("valid complete summary");

    let mut invalid = summary.clone();
    invalid.existing_disposition_counts = vec![(None, 1)];
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::InvalidPolicyTestSummary)
    );
    let mut invalid = summary.clone();
    invalid.destructive_impact_count = 0;
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::InvalidPolicyTestSummary)
    );
    let mut invalid = summary.clone();
    invalid.matched_rule_counts = vec![("RULE-A".to_string(), 2), ("RULE-B".to_string(), 1)];
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::InvalidPolicyTestSummary)
    );
    let mut invalid = summary.clone();
    invalid.ran_by = PolicyActor::System {
        policy_rule_set_version: 2,
    };
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::PolicyActorVersionMismatch)
    );

    let mut invalid = summary.clone();
    invalid.total_releases_evaluated =
        u32::try_from(MAX_POLICY_TEST_RELEASES + 1).expect("test limit fits u32");
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::PolicyTestScopeExceeded {
            dimension: "Releases",
            limit: MAX_POLICY_TEST_RELEASES,
        })
    );
    let mut invalid = summary.clone();
    invalid.matched_rule_counts = (0..=MAX_POLICY_TEST_MATCHED_RULES)
        .map(|index| (format!("RULE-{index}"), 1))
        .collect();
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::PolicyTestScopeExceeded {
            dimension: "matched rules",
            limit: MAX_POLICY_TEST_MATCHED_RULES,
        })
    );
    let mut invalid = summary;
    invalid.total_releases_evaluated =
        u32::try_from(MAX_POLICY_TEST_SAMPLES + 1).expect("sample cap fits u32");
    invalid.existing_disposition_counts = vec![(None, invalid.total_releases_evaluated)];
    invalid.disposition_counts = vec![(Disposition::Accept, invalid.total_releases_evaluated)];
    invalid.destructive_impact_count = 0;
    invalid.sample_release_ids = (0..=MAX_POLICY_TEST_SAMPLES)
        .map(|_| ReleaseId::new())
        .collect();
    assert_eq!(
        PolicyTestSummary::new(invalid),
        Err(DomainInvariantError::PolicyTestScopeExceeded {
            dimension: "sample Releases",
            limit: MAX_POLICY_TEST_SAMPLES,
        })
    );
}

#[test]
fn dry_run_is_bounded_typed_and_reports_existing_and_proposed_outcomes() {
    let mut configured = config();
    configured.batch_size = 1;
    let engine = PolicyEngine::new(configured).expect("engine");
    let release_id = ReleaseId::new();
    let classification = classify(
        release_id,
        Some("Studio Release"),
        vec![ReleaseTrait::Studio],
        Vec::new(),
    );
    let novelty = novelty_input(
        &engine,
        release_id,
        vec![fact(
            release_id,
            melodee_domain::AudioIdentityId::new(),
            Some(hash('b')),
            true,
        )],
    );
    let assessment = NoveltyEngine.assess(&novelty).expect("assessment");
    let mut input = policy_input(classification, assessment);
    input.existing_disposition = Some(Disposition::Review);
    let runner = PolicyDryRunner::new(engine);
    let result = runner
        .run_dry_run(vec![input.clone()], NOW)
        .expect("bounded dry run");
    assert_eq!(result.stats.evaluated, 1);
    assert_eq!(result.existing_disposition_counts["review"], 1);
    assert_eq!(result.proposed_disposition_counts["accept"], 1);
    assert_eq!(result.destructive_impact_count, 0);
    assert_eq!(result.sample_release_ids, vec![release_id]);

    assert!(matches!(
        runner.run_dry_run(vec![input.clone(), input], NOW),
        Err(PolicyDryRunError::TooManyReleases { max: 1 })
    ));
}

#[test]
fn dry_run_rejects_total_nested_track_facts_at_cap_plus_one() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let mut inputs = Vec::new();
    for track_count in [MAX_DRY_RUN_TRACK_FACTS, 1] {
        let release_id = ReleaseId::new();
        let tracks = (0..track_count)
            .map(|_| {
                fact(
                    release_id,
                    melodee_domain::AudioIdentityId::new(),
                    Some(hash('b')),
                    true,
                )
            })
            .collect();
        let novelty = NoveltyEngine
            .assess(&novelty_input(&engine, release_id, tracks))
            .expect("individually bounded novelty");
        inputs.push(policy_input(
            classify(release_id, None, vec![ReleaseTrait::Studio], Vec::new()),
            novelty,
        ));
    }
    assert!(matches!(
        PolicyDryRunner::new(engine).run_dry_run(inputs, NOW),
        Err(PolicyDryRunError::TooManyTrackFacts {
            max: MAX_DRY_RUN_TRACK_FACTS
        })
    ));
}

#[test]
fn dry_run_rejects_total_nested_classification_facts_at_cap_plus_one() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let per_release = MAX_RELEASE_CLASSIFICATION_EVIDENCE;
    let release_count = MAX_DRY_RUN_CLASSIFICATION_FACTS / (per_release + 1) + 1;
    let mut inputs = Vec::with_capacity(release_count);
    for _ in 0..release_count {
        let release_id = ReleaseId::new();
        let novelty = NoveltyEngine
            .assess(&novelty_input(
                &engine,
                release_id,
                vec![fact(
                    release_id,
                    melodee_domain::AudioIdentityId::new(),
                    Some(hash('b')),
                    true,
                )],
            ))
            .expect("novelty");
        inputs.push(policy_input(
            classification_with_evidence_count(release_id, per_release),
            novelty,
        ));
    }
    assert!(matches!(
        PolicyDryRunner::new(engine).run_dry_run(inputs, NOW),
        Err(PolicyDryRunError::TooManyClassificationFacts {
            max: MAX_DRY_RUN_CLASSIFICATION_FACTS
        })
    ));
}

#[test]
fn duplicate_rule_ids_and_unsafe_rule_dispositions_are_rejected() {
    let duplicate = PolicyRule {
        id: "DUP".to_string(),
        rule_type: PolicyRuleType::CorruptMedia,
        predicate_json: json!({}),
        disposition: Disposition::Quarantine,
        confidence_threshold: None,
    };
    let rule_set = PolicyRuleSet {
        id: default_rule_set_id(4),
        name: "duplicates".to_string(),
        version: 4,
        rules: vec![duplicate.clone(), duplicate],
        created_at: NOW,
        created_by: PolicyActor::System {
            policy_rule_set_version: 4,
        },
    };
    assert_eq!(
        PolicyRuleSet::new(rule_set),
        Err(DomainInvariantError::DuplicatePolicyRuleId)
    );

    let unsafe_rule = PolicyRule {
        id: "UNSAFE".to_string(),
        rule_type: PolicyRuleType::GreatestHitsNovelty,
        predicate_json: json!({}),
        disposition: Disposition::Delete,
        confidence_threshold: None,
    };
    assert_eq!(
        unsafe_rule.validate(),
        Err(DomainInvariantError::InvalidPolicyRuleDisposition)
    );

    let unbounded_delete = PolicyRule {
        id: "UNBOUNDED-DELETE".to_string(),
        rule_type: PolicyRuleType::GreatestHitsRedundancy,
        predicate_json: json!({}),
        disposition: Disposition::Delete,
        confidence_threshold: None,
    };
    assert_eq!(
        unbounded_delete.validate(),
        Err(DomainInvariantError::InvalidPolicyRuleDisposition)
    );
}

#[test]
fn track_fact_set_has_no_duplicate_ids_after_assessment() {
    let engine = PolicyEngine::new(config()).expect("engine");
    let release_id = ReleaseId::new();
    let input = novelty_input(
        &engine,
        release_id,
        vec![
            fact(
                release_id,
                melodee_domain::AudioIdentityId::new(),
                Some(hash('b')),
                true,
            ),
            fact(
                release_id,
                melodee_domain::AudioIdentityId::new(),
                Some(hash('c')),
                true,
            ),
        ],
    );
    let assessment = NoveltyEngine.assess(&input).expect("assessment");
    let ids: HashSet<_> = assessment
        .track_novelty()
        .iter()
        .map(|novelty| novelty.track_id)
        .collect();
    assert_eq!(ids.len(), assessment.track_novelty().len());
}
