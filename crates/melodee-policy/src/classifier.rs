//! Release classification engine: collects evidence from observations and
//! catalog context, produces multi-valued `ReleaseClassification` with
//! explainable evidence and confidence (PRD-CLASS-001 through 003).

use std::collections::BTreeSet;

use melodee_domain::ReleaseClassification;
use melodee_domain::{
    ClassificationEvidence, ClassificationEvidenceKind, Confidence, EvidenceSource, ReleaseTrait,
};
use time::OffsetDateTime;

/// Constructs a Confidence from a known-valid basis-point value.
/// Falls back to 0 (always valid) if the value exceeds the maximum.
fn confidence(bp: u16) -> Confidence {
    match Confidence::new(bp) {
        Ok(c) => c,
        Err(_) => confidence(0),
    }
}

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
    pub existing_classification_traits: BTreeSet<ReleaseTrait>,
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
    ) -> melodee_domain::ReleaseClassification {
        let mut evidence: Vec<ClassificationEvidence> = Vec::new();
        let mut traits: BTreeSet<ReleaseTrait> = input.existing_classification_traits.clone();

        // Title-based evidence (tier 5).
        if let Some(title) = &input.title {
            for trait_value in greatest_hits_title_traits(title) {
                traits.insert(trait_value);
                evidence.push(ClassificationEvidence {
                    id: melodee_domain::ReleaseClassificationId::new(),
                    release_id: input.release_id,
                    trait_: trait_value,
                    source: EvidenceSource::ReleaseTitle,
                    kind: ClassificationEvidenceKind::TitlePattern,
                    confidence: confidence(4_000),
                    detail: format!(
                        "Release title '{}' matched a pattern for {:?}",
                        title, trait_value
                    ),
                });
            }
        }

        // Directory-name evidence (tier 6, weakest).
        if let Some(dir) = &input.directory_name {
            for trait_value in greatest_hits_title_traits(dir) {
                traits.insert(trait_value);
                evidence.push(ClassificationEvidence {
                    id: melodee_domain::ReleaseClassificationId::new(),
                    release_id: input.release_id,
                    trait_: trait_value,
                    source: EvidenceSource::DirectoryName,
                    kind: ClassificationEvidenceKind::DirectoryName,
                    confidence: confidence(2_000),
                    detail: format!(
                        "Directory name '{}' matched a pattern for {:?}",
                        dir, trait_value
                    ),
                });
            }
        }

        // Parent directory-name evidence (tier 7, weakest).
        if let Some(parent) = &input.parent_directory_name {
            for trait_value in greatest_hits_title_traits(parent) {
                traits.insert(trait_value);
                evidence.push(ClassificationEvidence {
                    id: melodee_domain::ReleaseClassificationId::new(),
                    release_id: input.release_id,
                    trait_: trait_value,
                    source: EvidenceSource::ParentDirectoryName,
                    kind: ClassificationEvidenceKind::ParentDirectoryName,
                    confidence: confidence(1_500),
                    detail: format!(
                        "Parent directory '{}' matched a pattern for {:?}",
                        parent, trait_value
                    ),
                });
            }
        }

        // Structured tag evidence (tier 3).
        for &trait_value in &input.structured_tag_traits {
            traits.insert(trait_value);
            evidence.push(ClassificationEvidence {
                id: melodee_domain::ReleaseClassificationId::new(),
                release_id: input.release_id,
                trait_: trait_value,
                source: EvidenceSource::StructuredTags,
                kind: ClassificationEvidenceKind::StructuredField,
                confidence: confidence(7_000),
                detail: format!("Structured tag field identified {:?}", trait_value),
            });
        }

        // Embedded external ID evidence (tier 2).
        for &trait_value in &input.embedded_external_id_traits {
            traits.insert(trait_value);
            evidence.push(ClassificationEvidence {
                id: melodee_domain::ReleaseClassificationId::new(),
                release_id: input.release_id,
                trait_: trait_value,
                source: EvidenceSource::EmbeddedExternalId,
                kind: ClassificationEvidenceKind::EmbeddedExternalId,
                confidence: confidence(8_500),
                detail: format!("Embedded external ID identified {:?}", trait_value),
            });
        }

        // Catalog context evidence (tier 4).
        for &trait_value in &input.catalog_known_release_traits {
            traits.insert(trait_value);
            evidence.push(ClassificationEvidence {
                id: melodee_domain::ReleaseClassificationId::new(),
                release_id: input.release_id,
                trait_: trait_value,
                source: EvidenceSource::CatalogContext,
                kind: ClassificationEvidenceKind::CatalogKnownRelease,
                confidence: confidence(6_000),
                detail: format!("Catalog context identified {:?}", trait_value),
            });
        }

        // If no traits were found, default to Unknown.
        if traits.is_empty() {
            let trait_value = ReleaseTrait::Unknown;
            traits.insert(trait_value);
            evidence.push(ClassificationEvidence {
                id: melodee_domain::ReleaseClassificationId::new(),
                release_id: input.release_id,
                trait_: trait_value,
                source: EvidenceSource::ReleaseTitle,
                kind: ClassificationEvidenceKind::TitlePattern,
                confidence: confidence(100),
                detail: "No classification evidence found; defaulting to Unknown".to_string(),
            });
        }

        // Overall confidence = max evidence confidence, capped.
        let max_confidence = evidence
            .iter()
            .map(|e| e.confidence)
            .max()
            .unwrap_or(confidence(0));
        let classification_id = melodee_domain::ReleaseClassificationId::new();
        ReleaseClassification {
            id: classification_id,
            release_id: input.release_id,
            traits,
            evidence,
            confidence: max_confidence,
            classified_at: now,
            classifier_version: DEFAULT_CLASSIFIER_VERSION.to_string(),
        }
    }

    /// Returns true if the given classification's strongest evidence source is
    /// directory-only (PRD-CLASS-003: directory evidence cannot trigger
    /// destructive action).
    #[must_use]
    pub fn strongest_source_is_directory_only(
        classification: &melodee_domain::ReleaseClassification,
    ) -> bool {
        classification
            .evidence
            .iter()
            .min_by_key(|e| e.source.trust_rank())
            .is_some_and(|e| e.source.is_directory_only())
    }
}

/// Heuristically matches title text against Greatest Hits/compilation patterns.
fn greatest_hits_title_traits(title: &str) -> Vec<ReleaseTrait> {
    let lower = title.to_lowercase();
    let mut traits = Vec::new();
    if lower.contains("greatest hits")
        || lower.contains("best of")
        || lower.contains("collection")
        || lower.contains("compilation")
        || (lower.contains("greatest") && lower.contains("hit"))
    {
        traits.push(ReleaseTrait::GreatestHits);
        traits.push(ReleaseTrait::Compilation);
    }
    if lower.contains("box set") || lower.contains("boxset") {
        traits.push(ReleaseTrait::BoxSet);
    }
    if lower.contains("live") {
        traits.push(ReleaseTrait::Live);
    }
    if lower.contains("remaster") {
        traits.push(ReleaseTrait::Remaster);
    }
    if lower.contains("tribute") || lower.contains("cover") {
        traits.push(ReleaseTrait::Tribute);
    }
    if lower.contains("soundtrack") || lower.contains("ost") {
        traits.push(ReleaseTrait::Soundtrack);
    }
    if lower.contains("bootleg") {
        traits.push(ReleaseTrait::Bootleg);
    }
    if lower.contains("promo") {
        traits.push(ReleaseTrait::Promo);
    }
    if lower.contains("demo") {
        traits.push(ReleaseTrait::Demo);
    }
    if lower.contains("cast") || lower.contains("soundtrack") {
        traits.push(ReleaseTrait::CastRecording);
    }
    if lower.contains("spoken word") || lower.contains("audiobook") {
        traits.push(ReleaseTrait::SpokenWord);
    }
    if lower.contains("comedy") {
        traits.push(ReleaseTrait::Comedy);
    }
    if lower.contains("radio") {
        traits.push(ReleaseTrait::RadioShow);
    }
    if lower.contains("dj mix") || lower.contains("djmix") {
        traits.push(ReleaseTrait::DjMix);
    }
    if lower.contains("remix") {
        traits.push(ReleaseTrait::Remix);
    }
    traits
}
