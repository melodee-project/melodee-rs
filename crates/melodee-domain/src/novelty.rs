//! Phase 5 Track novelty analysis (PRD-POLICY-004, PRD-POLICY-005, SPEC section 15).
//!
//! Novelty answers: "Is this Track genuinely new to the catalog, or do we
//! already have it?" Classification describes what a Release *is*; novelty
//! describes whether its contents are *new*.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{
    AudioIdentityId, Confidence, DomainInvariantError, MediaAssetId, PolicyRuleSetId, ReleaseId,
    TrackId, TrackNoveltyId,
};

/// Novelty category for one Track relative to the existing catalog
/// (PRD-POLICY-004, SPEC section 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackNoveltyCategory {
    /// Same stored canonical bytes (shared `MediaAsset`).
    ExactDuplicate,
    /// Same underlying `AudioIdentity`, possibly different encoding.
    KnownAudioIdentity,
    /// Distinct `AudioIdentity` from a known one (live/acoustic/re-record/remix).
    AlternateRecording,
    /// No catalog audio identity match.
    NewTrack,
    /// Insufficient evidence to decide.
    Uncertain,
}

impl TrackNoveltyCategory {
    /// Every category in canonical order.
    pub const ALL: [Self; 5] = [
        Self::ExactDuplicate,
        Self::KnownAudioIdentity,
        Self::AlternateRecording,
        Self::NewTrack,
        Self::Uncertain,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactDuplicate => "exact_duplicate",
            Self::KnownAudioIdentity => "known_audio_identity",
            Self::AlternateRecording => "alternate_recording",
            Self::NewTrack => "new_track",
            Self::Uncertain => "uncertain",
        }
    }

    /// Whether this category represents genuinely new or alternate material
    /// that may warrant keeping the Release (PRD-POLICY-005).
    #[must_use]
    pub const fn is_novel(self) -> bool {
        matches!(self, Self::NewTrack | Self::AlternateRecording)
    }

    /// Whether this category is considered redundant (not novel) for
    /// Greatest Hits deduplication logic.
    #[must_use]
    pub const fn is_redundant(self) -> bool {
        matches!(self, Self::ExactDuplicate | Self::KnownAudioIdentity)
    }

    /// Whether this category prevents automatic deletion (PRD-POLICY-005,
    /// SPEC-INV-008).
    #[must_use]
    pub const fn is_unsafe_to_auto_delete(self) -> bool {
        !self.is_redundant()
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }
}

/// Evidence dimension for Track novelty assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoveltyEvidenceKind {
    /// Same canonical `MediaAsset` byte hash.
    ByteHashMatch,
    /// Same `AudioIdentity` per durable match decision.
    AudioIdentityMatch,
    /// Distinct `AudioIdentity` under the same Artist/Release context.
    AudioIdentityDistinct,
    /// No catalog match found for this Track's audio identity.
    NoCatalogMatch,
    /// Insufficient evidence (missing fingerprint, unknown identity).
    InsufficientEvidence,
}

impl NoveltyEvidenceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ByteHashMatch => "byte_hash_match",
            Self::AudioIdentityMatch => "audio_identity_match",
            Self::AudioIdentityDistinct => "audio_identity_distinct",
            Self::NoCatalogMatch => "no_catalog_match",
            Self::InsufficientEvidence => "insufficient_evidence",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::ByteHashMatch,
            Self::AudioIdentityMatch,
            Self::AudioIdentityDistinct,
            Self::NoCatalogMatch,
            Self::InsufficientEvidence,
        ]
        .into_iter()
        .find(|k| k.as_str() == value)
    }
}

/// Per-Track novelty assessment against the existing catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrackNovelty {
    pub id: TrackNoveltyId,
    pub release_id: ReleaseId,
    pub track_id: TrackId,
    pub audio_identity_id: AudioIdentityId,
    pub media_asset_id: Option<MediaAssetId>,
    pub category: TrackNoveltyCategory,
    pub confidence: Confidence,
    pub evidence_kind: NoveltyEvidenceKind,
    pub detail: String,
    pub evaluated_at: OffsetDateTime,
}

impl TrackNovelty {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "TrackNoveltyId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.track_id.is_uuid_v7(), "TrackId")?;
        validate_internal_id(value.audio_identity_id.is_uuid_v7(), "AudioIdentityId")?;
        if let Some(media_asset_id) = value.media_asset_id {
            validate_internal_id(media_asset_id.is_uuid_v7(), "MediaAssetId")?;
        }
        validate_nonblank("track novelty detail", &value.detail)?;
        let evidence_matches_category = matches!(
            (value.category, value.evidence_kind),
            (
                TrackNoveltyCategory::ExactDuplicate,
                NoveltyEvidenceKind::ByteHashMatch
            ) | (
                TrackNoveltyCategory::KnownAudioIdentity,
                NoveltyEvidenceKind::AudioIdentityMatch
            ) | (
                TrackNoveltyCategory::AlternateRecording,
                NoveltyEvidenceKind::AudioIdentityDistinct
            ) | (
                TrackNoveltyCategory::NewTrack,
                NoveltyEvidenceKind::NoCatalogMatch
            ) | (
                TrackNoveltyCategory::Uncertain,
                NoveltyEvidenceKind::InsufficientEvidence
            )
        );
        if !evidence_matches_category {
            return Err(DomainInvariantError::NoveltyCountMismatch);
        }
        Ok(value)
    }
}

/// Aggregate novelty counts for one Release (PRD-POLICY-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoveltyEvaluation {
    pub release_id: ReleaseId,
    pub policy_rule_set_id: PolicyRuleSetId,
    pub exact_duplicate_count: u32,
    pub known_audio_identity_count: u32,
    pub alternate_recording_count: u32,
    pub new_track_count: u32,
    pub uncertain_count: u32,
    pub total_track_count: u32,
    pub evaluated_at: OffsetDateTime,
    pub evaluator_version: String,
}

impl NoveltyEvaluation {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.policy_rule_set_id.is_uuid_v7(), "PolicyRuleSetId")?;
        let sum = value
            .exact_duplicate_count
            .checked_add(value.known_audio_identity_count)
            .and_then(|sum| sum.checked_add(value.alternate_recording_count))
            .and_then(|sum| sum.checked_add(value.new_track_count))
            .and_then(|sum| sum.checked_add(value.uncertain_count))
            .ok_or(DomainInvariantError::NoveltyCountMismatch)?;
        if sum != value.total_track_count {
            return Err(DomainInvariantError::NoveltyCountMismatch);
        }
        validate_nonblank("evaluator version", &value.evaluator_version)?;
        Ok(value)
    }

    /// Count of Tracks that are genuinely new or alternate (PRD-POLICY-005).
    #[must_use]
    pub const fn novel_track_count(&self) -> u32 {
        self.new_track_count
            .saturating_add(self.alternate_recording_count)
    }

    /// Count of Tracks that are redundant (exact duplicate or known audio identity).
    #[must_use]
    pub const fn redundant_track_count(&self) -> u32 {
        self.exact_duplicate_count
            .saturating_add(self.known_audio_identity_count)
    }

    /// Whether the Release is fully redundant with the existing catalog
    /// (no novel and no uncertain Tracks).
    #[must_use]
    pub fn is_redundant(&self) -> bool {
        self.total_track_count > 0
            && self.redundant_track_count() == self.total_track_count
            && self.novel_track_count() == 0
            && self.uncertain_count == 0
    }

    /// Whether the Release contains uncertain Tracks that prevent automatic
    /// deletion (PRD-POLICY-005, SPEC-INV-008).
    #[must_use]
    pub const fn has_uncertain_tracks(&self) -> bool {
        self.uncertain_count > 0
    }

    /// Returns the count for a given category.
    #[must_use]
    pub fn count_for(&self, category: TrackNoveltyCategory) -> u32 {
        match category {
            TrackNoveltyCategory::ExactDuplicate => self.exact_duplicate_count,
            TrackNoveltyCategory::KnownAudioIdentity => self.known_audio_identity_count,
            TrackNoveltyCategory::AlternateRecording => self.alternate_recording_count,
            TrackNoveltyCategory::NewTrack => self.new_track_count,
            TrackNoveltyCategory::Uncertain => self.uncertain_count,
        }
    }

    /// Returns a map of category -> count for all categories.
    #[must_use]
    pub fn counts_by_category(&self) -> BTreeMap<TrackNoveltyCategory, u32> {
        let mut map = BTreeMap::new();
        for category in TrackNoveltyCategory::ALL {
            map.insert(category, self.count_for(category));
        }
        map
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::EvidenceSource;

    const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    #[test]
    fn novel_categories_are_novel() {
        assert!(TrackNoveltyCategory::NewTrack.is_novel());
        assert!(TrackNoveltyCategory::AlternateRecording.is_novel());
        assert!(!TrackNoveltyCategory::ExactDuplicate.is_novel());
        assert!(!TrackNoveltyCategory::KnownAudioIdentity.is_novel());
    }

    #[test]
    fn uncertain_is_unsafe_to_auto_delete() {
        assert!(TrackNoveltyCategory::Uncertain.is_unsafe_to_auto_delete());
        assert!(!TrackNoveltyCategory::ExactDuplicate.is_unsafe_to_auto_delete());
    }

    #[test]
    fn evidence_source_hierarchy_order() {
        assert!(
            EvidenceSource::Administrator.trust_rank()
                < EvidenceSource::ParentDirectoryName.trust_rank()
        );
        assert!(EvidenceSource::DirectoryName.is_directory_only());
        assert!(EvidenceSource::ParentDirectoryName.is_directory_only());
        assert!(!EvidenceSource::ReleaseTitle.is_directory_only());
    }

    #[test]
    fn novelty_evaluation_counts_must_match() {
        let release_id = ReleaseId::new();
        let rule_set_id = PolicyRuleSetId::new();
        let eval = NoveltyEvaluation {
            release_id,
            policy_rule_set_id: rule_set_id,
            exact_duplicate_count: 3,
            known_audio_identity_count: 2,
            alternate_recording_count: 0,
            new_track_count: 0,
            uncertain_count: 0,
            total_track_count: 5,
            evaluated_at: T0,
            evaluator_version: "v1".into(),
        };
        assert!(NoveltyEvaluation::new(eval).is_ok());
    }

    #[test]
    fn novelty_evaluation_rejects_count_mismatch() {
        let release_id = ReleaseId::new();
        let rule_set_id = PolicyRuleSetId::new();
        let eval = NoveltyEvaluation {
            release_id,
            policy_rule_set_id: rule_set_id,
            exact_duplicate_count: 3,
            known_audio_identity_count: 2,
            alternate_recording_count: 1,
            new_track_count: 5,
            uncertain_count: 0,
            total_track_count: 5,
            evaluated_at: T0,
            evaluator_version: "v1".into(),
        };
        assert!(matches!(
            NoveltyEvaluation::new(eval),
            Err(DomainInvariantError::NoveltyCountMismatch)
        ));
    }

    #[test]
    fn is_redundant_when_all_tracks_are_duplicate_or_known() {
        let release_id = ReleaseId::new();
        let rule_set_id = PolicyRuleSetId::new();
        let eval = NoveltyEvaluation {
            release_id,
            policy_rule_set_id: rule_set_id,
            exact_duplicate_count: 5,
            known_audio_identity_count: 3,
            alternate_recording_count: 0,
            new_track_count: 0,
            uncertain_count: 0,
            total_track_count: 8,
            evaluated_at: T0,
            evaluator_version: "v1".into(),
        };
        let eval = NoveltyEvaluation::new(eval).unwrap();
        assert!(eval.is_redundant());
        assert_eq!(eval.novel_track_count(), 0);
        assert!(!eval.has_uncertain_tracks());
    }

    #[test]
    fn is_not_redundant_with_new_or_alternate_tracks() {
        let release_id = ReleaseId::new();
        let rule_set_id = PolicyRuleSetId::new();
        let eval = NoveltyEvaluation {
            release_id,
            policy_rule_set_id: rule_set_id,
            exact_duplicate_count: 3,
            known_audio_identity_count: 2,
            alternate_recording_count: 1,
            new_track_count: 1,
            uncertain_count: 0,
            total_track_count: 7,
            evaluated_at: T0,
            evaluator_version: "v1".into(),
        };
        let eval = NoveltyEvaluation::new(eval).unwrap();
        assert!(!eval.is_redundant());
        assert_eq!(eval.novel_track_count(), 2);
    }
}
