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
    /// Release is a reissue with identical content (safe for Greatest Hits
    /// redundancy).
    Reissue,
    /// Some Tracks have known audio identities but differ in encoding/fingerprint
    /// (e.g., remastered audio). This is NOT novel content — it is known material
    /// in a different encoding. A remaster should be kept (different encoding may
    /// differ in quality) but is not "new" content warranting a Review disposition
    /// purely on novelty grounds.
    Remaster,
    /// The Release has some novel/alternate Tracks (NewTrack or AlternateRecording)
    /// alongside known ones. Novel content requires Review before destructive action.
    Variant,
    /// No catalog match for this Release's content; it appears brand new.
    BrandNewRelease,
    /// Insufficient evidence to decide (no tracks, or all tracks uncertain).
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

    /// Whether this category represents genuinely new or alternate material
    /// that may warrant keeping the Release (PRD-POLICY-005).
    ///
    /// Remaster is NOT novel — it is known content in a different encoding.
    /// Only brand-new content (BrandNewRelease) or content with genuinely
    /// new/alternate tracks (Variant) is novel.
    #[must_use]
    pub const fn is_novel(self) -> bool {
        matches!(self, Self::BrandNewRelease | Self::Variant)
    }

    /// Whether the Release is fully redundant (no novel and no uncertain Tracks).
    /// A Release is redundant only when ALL Tracks are exact duplicates.
    /// KnownAudioIdentity tracks are NOT redundant — they are different
    /// encodings worth keeping.
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
    /// These are VERIFIED against the catalog sets above — caller-provided
    /// labels that contradict the catalog sets are downgraded to Uncertain.
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
    /// The engine verifies caller-provided `TrackNovelty` labels against the
    /// catalog sets:
    /// - A track labeled `ExactDuplicate` whose audio identity is NOT in
    ///   `catalog_exact_duplicates` is downgraded to `Uncertain`.
    /// - A track labeled `KnownAudioIdentity` whose audio identity is NOT in
    ///   `catalog_known_identities` is downgraded to `Uncertain`.
    ///
    /// Release-level category derived from aggregate counts:
    /// - All ExactDuplicate → Reissue (fully redundant, safe for Greatest Hits)
    /// - All KnownAudioIdentity (no ExactDuplicate) → Remaster (known content,
    ///   different encoding, not novel)
    /// - Mix of ExactDuplicate + KnownAudioIdentity → Remaster
    /// - Has NewTrack or AlternateRecording → Variant (novel content)
    /// - All NewTrack, no exact duplicates → BrandNewRelease
    /// - Otherwise → Unknown
    pub fn assess(&self, input: &NoveltyInput) -> Result<NoveltyAssessment, NoveltyEngineError> {
        let total = input.track_novelty.len() as u32;

        // Verify caller-provided labels against catalog sets; downgrade
        // contradictory labels to Uncertain.
        let mut verified: Vec<TrackNovelty> = Vec::with_capacity(input.track_novelty.len());
        for track in &input.track_novelty {
            let effective_category = match track.category {
                TrackNoveltyCategory::ExactDuplicate
                    if !input
                        .catalog_exact_duplicates
                        .contains(&track.audio_identity_id) =>
                {
                    TrackNoveltyCategory::Uncertain
                }
                TrackNoveltyCategory::KnownAudioIdentity
                    if !input
                        .catalog_known_identities
                        .contains(&track.audio_identity_id) =>
                {
                    TrackNoveltyCategory::Uncertain
                }
                other => other,
            };
            verified.push(TrackNovelty {
                category: effective_category,
                ..track.clone()
            });
        }

        let exact_duplicate_count =
            count_for_category(&verified, TrackNoveltyCategory::ExactDuplicate);
        let known_audio_identity_count =
            count_for_category(&verified, TrackNoveltyCategory::KnownAudioIdentity);
        let alternate_recording_count =
            count_for_category(&verified, TrackNoveltyCategory::AlternateRecording);
        let new_track_count = count_for_category(&verified, TrackNoveltyCategory::NewTrack);
        let uncertain_count = count_for_category(&verified, TrackNoveltyCategory::Uncertain);

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

        // Release-level category derived from verified aggregate counts.
        let has_novel = new_track_count > 0 || alternate_recording_count > 0;
        let all_exact = exact_duplicate_count == total && total > 0;
        let all_known = known_audio_identity_count == total && total > 0;
        let all_redundant =
            exact_duplicate_count + known_audio_identity_count == total && total > 0;

        let category = if total == 0 || uncertain_count == total {
            // No tracks or all uncertain → Unknown.
            NoveltyCategory::Unknown
        } else if all_exact {
            // All tracks are exact byte duplicates → reissue (fully redundant).
            NoveltyCategory::Reissue
        } else if all_known && !has_novel && uncertain_count == 0 {
            // All tracks have known audio identity (different encoding, no novel
            // content, no uncertain) → Remaster (known but distinct encoding).
            NoveltyCategory::Remaster
        } else if all_redundant && !has_novel && uncertain_count == 0 {
            // Mix of exact duplicates and known identities, no novel content → Remaster.
            NoveltyCategory::Remaster
        } else if has_uncertain_only_with_some_known(
            exact_duplicate_count,
            known_audio_identity_count,
            uncertain_count,
            total,
        ) {
            // Some tracks are known (exact dup or known identity) but others are
            // uncertain → not enough to classify as a clean Remaster; treat as
            // Variant since we can't confirm full redundancy.
            NoveltyCategory::Variant
        } else if has_novel && !all_exact && uncertain_count == 0 {
            // Has genuinely novel tracks (new or alternate) → Variant.
            NoveltyCategory::Variant
        } else if exact_duplicate_count == 0
            && known_audio_identity_count == 0
            && new_track_count == total
        {
            // All tracks are brand new → BrandNewRelease.
            NoveltyCategory::BrandNewRelease
        } else {
            // Mixed or unclassifiable → Variant (has some novel content).
            NoveltyCategory::Variant
        };

        Ok(NoveltyAssessment {
            category,
            evaluation,
            track_novelty: verified,
        })
    }
}

/// Checks if the only novel/uncertain content is from uncertainty mixed with
/// some known identical/identity tracks.
fn has_uncertain_only_with_some_known(
    exact_duplicate_count: u32,
    known_audio_identity_count: u32,
    uncertain_count: u32,
    total: u32,
) -> bool {
    uncertain_count > 0
        && (exact_duplicate_count + known_audio_identity_count) > 0
        && (exact_duplicate_count + known_audio_identity_count + uncertain_count) == total
}

fn count_for_category(items: &[TrackNovelty], category: TrackNoveltyCategory) -> u32 {
    items.iter().filter(|t| t.category == category).count() as u32
}

/// Result of a novelty assessment.
#[derive(Debug, Clone)]
pub struct NoveltyAssessment {
    pub category: NoveltyCategory,
    pub evaluation: NoveltyEvaluation,
    /// Per-track novelty records (verified against catalog sets), suitable for
    /// persistence alongside the evaluation.
    pub track_novelty: Vec<TrackNovelty>,
}
