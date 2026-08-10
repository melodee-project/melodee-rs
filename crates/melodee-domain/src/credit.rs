use serde::{Deserialize, Serialize};

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{
    ArtistId, ContributionId, DomainInvariantError, GroupMembershipId, ReleaseArtistId, ReleaseId,
    TrackArtistId, TrackId,
};

/// Exact human-facing billing text. The stored value is never trimmed or reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DisplayArtist(String);

impl DisplayArtist {
    pub fn new(value: String) -> Result<Self, DomainInvariantError> {
        validate_nonblank("DisplayArtist", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DisplayArtist {
    type Error = DomainInvariantError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DisplayArtist> for String {
    fn from(value: DisplayArtist) -> Self {
        value.0
    }
}

/// Presentation role on a Release or Track, distinct from participation credits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationRole {
    Primary,
    Featured,
    Guest,
    Other,
}

/// Ordered structured Release billing association.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseArtist {
    pub id: ReleaseArtistId,
    pub release_id: ReleaseId,
    pub artist_id: ArtistId,
    pub role: PresentationRole,
    pub presentation_order: u32,
    pub join_phrase: String,
}

impl ReleaseArtist {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ReleaseArtistId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        if value.presentation_order == 0 {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
        Ok(value)
    }
}

/// Ordered structured Track billing association.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrackArtist {
    pub id: TrackArtistId,
    pub track_id: TrackId,
    pub artist_id: ArtistId,
    pub role: PresentationRole,
    pub presentation_order: u32,
    pub join_phrase: String,
}

impl TrackArtist {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "TrackArtistId")?;
        validate_internal_id(value.track_id.is_uuid_v7(), "TrackId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        if value.presentation_order == 0 {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
        Ok(value)
    }
}

/// A known Artist's participation or credit on a Track or Release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionRole {
    PrimaryArtist,
    FeaturedArtist,
    GuestArtist,
    Performer,
    BandMember,
    Vocalist,
    Instrumentalist,
    Composer,
    Lyricist,
    Producer,
    Remixer,
    Engineer,
    Other,
}

impl ContributionRole {
    /// Roles included by the default “everything this Artist is on” query.
    #[must_use]
    pub const fn is_performance_role(self) -> bool {
        matches!(
            self,
            Self::PrimaryArtist
                | Self::FeaturedArtist
                | Self::GuestArtist
                | Self::Performer
                | Self::BandMember
                | Self::Vocalist
                | Self::Instrumentalist
        )
    }
}

/// Semantic owner of a contribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionTarget {
    Release(ReleaseId),
    Track(TrackId),
}

/// Directly evidenced participation or credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contribution {
    pub id: ContributionId,
    pub target: ContributionTarget,
    pub artist_id: ArtistId,
    pub role: ContributionRole,
    pub credit_text: Option<String>,
    pub evidence_source: String,
}

impl Contribution {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ContributionId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        match value.target {
            ContributionTarget::Release(id) => {
                validate_internal_id(id.is_uuid_v7(), "ReleaseId")?;
            }
            ContributionTarget::Track(id) => {
                validate_internal_id(id.is_uuid_v7(), "TrackId")?;
            }
        }
        validate_nonblank("contribution evidence source", &value.evidence_source)?;
        if value.credit_text.as_deref().is_some_and(str::is_empty) {
            return Err(DomainInvariantError::BlankValue("contribution credit text"));
        }
        Ok(value)
    }
}

/// Whether an Artist-track association is explicit or safely inferred through a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipationKind {
    Direct,
    InferredGroupParticipation {
        source_group_id: ArtistId,
        membership_id: GroupMembershipId,
    },
}

/// Rebuildable participation projection used by “everything this Artist is on.”
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtistTrackAssociation {
    pub artist_id: ArtistId,
    pub track_id: TrackId,
    pub role: ContributionRole,
    pub participation: ParticipationKind,
}

impl ArtistTrackAssociation {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        validate_internal_id(value.track_id.is_uuid_v7(), "TrackId")?;
        if let ParticipationKind::InferredGroupParticipation {
            source_group_id,
            membership_id,
        } = value.participation
        {
            validate_internal_id(source_group_id.is_uuid_v7(), "ArtistId")?;
            validate_internal_id(membership_id.is_uuid_v7(), "GroupMembershipId")?;
        }
        Ok(value)
    }

    #[must_use]
    pub const fn included_in_default_appears_on(self) -> bool {
        self.role.is_performance_role()
    }
}

/// Checks stable presentation order without attempting to recreate exact display text.
pub fn validate_release_artist_order(values: &[ReleaseArtist]) -> Result<(), DomainInvariantError> {
    let mut seen = std::collections::BTreeSet::new();
    if values
        .iter()
        .any(|value| !seen.insert((value.release_id, value.presentation_order)))
    {
        Err(DomainInvariantError::InvalidOrdinal)
    } else {
        Ok(())
    }
}

/// Checks stable presentation order without treating the display string as identity.
pub fn validate_track_artist_order(values: &[TrackArtist]) -> Result<(), DomainInvariantError> {
    let mut seen = std::collections::BTreeSet::new();
    if values
        .iter()
        .any(|value| !seen.insert((value.track_id, value.presentation_order)))
    {
        Err(DomainInvariantError::InvalidOrdinal)
    } else {
        Ok(())
    }
}
