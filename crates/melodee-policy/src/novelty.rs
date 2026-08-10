//! Release-level novelty analysis: assesses a Release's audio-identity
//! overlap with the existing catalog, producing a `NoveltyCategory` and a
//! `NoveltyEvaluation` (PRD-POLICY-004, PRD-POLICY-005).

use std::collections::BTreeSet;

use melodee_domain::{
    AudioIdentityId, NoveltyEvaluation, PolicyRuleSetId, ReleaseId, TrackId, TrackNovelty,
    TrackNoveltyCategory,
};
use serde::{Deserialize, Serialize};

use thiserror::Error;
use time::OffsetDateTime;

/// Release-level novelty category summarizing a Release relative to the
/// existing catalog (PRD-POLICY-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoveltyCategory {
    /// All Tracks are exact duplicates of existing catalog entries; the
    /// Release is a reissue with identical content.
    Reissue,
    /// Some Tracks have known audio identities but differ in encoding/fingerprint
    /// (e.g., remastered audio).
    Remaster,
    /// The Release has some novel/alternate Tracks alongside known ones.
    Variant,
    /// No catalog match for this Release's content; it appears brand new.
    BrandNewRelease,
    /// Insufficient evidence to decide.
    Unknown,
}

impl NoveltyCategory {
    pub const ALL: [Self; 5] = [
        Self::Reissue,
        Self::Remaster,
        Self::Variant,
        Self::BrandNewRelease,
        Self::Unknown,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reissue => "reissue",
            Self::Remaster => "remaster",
            Self::Variant => "variant",
            Self::BrandNewRelease => "brand_new_release",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }

    /// Whether this category is considered genuinely new or alternate
    /// material that may warrant keeping the Release (PRD-POLICY-005).
    #[must_use]
    pub const fn is_novel(self) -> bool {
        matches!(self, Self::BrandNewRelease | Self::Variant | Self::Remaster)
    }

    /// Whether the Release is fully redundant (no novel content).
    #[must_use]
    pub const fn is_redundant(self) -> bool {
        matches!(self, Self::Reissue)
    }

    /// Whether the category prevents automatic deletion (PRD-POLICY-005,
    /// SPEC-INV-008).
    #[must_use]
    pub const fn is_unsafe_to_auto_delete(self) -> bool {
        !self.is_redundant()
    }
}

/// Input for novelty assessment: the Release's Tracks and what the catalog
/// already knows about each audio identity.
#[derive(Debug, Clone)]
pub struct NoveltyInput {
    pub release_id: ReleaseId,
    /// Each Track's audio identity and byte hash, if available.
    pub track_audio: Vec<(TrackId, AudioIdentityId, Option<String>)>,
    /// Audio identities confirmed as exact byte duplicates in the catalog.
    pub catalog_exact_duplicates: BTreeSet<AudioIdentityId>,
    /// Audio identities known but distinct encoding/fingerprint.
    pub catalog_known_identities: BTreeSet<AudioIdentityId>,
    /// Track-level novelty records pre-computed by the catalog engine.
    pub track_novelty: Vec<TrackNovelty>,
    pub evaluator_version: String,
    pub now: OffsetDateTime,
}

#[derive(Debug, Error)]
pub enum NoveltyEngineError {
    #[error("novelty evaluation count mismatch")]
    CountMismatch,
    #[error("invalid confidence basis points")]
    InvalidConfidence,
}

/// Engine that assesses Release-level novelty from per-track assessments
/// and catalog context.
#[derive(Debug, Clone, Default)]
pub struct NoveltyEngine;

impl NoveltyEngine {
    /// Assesses novelty for a Release given catalog context and per-Track
    /// novelty records (PRD-POLICY-004, PRD-POLICY-005).
    ///
    /// Returns the release-level `NoveltyCategory` and a `NoveltyEvaluation`
    /// with aggregate counts.
    pub fn assess(&self, input: &NoveltyInput) -> Result<NoveltyAssessment, NoveltyEngineError> {
        let total = input.track_novelty.len() as u32;

        let exact_duplicate_count =
            count_for_category(&input.track_novelty, TrackNoveltyCategory::ExactDuplicate);
        let known_audio_identity_count = count_for_category(
            &input.track_novelty,
            TrackNoveltyCategory::KnownAudioIdentity,
        );
        let alternate_recording_count = count_for_category(
            &input.track_novelty,
            TrackNoveltyCategory::AlternateRecording,
        );
        let new_track_count =
            count_for_category(&input.track_novelty, TrackNoveltyCategory::NewTrack);
        let uncertain_count =
            count_for_category(&input.track_novelty, TrackNoveltyCategory::Uncertain);

        let sum = exact_duplicate_count
            + known_audio_identity_count
            + alternate_recording_count
            + new_track_count
            + uncertain_count;
        if sum != total {
            return Err(NoveltyEngineError::CountMismatch);
        }

        let evaluation = NoveltyEvaluation {
            release_id: input.release_id,
            policy_rule_set_id: PolicyRuleSetId::new(),
            exact_duplicate_count,
            known_audio_identity_count,
            alternate_recording_count,
            new_track_count,
            uncertain_count,
            total_track_count: total,
            evaluated_at: input.now,
            evaluator_version: input.evaluator_version.clone(),
        };

        // Release-level category derived from aggregate novel/redundant counts.
        let category = if total == 0 {
            NoveltyCategory::Unknown
        } else if exact_duplicate_count == total
            && known_audio_identity_count == 0
            && alternate_recording_count == 0
            && new_track_count == 0
            && uncertain_count == 0
        {
            NoveltyCategory::Reissue
        } else if new_track_count == 0
            && alternate_recording_count == 0
            && (exact_duplicate_count + known_audio_identity_count) > 0
            && (exact_duplicate_count
                + known_audio_identity_count
                + new_track_count
                + alternate_recording_count)
                == total
            && known_audio_identity_count > 0
        {
            NoveltyCategory::Remaster
        } else if exact_duplicate_count + known_audio_identity_count == total
            && (new_track_count > 0 || alternate_recording_count > 0)
        {
            NoveltyCategory::Variant
        } else if exact_duplicate_count == 0
            && known_audio_identity_count == 0
            && new_track_count > 0
        {
            NoveltyCategory::BrandNewRelease
        } else {
            NoveltyCategory::Variant
        };

        Ok(NoveltyAssessment {
            category,
            evaluation,
        })
    }
}

fn count_for_category(items: &[TrackNovelty], category: TrackNoveltyCategory) -> u32 {
    items.iter().filter(|t| t.category == category).count() as u32
}

/// Result of a novelty assessment.
#[derive(Debug, Clone)]
pub struct NoveltyAssessment {
    pub category: NoveltyCategory,
    pub evaluation: NoveltyEvaluation,
}
