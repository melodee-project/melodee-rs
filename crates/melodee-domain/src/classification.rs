//! Release classification: multi-valued traits, evidence, and the
//! evidence hierarchy (PRD-CLASS-001, PRD-CLASS-002, PRD-CLASS-003,
//! SPEC section 15).
//!
//! Classification describes what Melodee believes a Release is. It is separate
//! from policy disposition (PRD-POLICY-002): policy decides what to do with
//! that belief.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{Confidence, DomainInvariantError, ReleaseClassificationId, ReleaseId, ReleaseTrait};

/// Complete ReleaseTrait vocabulary; a classification cannot contain more
/// distinct traits than the domain defines.
pub const MAX_RELEASE_CLASSIFICATION_TRAITS: usize = ReleaseTrait::ALL.len();
/// Shared domain/persistence cap for explainable evidence on one Release.
pub const MAX_RELEASE_CLASSIFICATION_EVIDENCE: usize = 1_000;

/// Position in the evidence trust hierarchy (PRD-CLASS-003).
///
/// Lower numeric values are higher trust. Admin decisions outrank all
/// automated signals; directory/path naming is weakest and can never alone
/// authorize destructive action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Administrator,
    EmbeddedExternalId,
    StructuredTags,
    CatalogContext,
    ReleaseTitle,
    DirectoryName,
    ParentDirectoryName,
}

impl EvidenceSource {
    /// Every source in canonical hierarchy order, highest trust first.
    pub const ALL_BY_TRUST: [Self; 7] = [
        Self::Administrator,
        Self::EmbeddedExternalId,
        Self::StructuredTags,
        Self::CatalogContext,
        Self::ReleaseTitle,
        Self::DirectoryName,
        Self::ParentDirectoryName,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Administrator => "administrator",
            Self::EmbeddedExternalId => "embedded_external_id",
            Self::StructuredTags => "structured_tags",
            Self::CatalogContext => "catalog_context",
            Self::ReleaseTitle => "release_title",
            Self::DirectoryName => "directory_name",
            Self::ParentDirectoryName => "parent_directory_name",
        }
    }

    /// Returns true for the two weakest tiers (directory/path naming only).
    #[must_use]
    pub const fn is_directory_only(self) -> bool {
        matches!(self, Self::DirectoryName | Self::ParentDirectoryName)
    }

    /// Numeric ordering: lower means higher trust. `Administrator` is 0.
    #[must_use]
    pub const fn trust_rank(self) -> u8 {
        match self {
            Self::Administrator => 0,
            Self::EmbeddedExternalId => 1,
            Self::StructuredTags => 2,
            Self::CatalogContext => 3,
            Self::ReleaseTitle => 4,
            Self::DirectoryName => 5,
            Self::ParentDirectoryName => 6,
        }
    }
}

/// Dimension of evidence considered for a Release trait (PRD-CLASS-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationEvidenceKind {
    /// Release title text matched a pattern.
    TitlePattern,
    /// Structured tag field (e.g. ID3 TCON, MusicBrainz release-group type).
    StructuredField,
    /// Embedded external ID or provider-supplied release-type metadata.
    EmbeddedExternalId,
    /// Catalog context: known same-name Releases, prior classifications.
    CatalogKnownRelease,
    /// Catalog context: observed Artist identity resolution.
    CatalogArtistMatch,
    /// Inbound directory name matched a pattern.
    DirectoryName,
    /// Parent directory name matched a pattern.
    ParentDirectoryName,
    /// Administrator manual override.
    ManualOverride,
}

/// One immutable evidence record supporting a trait for a Release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationEvidence {
    pub id: ReleaseClassificationId,
    pub release_id: ReleaseId,
    pub trait_: ReleaseTrait,
    pub source: EvidenceSource,
    pub kind: ClassificationEvidenceKind,
    pub confidence: Confidence,
    pub detail: String,
}

impl ClassificationEvidence {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ReleaseClassificationId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_nonblank("classification evidence detail", &value.detail)?;
        Ok(value)
    }
}

impl EvidenceSource {
    /// Parses the storage form.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL_BY_TRUST.into_iter().find(|s| s.as_str() == value)
    }
}

impl ClassificationEvidenceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TitlePattern => "title_pattern",
            Self::StructuredField => "structured_field",
            Self::EmbeddedExternalId => "embedded_external_id",
            Self::CatalogKnownRelease => "catalog_known_release",
            Self::CatalogArtistMatch => "catalog_artist_match",
            Self::DirectoryName => "directory_name",
            Self::ParentDirectoryName => "parent_directory_name",
            Self::ManualOverride => "manual_override",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::TitlePattern,
            Self::StructuredField,
            Self::EmbeddedExternalId,
            Self::CatalogKnownRelease,
            Self::CatalogArtistMatch,
            Self::DirectoryName,
            Self::ParentDirectoryName,
            Self::ManualOverride,
        ]
        .into_iter()
        .find(|k| k.as_str() == value)
    }
}

/// A confirmed classification for one Release: multi-valued traits plus
/// explainable evidence (PRD-CLASS-001, PRD-CLASS-002).
///
/// `traits` may legitimately contain only `Unknown` if no stronger signal
/// was available. `evidence` records why each trait is believed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseClassification {
    pub id: ReleaseClassificationId,
    pub release_id: ReleaseId,
    pub traits: BTreeSet<ReleaseTrait>,
    pub evidence: Vec<ClassificationEvidence>,
    pub confidence: Confidence,
    pub classified_at: OffsetDateTime,
    pub classifier_version: String,
}

impl ReleaseClassification {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ReleaseClassificationId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_nonblank("classifier version", &value.classifier_version)?;
        if value.traits.len() > MAX_RELEASE_CLASSIFICATION_TRAITS {
            return Err(DomainInvariantError::ClassificationScopeExceeded {
                dimension: "traits",
                limit: MAX_RELEASE_CLASSIFICATION_TRAITS,
            });
        }
        if value.evidence.len() > MAX_RELEASE_CLASSIFICATION_EVIDENCE {
            return Err(DomainInvariantError::ClassificationScopeExceeded {
                dimension: "evidence rows",
                limit: MAX_RELEASE_CLASSIFICATION_EVIDENCE,
            });
        }
        if value.traits.is_empty() || value.evidence.is_empty() {
            return Err(DomainInvariantError::IncompleteClassificationEvidence);
        }
        for evidence in &value.evidence {
            ClassificationEvidence::new(evidence.clone())?;
            if evidence.id != value.id
                || evidence.release_id != value.release_id
                || !value.traits.contains(&evidence.trait_)
            {
                return Err(DomainInvariantError::ForeignClassificationEvidence);
            }
        }
        let mut evidence_keys = BTreeSet::new();
        if value.evidence.iter().any(|evidence| {
            !evidence_keys.insert((
                evidence.trait_,
                evidence.source,
                evidence.kind.as_str(),
                evidence.detail.as_str(),
            ))
        }) {
            return Err(DomainInvariantError::DuplicateClassificationEvidence);
        }
        if value.traits.iter().any(|trait_| {
            !value
                .evidence
                .iter()
                .any(|evidence| evidence.trait_ == *trait_)
        }) {
            return Err(DomainInvariantError::IncompleteClassificationEvidence);
        }
        let max_confidence = value
            .evidence
            .iter()
            .map(|evidence| evidence.confidence)
            .max()
            .ok_or(DomainInvariantError::IncompleteClassificationEvidence)?;
        if value.confidence != max_confidence {
            return Err(DomainInvariantError::InvalidClassificationConfidence);
        }
        Ok(value)
    }

    #[must_use]
    pub fn has_trait(&self, trait_: ReleaseTrait) -> bool {
        self.traits.contains(&trait_)
    }

    /// Evidence supporting one trait, isolated from unrelated classification
    /// dimensions so policy confidence cannot be borrowed across traits.
    pub fn evidence_for(
        &self,
        trait_: ReleaseTrait,
    ) -> impl Iterator<Item = &ClassificationEvidence> {
        self.evidence
            .iter()
            .filter(move |evidence| evidence.trait_ == trait_)
    }

    /// Highest confidence from a non-directory source supporting exactly one
    /// trait. This is the destructive-policy confidence boundary.
    #[must_use]
    pub fn strongest_non_directory_confidence_for(
        &self,
        trait_: ReleaseTrait,
    ) -> Option<Confidence> {
        self.evidence_for(trait_)
            .filter(|evidence| !evidence.source.is_directory_only())
            .map(|evidence| evidence.confidence)
            .max()
    }
}
