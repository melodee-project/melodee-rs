//! Track novelty derived from complete catalog facts (PRD-POLICY-004/005).

use std::collections::BTreeSet;

use melodee_domain::{
    AudioIdentityId, Confidence, DomainInvariantError, MediaAssetId, NoveltyEvaluation,
    NoveltyEvidenceKind, PolicyRuleSetId, ReleaseId, TrackId, TrackNovelty, TrackNoveltyCategory,
    TrackNoveltyId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

/// Hard bound preventing one malformed Release from causing unbounded memory
/// growth in policy evaluation.
pub const MAX_TRACKS_PER_NOVELTY_EVALUATION: usize = 10_000;

/// Stable evaluator identity persisted with fact-derived novelty rows.
pub const DEFAULT_NOVELTY_EVALUATOR_VERSION: &str = "melodee.policy.novelty-v1";

/// Release-level novelty summary used for operator reporting. Destructive
/// policy evaluates the underlying counts, never a caller-provided category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoveltyCategory {
    /// Every Track is an exact duplicate or a known AudioIdentity.
    Reissue,
    /// Retained for storage/wire compatibility; fact-derived evaluation folds
    /// known AudioIdentity Tracks into redundant `Reissue` semantics.
    Remaster,
    /// At least one Track is new or an alternate recording.
    Variant,
    /// Every Track is new to the catalog.
    BrandNewRelease,
    /// At least one Track lacks enough durable evidence to decide.
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
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    #[must_use]
    pub const fn is_novel(self) -> bool {
        matches!(self, Self::Variant | Self::BrandNewRelease)
    }

    #[must_use]
    pub const fn is_redundant(self) -> bool {
        matches!(self, Self::Reissue)
    }

    #[must_use]
    pub const fn is_unsafe_to_auto_delete(self) -> bool {
        !self.is_redundant()
    }
}

/// One authoritative same-Release Track fact loaded from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoveltyTrackFact {
    pub release_id: ReleaseId,
    pub track_id: TrackId,
    pub audio_identity_id: AudioIdentityId,
    pub media_asset_id: Option<MediaAssetId>,
    pub byte_hash: Option<String>,
    /// True only when the AudioIdentity has durable fingerprint/match evidence;
    /// provisional identity allocation alone is not confirmation.
    pub audio_identity_confirmed: bool,
}

/// Complete inputs for one Release novelty evaluation.
#[derive(Debug, Clone)]
pub struct NoveltyInput {
    pub release_id: ReleaseId,
    pub tracks: Vec<NoveltyTrackFact>,
    /// Canonical byte hashes present in another Release.
    pub catalog_exact_byte_hashes: BTreeSet<String>,
    /// Durable AudioIdentity IDs referenced by another Release.
    pub catalog_known_audio_identities: BTreeSet<AudioIdentityId>,
    /// Distinct identities confirmed as alternate performances/versions in the
    /// same catalog context.
    pub catalog_alternate_audio_identities: BTreeSet<AudioIdentityId>,
    pub policy_rule_set_id: PolicyRuleSetId,
    pub evaluator_version: String,
    pub now: OffsetDateTime,
}

/// Typed novelty failures. Invalid or incomplete facts fail closed rather than
/// becoming a destructive redundancy result.
#[derive(Debug, Error)]
pub enum NoveltyEngineError {
    #[error("novelty evaluation has no Tracks")]
    NoTracks,
    #[error("novelty evaluation exceeds the {max} Track limit")]
    TooManyTracks { max: usize },
    #[error("Track {track_id} does not belong to Release {release_id}")]
    ForeignTrack {
        track_id: TrackId,
        release_id: ReleaseId,
    },
    #[error("Track {0} appears more than once in novelty facts")]
    DuplicateTrack(TrackId),
    #[error("invalid BLAKE3 byte hash in novelty facts")]
    InvalidByteHash,
    #[error("an AudioIdentity cannot be both known and an alternate recording")]
    ConflictingAudioIdentityFact,
    #[error("novelty {dimension} exceeds the {max} item limit")]
    TooManyCatalogFacts { dimension: &'static str, max: usize },
    #[error("novelty domain invariant failed")]
    Domain(#[from] DomainInvariantError),
}

/// Pure novelty engine deriving Track categories from catalog facts.
#[derive(Debug, Clone, Default)]
pub struct NoveltyEngine;

impl NoveltyEngine {
    pub fn assess(&self, input: &NoveltyInput) -> Result<NoveltyAssessment, NoveltyEngineError> {
        validate_input(input)?;

        let mut rows = Vec::with_capacity(input.tracks.len());
        for fact in &input.tracks {
            let (category, evidence_kind, confidence, detail) = derive_track_novelty(input, fact)?;
            rows.push(TrackNovelty::new(TrackNovelty {
                id: TrackNoveltyId::new(),
                release_id: input.release_id,
                track_id: fact.track_id,
                audio_identity_id: fact.audio_identity_id,
                media_asset_id: fact.media_asset_id,
                category,
                confidence,
                evidence_kind,
                detail,
                evaluated_at: input.now,
            })?);
        }

        let total_track_count =
            u32::try_from(rows.len()).map_err(|_| NoveltyEngineError::TooManyTracks {
                max: MAX_TRACKS_PER_NOVELTY_EVALUATION,
            })?;
        let exact_duplicate_count =
            count_for_category(&rows, TrackNoveltyCategory::ExactDuplicate)?;
        let known_audio_identity_count =
            count_for_category(&rows, TrackNoveltyCategory::KnownAudioIdentity)?;
        let alternate_recording_count =
            count_for_category(&rows, TrackNoveltyCategory::AlternateRecording)?;
        let new_track_count = count_for_category(&rows, TrackNoveltyCategory::NewTrack)?;
        let uncertain_count = count_for_category(&rows, TrackNoveltyCategory::Uncertain)?;

        let evaluation = NoveltyEvaluation::new(NoveltyEvaluation {
            release_id: input.release_id,
            policy_rule_set_id: input.policy_rule_set_id,
            exact_duplicate_count,
            known_audio_identity_count,
            alternate_recording_count,
            new_track_count,
            uncertain_count,
            total_track_count,
            evaluated_at: input.now,
            evaluator_version: input.evaluator_version.clone(),
        })?;

        let category = if evaluation.has_uncertain_tracks() {
            NoveltyCategory::Unknown
        } else if evaluation.new_track_count == evaluation.total_track_count {
            NoveltyCategory::BrandNewRelease
        } else if evaluation.novel_track_count() > 0 {
            NoveltyCategory::Variant
        } else if evaluation.is_redundant() {
            NoveltyCategory::Reissue
        } else {
            NoveltyCategory::Unknown
        };

        Ok(NoveltyAssessment {
            category,
            evaluation,
            track_novelty: rows,
        })
    }
}

fn validate_input(input: &NoveltyInput) -> Result<(), NoveltyEngineError> {
    if !input.release_id.is_uuid_v7() || !input.policy_rule_set_id.is_uuid_v7() {
        return Err(NoveltyEngineError::Domain(
            DomainInvariantError::InvalidInternalId("novelty input identity"),
        ));
    }
    if input.evaluator_version.trim().is_empty() {
        return Err(NoveltyEngineError::Domain(
            DomainInvariantError::BlankValue("novelty evaluator version"),
        ));
    }
    if input.tracks.is_empty() {
        return Err(NoveltyEngineError::NoTracks);
    }
    if input.tracks.len() > MAX_TRACKS_PER_NOVELTY_EVALUATION {
        return Err(NoveltyEngineError::TooManyTracks {
            max: MAX_TRACKS_PER_NOVELTY_EVALUATION,
        });
    }
    for (dimension, count) in [
        ("exact byte hashes", input.catalog_exact_byte_hashes.len()),
        (
            "known AudioIdentities",
            input.catalog_known_audio_identities.len(),
        ),
        (
            "alternate AudioIdentities",
            input.catalog_alternate_audio_identities.len(),
        ),
    ] {
        if count > MAX_TRACKS_PER_NOVELTY_EVALUATION {
            return Err(NoveltyEngineError::TooManyCatalogFacts {
                dimension,
                max: MAX_TRACKS_PER_NOVELTY_EVALUATION,
            });
        }
    }
    if input
        .catalog_known_audio_identities
        .iter()
        .any(|id| input.catalog_alternate_audio_identities.contains(id))
    {
        return Err(NoveltyEngineError::ConflictingAudioIdentityFact);
    }
    if input
        .catalog_known_audio_identities
        .iter()
        .chain(input.catalog_alternate_audio_identities.iter())
        .any(|id| !id.is_uuid_v7())
    {
        return Err(NoveltyEngineError::Domain(
            DomainInvariantError::InvalidInternalId("AudioIdentityId"),
        ));
    }
    if input
        .catalog_exact_byte_hashes
        .iter()
        .any(|hash| !is_blake3(hash))
    {
        return Err(NoveltyEngineError::InvalidByteHash);
    }

    let mut track_ids = BTreeSet::new();
    for fact in &input.tracks {
        if fact.release_id != input.release_id {
            return Err(NoveltyEngineError::ForeignTrack {
                track_id: fact.track_id,
                release_id: input.release_id,
            });
        }
        if !track_ids.insert(fact.track_id) {
            return Err(NoveltyEngineError::DuplicateTrack(fact.track_id));
        }
        if !fact.track_id.is_uuid_v7()
            || !fact.audio_identity_id.is_uuid_v7()
            || fact.media_asset_id.is_some_and(|id| !id.is_uuid_v7())
        {
            return Err(NoveltyEngineError::Domain(
                DomainInvariantError::InvalidInternalId("novelty Track fact identity"),
            ));
        }
        if fact
            .byte_hash
            .as_deref()
            .is_some_and(|hash| !is_blake3(hash))
        {
            return Err(NoveltyEngineError::InvalidByteHash);
        }
    }
    Ok(())
}

fn derive_track_novelty(
    input: &NoveltyInput,
    fact: &NoveltyTrackFact,
) -> Result<
    (
        TrackNoveltyCategory,
        NoveltyEvidenceKind,
        Confidence,
        String,
    ),
    NoveltyEngineError,
> {
    if let Some(hash) = &fact.byte_hash
        && input.catalog_exact_byte_hashes.contains(hash)
    {
        return Ok((
            TrackNoveltyCategory::ExactDuplicate,
            NoveltyEvidenceKind::ByteHashMatch,
            Confidence::new(10_000)?,
            "Canonical byte hash exists in another Release".to_string(),
        ));
    }
    if input
        .catalog_known_audio_identities
        .contains(&fact.audio_identity_id)
    {
        return Ok((
            TrackNoveltyCategory::KnownAudioIdentity,
            NoveltyEvidenceKind::AudioIdentityMatch,
            Confidence::new(9_000)?,
            "AudioIdentity is durably referenced by another Release".to_string(),
        ));
    }
    if fact.audio_identity_confirmed
        && input
            .catalog_alternate_audio_identities
            .contains(&fact.audio_identity_id)
    {
        return Ok((
            TrackNoveltyCategory::AlternateRecording,
            NoveltyEvidenceKind::AudioIdentityDistinct,
            Confidence::new(8_500)?,
            "Catalog evidence confirms a distinct alternate recording".to_string(),
        ));
    }
    if fact.audio_identity_confirmed {
        return Ok((
            TrackNoveltyCategory::NewTrack,
            NoveltyEvidenceKind::NoCatalogMatch,
            Confidence::new(8_500)?,
            "Confirmed AudioIdentity has no catalog match".to_string(),
        ));
    }
    Ok((
        TrackNoveltyCategory::Uncertain,
        NoveltyEvidenceKind::InsufficientEvidence,
        Confidence::zero(),
        "AudioIdentity lacks durable fingerprint or match confirmation".to_string(),
    ))
}

fn count_for_category(
    rows: &[TrackNovelty],
    category: TrackNoveltyCategory,
) -> Result<u32, NoveltyEngineError> {
    u32::try_from(rows.iter().filter(|row| row.category == category).count()).map_err(|_| {
        NoveltyEngineError::TooManyTracks {
            max: MAX_TRACKS_PER_NOVELTY_EVALUATION,
        }
    })
}

fn is_blake3(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Validated novelty result. Fields are private so callers cannot synthesize a
/// destructive `Reissue` without passing complete Track facts through the engine.
#[derive(Debug, Clone)]
pub struct NoveltyAssessment {
    category: NoveltyCategory,
    evaluation: NoveltyEvaluation,
    track_novelty: Vec<TrackNovelty>,
}

impl NoveltyAssessment {
    #[must_use]
    pub const fn category(&self) -> NoveltyCategory {
        self.category
    }

    #[must_use]
    pub const fn evaluation(&self) -> &NoveltyEvaluation {
        &self.evaluation
    }

    #[must_use]
    pub fn track_novelty(&self) -> &[TrackNovelty] {
        &self.track_novelty
    }

    #[must_use]
    pub fn into_parts(self) -> (NoveltyCategory, NoveltyEvaluation, Vec<TrackNovelty>) {
        (self.category, self.evaluation, self.track_novelty)
    }
}
