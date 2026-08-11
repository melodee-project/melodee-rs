//! Release classification engine: collects evidence from observations and
//! catalog context, produces multi-valued `ReleaseClassification` with
//! explainable evidence and confidence (PRD-CLASS-001 through 003).

use std::collections::BTreeSet;

use melodee_domain::ReleaseClassification;
use melodee_domain::{
    ClassificationEvidence, ClassificationEvidenceKind, Confidence, DomainInvariantError,
    EvidenceSource, ReleaseTrait,
};
use thiserror::Error;
use time::OffsetDateTime;

/// Default classifier version string persisted with classifications.
pub const DEFAULT_CLASSIFIER_VERSION: &str = "melodee.policy.classifier-v1";

/// Confidence (basis points) above which a trait hypothesis may be
/// considered for destructive disposition. Directory-only evidence never
/// reaches this threshold.
pub const DEFAULT_DESTRUCTIVE_CONFIDENCE: u16 = 8_000;

/// Input observations the classifier can consume.
#[derive(Debug, Clone)]
pub struct ClassificationInput {
    pub release_id: melodee_domain::ReleaseId,
    pub title: Option<String>,
    pub directory_name: Option<String>,
    pub parent_directory_name: Option<String>,
    pub structured_tag_traits: Vec<ReleaseTrait>,
    pub embedded_external_id_traits: Vec<ReleaseTrait>,
    pub catalog_known_release_traits: Vec<ReleaseTrait>,
    pub administrator_traits: Vec<AdministratorClassification>,
}

/// One locked manual classification signal with exact administrator
/// provenance. It participates at the highest evidence tier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdministratorClassification {
    pub trait_: ReleaseTrait,
    pub actor: String,
    pub reason: String,
}

/// Typed classification failures.
#[derive(Debug, Error)]
pub enum ClassificationError {
    #[error("classification domain invariant failed")]
    Domain(#[from] DomainInvariantError),
}

/// Pure classification engine: converts observations into a
/// `ReleaseClassification` with explainable evidence.
#[derive(Debug, Clone, Copy, Default)]
pub struct Classifier;

impl Classifier {
    /// Classifies a Release from its observations (PRD-CLASS-001/002/003).
    ///
    /// Evidence follows the hierarchy: admin > embedded > tags > catalog >
    /// title > directory > parent directory. Directory/path evidence alone
    /// never reaches the destructive-confidence threshold.
    pub fn classify(
        &self,
        input: &ClassificationInput,
        now: OffsetDateTime,
    ) -> Result<melodee_domain::ReleaseClassification, ClassificationError> {
        let classification_id = melodee_domain::ReleaseClassificationId::new();
        let mut evidence: Vec<ClassificationEvidence> = Vec::new();
        let mut traits: BTreeSet<ReleaseTrait> = BTreeSet::new();

        // Helper to add evidence: all evidence records share the parent
        // classification ID (domain invariant).
        macro_rules! add_evidence {
            ($trait_:expr, $source:expr, $kind:expr, $bp:expr, $detail:expr) => {
                traits.insert($trait_);
                evidence.push(ClassificationEvidence {
                    id: classification_id,
                    release_id: input.release_id,
                    trait_: $trait_,
                    source: $source,
                    kind: $kind,
                    confidence: Confidence::new($bp)?,
                    detail: $detail.to_string(),
                });
            };
        }

        // Administrator decisions are explicit locked evidence, never inferred
        // from the presence of an actor elsewhere in policy evaluation.
        for manual in input.administrator_traits.iter().collect::<BTreeSet<_>>() {
            melodee_domain::AdministratorOverride::new(melodee_domain::AdministratorOverride {
                actor: manual.actor.clone(),
                disposition: melodee_domain::Disposition::Review,
                reason: manual.reason.clone(),
            })?;
            add_evidence!(
                manual.trait_,
                EvidenceSource::Administrator,
                ClassificationEvidenceKind::ManualOverride,
                10_000,
                format!(
                    "Administrator '{}' classified {:?}: {}",
                    manual.actor, manual.trait_, manual.reason
                )
            );
        }

        // Title-based evidence (tier 5).
        if let Some(title) = &input.title {
            for trait_value in greatest_hits_title_traits(title) {
                add_evidence!(
                    trait_value,
                    EvidenceSource::ReleaseTitle,
                    ClassificationEvidenceKind::TitlePattern,
                    4_000,
                    format!(
                        "Release title '{}' matched a pattern for {:?}",
                        title, trait_value
                    )
                );
            }
        }

        // Directory-name evidence (tier 6, weakest).
        if let Some(dir) = &input.directory_name {
            for trait_value in greatest_hits_title_traits(dir) {
                add_evidence!(
                    trait_value,
                    EvidenceSource::DirectoryName,
                    ClassificationEvidenceKind::DirectoryName,
                    2_000,
                    format!(
                        "Directory name '{}' matched a pattern for {:?}",
                        dir, trait_value
                    )
                );
            }
        }

        // Parent directory-name evidence (tier 7, weakest).
        if let Some(parent) = &input.parent_directory_name {
            for trait_value in greatest_hits_title_traits(parent) {
                add_evidence!(
                    trait_value,
                    EvidenceSource::ParentDirectoryName,
                    ClassificationEvidenceKind::ParentDirectoryName,
                    1_500,
                    format!(
                        "Parent directory '{}' matched a pattern for {:?}",
                        parent, trait_value
                    )
                );
            }
        }

        // Structured tag evidence (tier 3).
        for &trait_value in input.structured_tag_traits.iter().collect::<BTreeSet<_>>() {
            add_evidence!(
                trait_value,
                EvidenceSource::StructuredTags,
                ClassificationEvidenceKind::StructuredField,
                7_000,
                format!("Structured tag field identified {:?}", trait_value)
            );
        }

        // Embedded external ID evidence (tier 2).
        for &trait_value in input
            .embedded_external_id_traits
            .iter()
            .collect::<BTreeSet<_>>()
        {
            add_evidence!(
                trait_value,
                EvidenceSource::EmbeddedExternalId,
                ClassificationEvidenceKind::EmbeddedExternalId,
                8_500,
                format!("Embedded external ID identified {:?}", trait_value)
            );
        }

        // Catalog context evidence (tier 4).
        for &trait_value in input
            .catalog_known_release_traits
            .iter()
            .collect::<BTreeSet<_>>()
        {
            add_evidence!(
                trait_value,
                EvidenceSource::CatalogContext,
                ClassificationEvidenceKind::CatalogKnownRelease,
                6_000,
                format!("Catalog context identified {:?}", trait_value)
            );
        }

        // If no traits were found, default to Unknown.
        if traits.is_empty() {
            add_evidence!(
                ReleaseTrait::Unknown,
                EvidenceSource::ReleaseTitle,
                ClassificationEvidenceKind::TitlePattern,
                100,
                "No classification evidence found; defaulting to Unknown"
            );
        }

        // Overall confidence = max evidence confidence.
        let max_confidence = evidence
            .iter()
            .map(|e| e.confidence)
            .max()
            .unwrap_or(Confidence::zero());

        ReleaseClassification::new(ReleaseClassification {
            id: classification_id,
            release_id: input.release_id,
            traits,
            evidence,
            confidence: max_confidence,
            classified_at: now,
            classifier_version: DEFAULT_CLASSIFIER_VERSION.to_string(),
        })
        .map_err(ClassificationError::from)
    }

    /// Returns true if a specific trait's highest-trust evidence source is
    /// directory-only. Policy checks remain trait-scoped; unrelated evidence
    /// must not make a weak trait eligible for a destructive rule.
    #[must_use]
    pub fn strongest_source_is_directory_only(
        classification: &melodee_domain::ReleaseClassification,
        trait_: ReleaseTrait,
    ) -> bool {
        classification
            .evidence_for(trait_)
            .min_by_key(|e| e.source.trust_rank())
            .is_some_and(|e| e.source.is_directory_only())
    }
}

/// Heuristically matches title text against Greatest Hits/compilation patterns.
fn greatest_hits_title_traits(title: &str) -> Vec<ReleaseTrait> {
    let words: Vec<String> = title
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let has_word =
        |candidates: &[&str]| words.iter().any(|word| candidates.contains(&word.as_str()));
    let has_phrase = |phrase: &[&str]| {
        words
            .windows(phrase.len())
            .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
    };
    let mut traits = Vec::new();
    if has_phrase(&["greatest", "hit"])
        || has_phrase(&["greatest", "hits"])
        || has_phrase(&["best", "of"])
        || has_word(&["collection", "collections", "compilation", "compilations"])
    {
        traits.push(ReleaseTrait::GreatestHits);
        traits.push(ReleaseTrait::Compilation);
    }
    if has_phrase(&["box", "set"]) || has_word(&["boxset", "boxsets"]) {
        traits.push(ReleaseTrait::BoxSet);
    }
    if has_word(&["live"]) {
        traits.push(ReleaseTrait::Live);
    }
    if has_word(&["remaster", "remastered", "remasters"]) {
        traits.push(ReleaseTrait::Remaster);
    }
    if has_word(&["tribute", "tributes", "cover", "covers"]) {
        traits.push(ReleaseTrait::Tribute);
    }
    if has_word(&["soundtrack", "soundtracks", "ost"]) {
        traits.push(ReleaseTrait::Soundtrack);
    }
    if has_word(&["bootleg", "bootlegs"]) {
        traits.push(ReleaseTrait::Bootleg);
    }
    if has_word(&["promo", "promos", "promotional"]) {
        traits.push(ReleaseTrait::Promo);
    }
    if has_word(&["demo", "demos"]) {
        traits.push(ReleaseTrait::Demo);
    }
    if has_phrase(&["cast", "recording"]) || has_word(&["soundtrack", "soundtracks", "ost"]) {
        traits.push(ReleaseTrait::CastRecording);
    }
    if has_phrase(&["spoken", "word"]) || has_word(&["audiobook", "audiobooks"]) {
        traits.push(ReleaseTrait::SpokenWord);
    }
    if has_word(&["comedy"]) {
        traits.push(ReleaseTrait::Comedy);
    }
    if has_word(&["radio"]) || has_phrase(&["radio", "show"]) {
        traits.push(ReleaseTrait::RadioShow);
    }
    if has_phrase(&["dj", "mix"]) || has_word(&["djmix"]) {
        traits.push(ReleaseTrait::DjMix);
    }
    if has_word(&["remix", "remixes"]) {
        traits.push(ReleaseTrait::Remix);
    }
    traits
}
