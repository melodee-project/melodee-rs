//! Mapping from the authoritative PostgreSQL catalog snapshot to pure policy
//! inputs. Stored paths are parsed lexically and are never opened.

use std::path::Path;

use melodee_persistence::PolicyCatalogContext;
use time::OffsetDateTime;

use crate::{
    AdministratorClassification, ClassificationInput, DEFAULT_NOVELTY_EVALUATOR_VERSION,
    NoveltyInput, NoveltyTrackFact, PolicyEngine, PolicyEngineError, PolicyEvaluation,
};

/// Classifies, assesses novelty, and evaluates one authoritative catalog
/// snapshot without reading source or canonical media.
pub fn evaluate_catalog_context(
    engine: &PolicyEngine,
    context: &PolicyCatalogContext,
    now: OffsetDateTime,
) -> Result<PolicyEvaluation, PolicyEngineError> {
    let release_id = context.claim.release_id;
    let classification_input = ClassificationInput {
        release_id,
        title: Some(context.claim.title.clone()),
        directory_name: stored_file_name(&context.claim.item.current_path),
        parent_directory_name: stored_parent_name(&context.claim.item.current_path),
        structured_tag_traits: context.structured_tag_traits.clone(),
        embedded_external_id_traits: context.embedded_external_id_traits.clone(),
        catalog_known_release_traits: context.catalog_known_release_traits.clone(),
        administrator_traits: Vec::<AdministratorClassification>::new(),
    };
    let novelty_input = NoveltyInput {
        release_id,
        tracks: context
            .tracks
            .iter()
            .map(|fact| NoveltyTrackFact {
                release_id: melodee_domain::ReleaseId::from_uuid(fact.release_id),
                track_id: melodee_domain::TrackId::from_uuid(fact.track_id),
                audio_identity_id: melodee_domain::AudioIdentityId::from_uuid(
                    fact.audio_identity_id,
                ),
                media_asset_id: fact
                    .media_asset_id
                    .map(melodee_domain::MediaAssetId::from_uuid),
                byte_hash: fact.byte_hash.clone(),
                audio_fingerprint_digest: fact.audio_fingerprint_digest.clone(),
                audio_identity_confirmed: fact.has_confirmed_audio_identity,
            })
            .collect(),
        catalog_exact_byte_hashes: context.catalog_exact_byte_hashes.clone(),
        catalog_known_audio_identities: context.catalog_known_audio_identities.clone(),
        catalog_alternate_audio_identities: context.catalog_alternate_audio_identities.clone(),
        catalog_acoustic_fingerprints: context.catalog_acoustic_fingerprints.clone(),
        policy_rule_set_id: engine.rule_set_id(),
        evaluator_version: DEFAULT_NOVELTY_EVALUATOR_VERSION.to_owned(),
        now,
    };
    engine.evaluate_facts(
        &classification_input,
        &novelty_input,
        context.existing_disposition,
        context.claim.source_fingerprint.clone(),
        context.has_corrupt_media,
        None,
        now,
    )
}

fn stored_file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn stored_parent_name(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeSet;

    use melodee_domain::{
        AudioIdentityId, Disposition, EvidenceSource, MediaAssetId, ReleaseId, ReleaseTrait,
        ReleaseWorkItem, TrackId, TrackNoveltyCategory,
    };
    use melodee_persistence::{CatalogTrackFact, PolicyClaim};
    use uuid::Uuid;

    use super::*;
    use crate::PolicyConfig;

    const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    fn hash(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn context(path: String) -> PolicyCatalogContext {
        let release_id = ReleaseId::new();
        let audio_identity_id = AudioIdentityId::new();
        let byte_hash = hash('c');
        let mut item = ReleaseWorkItem::discovered(path.clone(), None, Uuid::now_v7(), NOW);
        item.current_path = path;
        PolicyCatalogContext {
            claim: PolicyClaim {
                item,
                release_id,
                title: "Ordinary Release".to_owned(),
                source_fingerprint: hash('a'),
                semantic_manifest_hash: hash('b'),
            },
            tracks: vec![CatalogTrackFact {
                release_id: release_id.into_uuid(),
                track_id: TrackId::new().into_uuid(),
                audio_identity_id: audio_identity_id.into_uuid(),
                media_asset_id: Some(MediaAssetId::new().into_uuid()),
                byte_hash: Some(byte_hash),
                audio_fingerprint: Some("fingerprint-observation".to_owned()),
                audio_fingerprint_digest: None,
                has_confirmed_audio_identity: false,
            }],
            current_classification: None,
            current_decision: None,
            structured_tag_traits: Vec::new(),
            embedded_external_id_traits: Vec::new(),
            catalog_known_release_traits: Vec::new(),
            existing_disposition: None,
            has_corrupt_media: false,
            catalog_exact_byte_hashes: BTreeSet::new(),
            catalog_known_audio_identities: BTreeSet::new(),
            catalog_alternate_audio_identities: BTreeSet::new(),
            catalog_acoustic_fingerprints: BTreeSet::new(),
        }
    }

    #[test]
    fn authoritative_context_fields_map_to_one_policy_evaluation() {
        let engine = PolicyEngine::new(PolicyConfig::default()).expect("default engine");
        let mut context = context("/stored/Artist/Ordinary Release".to_owned());
        context.structured_tag_traits = vec![ReleaseTrait::Live];
        context.embedded_external_id_traits = vec![ReleaseTrait::Compilation];
        context.catalog_known_release_traits = vec![ReleaseTrait::Remaster];
        context.existing_disposition = Some(Disposition::Ignore);
        context
            .catalog_exact_byte_hashes
            .insert(context.tracks[0].byte_hash.clone().expect("byte hash"));

        let evaluation =
            evaluate_catalog_context(&engine, &context, NOW).expect("evaluate catalog context");

        assert!(evaluation.classification.has_trait(ReleaseTrait::Live));
        assert!(
            evaluation
                .classification
                .has_trait(ReleaseTrait::Compilation)
        );
        assert!(evaluation.classification.has_trait(ReleaseTrait::Remaster));
        assert_eq!(
            evaluation.novelty.track_novelty()[0].category,
            TrackNoveltyCategory::ExactDuplicate
        );
        assert_eq!(evaluation.existing_disposition, Some(Disposition::Ignore));
        assert_eq!(
            evaluation.decision.source_fingerprint,
            context.claim.source_fingerprint
        );
    }

    #[test]
    fn stored_path_is_split_lexically_without_filesystem_access() {
        let engine = PolicyEngine::new(PolicyConfig::default()).expect("default engine");
        let nonexistent = format!(
            "/melodee-policy-path-must-not-exist-{}/Live/Greatest Hits",
            Uuid::now_v7()
        );
        assert!(!Path::new(&nonexistent).exists());
        let context = context(nonexistent);

        let evaluation =
            evaluate_catalog_context(&engine, &context, NOW).expect("evaluate stored path");

        assert!(
            evaluation
                .classification
                .evidence
                .iter()
                .any(|evidence| evidence.trait_ == ReleaseTrait::GreatestHits
                    && evidence.source == EvidenceSource::DirectoryName)
        );
        assert!(
            evaluation
                .classification
                .evidence
                .iter()
                .any(|evidence| evidence.trait_ == ReleaseTrait::Live
                    && evidence.source == EvidenceSource::ParentDirectoryName)
        );
    }
}
