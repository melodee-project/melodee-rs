//! Data-driven Greatest Hits compatibility and safety evidence.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use melodee_domain::{Disposition, MediaAssetId, ReleaseId, ReleaseTrait};
use melodee_policy::{
    ClassificationInput, DEFAULT_NOVELTY_EVALUATOR_VERSION, NoveltyInput, NoveltyTrackFact,
    PolicyConfig, PolicyEngine,
};
use serde::Deserialize;
use time::OffsetDateTime;

const CORPUS: &str = include_str!("../../../compatibility/phase5/greatest-hits-safety.json");
const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompatibilityCorpus {
    schema_version: u32,
    authority: CorpusAuthority,
    default_preferences: DefaultPreferences,
    cases: Vec<CompatibilityCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorpusAuthority {
    decision: String,
    policy_update: String,
    requirements: Vec<String>,
    legacy_evidence: String,
    legacy_behavior_preserved: String,
    legacy_behavior_not_preserved: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefaultPreferences {
    redundant_greatest_hits: String,
    novel_greatest_hits: String,
    uncertain_greatest_hits: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompatibilityCase {
    id: String,
    title: String,
    #[serde(default)]
    directory_name: Option<String>,
    #[serde(default)]
    trusted_traits: Vec<String>,
    track_novelty: Vec<String>,
    expected_disposition: String,
    forbidden_disposition: String,
}

#[test]
fn greatest_hits_compatibility_corpus_executes_the_real_policy_pipeline() {
    let corpus: CompatibilityCorpus =
        serde_json::from_str(CORPUS).expect("parse committed Phase 5 compatibility corpus");
    assert_eq!(corpus.schema_version, 2);
    assert_eq!(corpus.authority.decision, "ADR-0120");
    assert_eq!(corpus.authority.policy_update, "ADR-0177");
    assert_eq!(
        corpus.authority.requirements,
        [
            "PRD-CLASS-003",
            "PRD-POLICY-003",
            "PRD-POLICY-004",
            "PRD-POLICY-005",
            "PRD-POLICY-006",
            "PRD-POLICY-007",
        ]
    );
    assert_eq!(
        corpus.authority.legacy_evidence,
        "compatibility/media/README.md"
    );
    assert!(
        corpus
            .authority
            .legacy_behavior_preserved
            .contains("directory name alone is non-destructive")
    );
    assert!(
        corpus
            .authority
            .legacy_behavior_not_preserved
            .contains("Same-size files")
    );

    let config = PolicyConfig::default();
    assert_eq!(
        disposition(&corpus.default_preferences.redundant_greatest_hits),
        config.redundant_greatest_hits_disposition
    );
    assert_eq!(
        disposition(&corpus.default_preferences.novel_greatest_hits),
        config.novel_greatest_hits_disposition
    );
    assert_eq!(
        disposition(&corpus.default_preferences.uncertain_greatest_hits),
        config.uncertain_greatest_hits_disposition
    );
    let engine = PolicyEngine::new(config).expect("construct default policy pipeline");

    assert_eq!(corpus.cases.len(), 9, "corpus cases are intentional");
    let mut case_ids = BTreeSet::new();
    for case in &corpus.cases {
        assert!(case_ids.insert(case.id.as_str()), "duplicate case ID");
        let release_id = ReleaseId::new();
        let structured_tag_traits = case
            .trusted_traits
            .iter()
            .map(|trait_| ReleaseTrait::parse(trait_).expect("known corpus ReleaseTrait"))
            .collect();
        let classification = ClassificationInput {
            release_id,
            title: Some(case.title.clone()),
            directory_name: case.directory_name.clone(),
            parent_directory_name: None,
            structured_tag_traits,
            embedded_external_id_traits: Vec::new(),
            catalog_known_release_traits: Vec::new(),
            administrator_traits: Vec::new(),
        };
        let mut exact_hashes = BTreeSet::new();
        let mut known_identities = BTreeSet::new();
        let mut alternate_identities = BTreeSet::new();
        let tracks = case
            .track_novelty
            .iter()
            .enumerate()
            .map(|(ordinal, category)| {
                let audio_identity_id = melodee_domain::AudioIdentityId::new();
                let byte_hash = format!("{:064x}", ordinal + 1);
                let confirmed = match category.as_str() {
                    "exact_duplicate" => {
                        exact_hashes.insert(byte_hash.clone());
                        false
                    }
                    "known_audio_identity" => {
                        known_identities.insert(audio_identity_id);
                        false
                    }
                    "alternate_recording" => {
                        alternate_identities.insert(audio_identity_id);
                        true
                    }
                    "new_track" => true,
                    "uncertain" => false,
                    other => panic!("unknown corpus novelty category {other}"),
                };
                NoveltyTrackFact {
                    release_id,
                    track_id: melodee_domain::TrackId::new(),
                    audio_identity_id,
                    media_asset_id: Some(MediaAssetId::new()),
                    byte_hash: Some(byte_hash),
                    audio_identity_confirmed: confirmed,
                }
            })
            .collect();
        let novelty = NoveltyInput {
            release_id,
            tracks,
            catalog_exact_byte_hashes: exact_hashes,
            catalog_known_audio_identities: known_identities,
            catalog_alternate_audio_identities: alternate_identities,
            policy_rule_set_id: engine.rule_set_id(),
            evaluator_version: DEFAULT_NOVELTY_EVALUATOR_VERSION.to_string(),
            now: NOW,
        };

        let evaluated = engine
            .evaluate_facts(
                &classification,
                &novelty,
                None,
                "a".repeat(64),
                false,
                None,
                NOW,
            )
            .unwrap_or_else(|error| panic!("case {} failed evaluation: {error}", case.id));
        let expected = disposition(&case.expected_disposition);
        let forbidden = disposition(&case.forbidden_disposition);
        assert_eq!(
            evaluated.decision.disposition,
            expected,
            "case {} expected {} but matched {:?}: {}",
            case.id,
            case.expected_disposition,
            evaluated.decision.matched_rule_ids,
            evaluated.decision.evidence_summary
        );
        assert_ne!(
            evaluated.decision.disposition, forbidden,
            "case {} reached forbidden disposition {}",
            case.id, case.forbidden_disposition
        );
        let derived: Vec<&str> = evaluated
            .novelty
            .track_novelty()
            .iter()
            .map(|track| track.category.as_str())
            .collect();
        assert_eq!(
            derived, case.track_novelty,
            "case {} novelty drift",
            case.id
        );
    }
}

fn disposition(value: &str) -> Disposition {
    Disposition::parse(value).expect("known corpus Disposition")
}
