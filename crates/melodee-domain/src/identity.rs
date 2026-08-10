use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{
    ArtistAliasId, ArtistId, ArtistMatchDecisionId, ArtistMatchEvidenceId, ArtistMergeId,
    ArtistNegativeAssertionId, ArtistPublicId, ArtistSplitId, AudioIdentityDecisionId,
    AudioIdentityEvidenceId, AudioIdentityId, Confidence, ContributionId, DomainInvariantError,
    ExternalIdentityId, ObservedArtistId, ReleaseArtistId, ReleaseId, TrackArtistId, TrackId,
};

/// Raw artist-like input and its source context before identity resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservedArtist {
    pub id: ObservedArtistId,
    pub release_id: ReleaseId,
    pub track_id: Option<TrackId>,
    pub raw_value: String,
    pub normalized_value: String,
    pub source_file: Option<String>,
    pub source_field: String,
}

impl ObservedArtist {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ObservedArtistId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        if let Some(track_id) = value.track_id {
            validate_internal_id(track_id.is_uuid_v7(), "TrackId")?;
        }
        validate_nonblank("observed Artist raw value", &value.raw_value)?;
        validate_nonblank("observed Artist normalized value", &value.normalized_value)?;
        validate_nonblank("observed Artist source field", &value.source_field)?;
        Ok(value)
    }
}

/// Candidate text extracted from billing syntax. It intentionally cannot carry an ArtistId.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedArtistCandidate {
    pub raw_value: String,
    pub join_phrase: String,
}

impl ParsedArtistCandidate {
    pub fn new(raw_value: String, join_phrase: String) -> Result<Self, DomainInvariantError> {
        validate_nonblank("parsed Artist candidate", &raw_value)?;
        Ok(Self {
            raw_value,
            join_phrase,
        })
    }
}

/// Evidence dimension considered while resolving an Artist observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    NameSimilarity,
    Alias,
    EmbeddedExternalId,
    ReleaseContext,
    TrackParticipation,
    ArtistType,
    ActiveDate,
    Area,
    GroupMembership,
    ExternalProviderAgreement,
    Manual,
}

/// Whether evidence supports or conflicts with one candidate identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePolarity {
    Supports,
    Conflicts,
}

/// One immutable evidence fact for an observation/candidate pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistMatchEvidence {
    pub id: ArtistMatchEvidenceId,
    pub observed_artist_id: ObservedArtistId,
    pub candidate_artist_id: ArtistId,
    pub kind: EvidenceKind,
    pub polarity: EvidencePolarity,
    pub confidence: Confidence,
    pub source: String,
}

impl ArtistMatchEvidence {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistMatchEvidenceId")?;
        validate_internal_id(value.observed_artist_id.is_uuid_v7(), "ObservedArtistId")?;
        validate_internal_id(value.candidate_artist_id.is_uuid_v7(), "ArtistId")?;
        validate_nonblank("Artist match evidence source", &value.source)?;
        Ok(value)
    }
}

/// Authority responsible for a durable identity decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAuthority {
    Automated { resolver_version: String },
    Administrator { actor: String },
}

impl DecisionAuthority {
    fn validate(&self) -> Result<(), DomainInvariantError> {
        match self {
            Self::Automated { resolver_version } => {
                validate_nonblank("identity resolver version", resolver_version)
            }
            Self::Administrator { actor } => validate_nonblank("identity decision actor", actor),
        }
    }

    #[must_use]
    pub const fn is_administrator(&self) -> bool {
        matches!(self, Self::Administrator { .. })
    }
}

/// Positive decision state. Rejection is represented by ArtistNegativeAssertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistMatchOutcome {
    Proposed,
    Confirmed,
}

/// Input for a validated Artist match decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtistMatchDecision {
    pub id: ArtistMatchDecisionId,
    pub observed_artist_id: ObservedArtistId,
    pub artist_id: ArtistId,
    pub outcome: ArtistMatchOutcome,
    pub authority: DecisionAuthority,
    pub locked: bool,
    pub reason: String,
    pub decided_at: OffsetDateTime,
}

/// Durable proposed or confirmed resolution of an observation to an Artist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistMatchDecision {
    id: ArtistMatchDecisionId,
    observed_artist_id: ObservedArtistId,
    artist_id: ArtistId,
    outcome: ArtistMatchOutcome,
    authority: DecisionAuthority,
    locked: bool,
    reason: String,
    decided_at: OffsetDateTime,
    evidence_ids: Vec<ArtistMatchEvidenceId>,
}

impl ArtistMatchDecision {
    pub fn new(
        value: NewArtistMatchDecision,
        evidence: &[ArtistMatchEvidence],
    ) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistMatchDecisionId")?;
        validate_internal_id(value.observed_artist_id.is_uuid_v7(), "ObservedArtistId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        value.authority.validate()?;
        validate_nonblank("Artist match decision reason", &value.reason)?;
        if value.locked && !value.authority.is_administrator() {
            return Err(DomainInvariantError::InvalidDecisionLock);
        }
        if evidence.iter().any(|item| {
            item.observed_artist_id != value.observed_artist_id
                || item.candidate_artist_id != value.artist_id
        }) {
            return Err(DomainInvariantError::ForeignMatchEvidence);
        }
        for item in evidence {
            validate_internal_id(item.id.is_uuid_v7(), "ArtistMatchEvidenceId")?;
        }
        if value.outcome == ArtistMatchOutcome::Confirmed && !value.authority.is_administrator() {
            if evidence
                .iter()
                .any(|item| item.polarity == EvidencePolarity::Conflicts)
            {
                return Err(DomainInvariantError::ConflictingMatchEvidence);
            }
            let has_non_name_support = evidence.iter().any(|item| {
                item.polarity == EvidencePolarity::Supports
                    && item.kind != EvidenceKind::NameSimilarity
            });
            if !has_non_name_support {
                return Err(DomainInvariantError::AutomaticNameOnlyMatch);
            }
        }
        let evidence_ids = evidence.iter().map(|item| item.id).collect();
        Ok(Self {
            id: value.id,
            observed_artist_id: value.observed_artist_id,
            artist_id: value.artist_id,
            outcome: value.outcome,
            authority: value.authority,
            locked: value.locked,
            reason: value.reason,
            decided_at: value.decided_at,
            evidence_ids,
        })
    }

    #[must_use]
    pub const fn id(&self) -> ArtistMatchDecisionId {
        self.id
    }

    #[must_use]
    pub const fn observed_artist_id(&self) -> ObservedArtistId {
        self.observed_artist_id
    }

    #[must_use]
    pub const fn artist_id(&self) -> ArtistId {
        self.artist_id
    }

    #[must_use]
    pub const fn outcome(&self) -> ArtistMatchOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn locked(&self) -> bool {
        self.locked
    }

    #[must_use]
    pub fn authority(&self) -> &DecisionAuthority {
        &self.authority
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub const fn decided_at(&self) -> OffsetDateTime {
        self.decided_at
    }

    #[must_use]
    pub fn evidence_ids(&self) -> &[ArtistMatchEvidenceId] {
        &self.evidence_ids
    }

    /// Locked manual decisions may be superseded only by another administrator decision.
    #[must_use]
    pub fn can_be_superseded_by(&self, authority: &DecisionAuthority) -> bool {
        !self.locked || authority.is_administrator()
    }
}

/// Durable do-not-match assertion preventing repeated unsafe suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistNegativeAssertion {
    pub id: ArtistNegativeAssertionId,
    pub observed_artist_id: ObservedArtistId,
    pub rejected_artist_id: ArtistId,
    pub authority: DecisionAuthority,
    pub locked: bool,
    pub reason: String,
    pub decided_at: OffsetDateTime,
}

impl ArtistNegativeAssertion {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistNegativeAssertionId")?;
        validate_internal_id(value.observed_artist_id.is_uuid_v7(), "ObservedArtistId")?;
        validate_internal_id(value.rejected_artist_id.is_uuid_v7(), "ArtistId")?;
        value.authority.validate()?;
        validate_nonblank("negative assertion reason", &value.reason)?;
        if value.locked && !value.authority.is_administrator() {
            return Err(DomainInvariantError::InvalidDecisionLock);
        }
        Ok(value)
    }

    #[must_use]
    pub fn prevents_suggestion(
        &self,
        observed_artist_id: ObservedArtistId,
        artist_id: ArtistId,
    ) -> bool {
        self.observed_artist_id == observed_artist_id && self.rejected_artist_id == artist_id
    }
}

/// Append-only Artist merge event retaining the source public ID as a redirect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistMerge {
    pub id: ArtistMergeId,
    pub source_artist_id: ArtistId,
    pub source_public_api_id: ArtistPublicId,
    pub target_artist_id: ArtistId,
    pub actor: String,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

impl ArtistMerge {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistMergeId")?;
        validate_internal_id(value.source_artist_id.is_uuid_v7(), "ArtistId")?;
        validate_internal_id(value.target_artist_id.is_uuid_v7(), "ArtistId")?;
        if value.source_artist_id == value.target_artist_id {
            return Err(DomainInvariantError::SelfMerge);
        }
        validate_nonblank("Artist merge actor", &value.actor)?;
        validate_nonblank("Artist merge reason", &value.reason)?;
        Ok(value)
    }

    #[must_use]
    pub const fn tombstone_status(&self) -> crate::ArtistStatus {
        crate::ArtistStatus::MergedInto(self.target_artist_id)
    }
}

/// One explicitly selected relationship that may move during an Artist split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAssignment {
    ReleaseArtist(ReleaseArtistId),
    TrackArtist(TrackArtistId),
    Contribution(ContributionId),
    Alias(ArtistAliasId),
    ExternalIdentity(ExternalIdentityId),
}

/// Append-only Artist split event. MediaAsset locations are deliberately unrepresentable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistSplit {
    pub id: ArtistSplitId,
    pub source_artist_id: ArtistId,
    pub new_artist_id: ArtistId,
    pub assignments: Vec<SplitAssignment>,
    pub actor: String,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

/// Evidence dimension considered while resolving underlying audible identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioIdentityEvidenceKind {
    ByteHash,
    AcousticFingerprint,
    Duration,
    Metadata,
    ExternalIdentifier,
    CatalogContext,
    Manual,
}

/// One immutable comparison between distinct provisional AudioIdentities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioIdentityEvidence {
    pub id: AudioIdentityEvidenceId,
    pub source_audio_identity_id: AudioIdentityId,
    pub candidate_audio_identity_id: AudioIdentityId,
    pub kind: AudioIdentityEvidenceKind,
    pub polarity: EvidencePolarity,
    pub confidence: Confidence,
    pub source: String,
}

impl AudioIdentityEvidence {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "AudioIdentityEvidenceId")?;
        validate_internal_id(
            value.source_audio_identity_id.is_uuid_v7(),
            "AudioIdentityId",
        )?;
        validate_internal_id(
            value.candidate_audio_identity_id.is_uuid_v7(),
            "AudioIdentityId",
        )?;
        if value.source_audio_identity_id == value.candidate_audio_identity_id {
            return Err(DomainInvariantError::SelfAudioIdentityEvidence);
        }
        validate_nonblank("AudioIdentity evidence source", &value.source)?;
        Ok(value)
    }
}

/// Resolution state for two provisional underlying-audio identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioIdentityMatchOutcome {
    Proposed,
    Confirmed,
}

/// Input for a validated underlying-audio identity decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAudioIdentityMatchDecision {
    pub id: AudioIdentityDecisionId,
    pub source_audio_identity_id: AudioIdentityId,
    pub candidate_audio_identity_id: AudioIdentityId,
    pub outcome: AudioIdentityMatchOutcome,
    pub authority: DecisionAuthority,
    pub locked: bool,
    pub reason: String,
    pub decided_at: OffsetDateTime,
}

/// Durable AudioIdentity decision. Fingerprints, hashes, or metadata never decide alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioIdentityMatchDecision {
    id: AudioIdentityDecisionId,
    source_audio_identity_id: AudioIdentityId,
    candidate_audio_identity_id: AudioIdentityId,
    outcome: AudioIdentityMatchOutcome,
    authority: DecisionAuthority,
    locked: bool,
    reason: String,
    decided_at: OffsetDateTime,
    evidence_ids: Vec<AudioIdentityEvidenceId>,
}

impl AudioIdentityMatchDecision {
    pub fn new(
        value: NewAudioIdentityMatchDecision,
        evidence: &[AudioIdentityEvidence],
    ) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "AudioIdentityDecisionId")?;
        validate_internal_id(
            value.source_audio_identity_id.is_uuid_v7(),
            "AudioIdentityId",
        )?;
        validate_internal_id(
            value.candidate_audio_identity_id.is_uuid_v7(),
            "AudioIdentityId",
        )?;
        value.authority.validate()?;
        validate_nonblank("AudioIdentity decision reason", &value.reason)?;
        if value.source_audio_identity_id == value.candidate_audio_identity_id {
            return Err(DomainInvariantError::SelfAudioIdentityEvidence);
        }
        if value.locked && !value.authority.is_administrator() {
            return Err(DomainInvariantError::InvalidDecisionLock);
        }
        if evidence.iter().any(|item| {
            item.source_audio_identity_id != value.source_audio_identity_id
                || item.candidate_audio_identity_id != value.candidate_audio_identity_id
        }) {
            return Err(DomainInvariantError::ForeignAudioIdentityEvidence);
        }
        for item in evidence {
            validate_internal_id(item.id.is_uuid_v7(), "AudioIdentityEvidenceId")?;
        }
        if value.outcome == AudioIdentityMatchOutcome::Confirmed
            && !value.authority.is_administrator()
        {
            if evidence
                .iter()
                .any(|item| item.polarity == EvidencePolarity::Conflicts)
            {
                return Err(DomainInvariantError::ConflictingMatchEvidence);
            }
            let independent_support: BTreeSet<_> = evidence
                .iter()
                .filter(|item| item.polarity == EvidencePolarity::Supports)
                .map(|item| item.kind)
                .collect();
            if independent_support.len() < 2 {
                return Err(DomainInvariantError::InsufficientAudioIdentityEvidence);
            }
        }
        Ok(Self {
            id: value.id,
            source_audio_identity_id: value.source_audio_identity_id,
            candidate_audio_identity_id: value.candidate_audio_identity_id,
            outcome: value.outcome,
            authority: value.authority,
            locked: value.locked,
            reason: value.reason,
            decided_at: value.decided_at,
            evidence_ids: evidence.iter().map(|item| item.id).collect(),
        })
    }

    #[must_use]
    pub const fn id(&self) -> AudioIdentityDecisionId {
        self.id
    }

    #[must_use]
    pub const fn source_audio_identity_id(&self) -> AudioIdentityId {
        self.source_audio_identity_id
    }

    #[must_use]
    pub const fn candidate_audio_identity_id(&self) -> AudioIdentityId {
        self.candidate_audio_identity_id
    }

    #[must_use]
    pub const fn outcome(&self) -> AudioIdentityMatchOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn locked(&self) -> bool {
        self.locked
    }

    #[must_use]
    pub fn authority(&self) -> &DecisionAuthority {
        &self.authority
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub const fn decided_at(&self) -> OffsetDateTime {
        self.decided_at
    }

    #[must_use]
    pub fn evidence_ids(&self) -> &[AudioIdentityEvidenceId] {
        &self.evidence_ids
    }
}

impl ArtistSplit {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistSplitId")?;
        validate_internal_id(value.source_artist_id.is_uuid_v7(), "ArtistId")?;
        validate_internal_id(value.new_artist_id.is_uuid_v7(), "ArtistId")?;
        for assignment in &value.assignments {
            match assignment {
                SplitAssignment::ReleaseArtist(id) => {
                    validate_internal_id(id.is_uuid_v7(), "ReleaseArtistId")?;
                }
                SplitAssignment::TrackArtist(id) => {
                    validate_internal_id(id.is_uuid_v7(), "TrackArtistId")?;
                }
                SplitAssignment::Contribution(id) => {
                    validate_internal_id(id.is_uuid_v7(), "ContributionId")?;
                }
                SplitAssignment::Alias(id) => {
                    validate_internal_id(id.is_uuid_v7(), "ArtistAliasId")?;
                }
                SplitAssignment::ExternalIdentity(id) => {
                    validate_internal_id(id.is_uuid_v7(), "ExternalIdentityId")?;
                }
            }
        }
        if value.source_artist_id == value.new_artist_id || value.assignments.is_empty() {
            return Err(DomainInvariantError::EmptyOrSelfSplit);
        }
        let unique: BTreeSet<_> = value.assignments.iter().copied().collect();
        if unique.len() != value.assignments.len() {
            return Err(DomainInvariantError::DuplicateSplitAssignment);
        }
        validate_nonblank("Artist split actor", &value.actor)?;
        validate_nonblank("Artist split reason", &value.reason)?;
        Ok(value)
    }
}
