use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::values::{validate_internal_id, validate_nonblank};
use crate::{
    ArtistAliasId, ArtistId, ArtistPublicId, ArtistRelationshipId, AudioIdentityId, Confidence,
    DateRange, DomainInvariantError, ExternalIdentityId, GroupMembershipId, ReleaseId, TrackId,
};

/// Identity-oriented Artist classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistType {
    Person,
    Group,
    Orchestra,
    Ensemble,
    Choir,
    Character,
    Other,
    Unknown,
}

/// Current lifecycle state of a Melodee-owned Artist identity.
///
/// `Active` is the normal state of an imported Artist: the catalogue knows the
/// name and the credits are usable. `Verified` records that an operator asserted
/// the identity itself. Neither state is a name-keyed identity claim; see
/// ADR-0190 for why the imported state is named for what it is (`active`) rather
/// than for what it lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistStatus {
    Active,
    Verified,
    MergedInto(ArtistId),
    Deprecated,
}

/// Artist creation data whose validation is independent of persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtist {
    pub id: ArtistId,
    pub public_api_id: ArtistPublicId,
    pub canonical_name: String,
    pub normalized_name: String,
    pub sort_name: String,
    pub artist_type: ArtistType,
    pub disambiguation: Option<String>,
    pub active_dates: Option<DateRange>,
    pub status: ArtistStatus,
    pub created_at: OffsetDateTime,
}

/// Stable Melodee-owned Artist aggregate. Names are attributes and are never identity keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artist {
    id: ArtistId,
    public_api_id: ArtistPublicId,
    canonical_name: String,
    normalized_name: String,
    sort_name: String,
    artist_type: ArtistType,
    disambiguation: Option<String>,
    active_dates: Option<DateRange>,
    status: ArtistStatus,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl Artist {
    pub fn new(value: NewArtist) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistId")?;
        validate_nonblank("canonical Artist name", &value.canonical_name)?;
        validate_nonblank("normalized Artist name", &value.normalized_name)?;
        validate_nonblank("Artist sort name", &value.sort_name)?;
        if value.disambiguation.as_deref().is_some_and(str::is_empty) {
            return Err(DomainInvariantError::BlankValue("Artist disambiguation"));
        }
        if let ArtistStatus::MergedInto(target) = value.status {
            validate_internal_id(target.is_uuid_v7(), "ArtistId")?;
            if target == value.id {
                return Err(DomainInvariantError::SelfMerge);
            }
        }
        Ok(Self {
            id: value.id,
            public_api_id: value.public_api_id,
            canonical_name: value.canonical_name,
            normalized_name: value.normalized_name,
            sort_name: value.sort_name,
            artist_type: value.artist_type,
            disambiguation: value.disambiguation,
            active_dates: value.active_dates,
            status: value.status,
            created_at: value.created_at,
            updated_at: value.created_at,
        })
    }

    #[must_use]
    pub const fn id(&self) -> ArtistId {
        self.id
    }

    #[must_use]
    pub const fn public_api_id(&self) -> ArtistPublicId {
        self.public_api_id
    }

    #[must_use]
    pub fn canonical_name(&self) -> &str {
        &self.canonical_name
    }

    #[must_use]
    pub fn normalized_name(&self) -> &str {
        &self.normalized_name
    }

    #[must_use]
    pub fn sort_name(&self) -> &str {
        &self.sort_name
    }

    #[must_use]
    pub const fn artist_type(&self) -> ArtistType {
        self.artist_type
    }

    #[must_use]
    pub fn disambiguation(&self) -> Option<&str> {
        self.disambiguation.as_deref()
    }

    #[must_use]
    pub const fn active_dates(&self) -> Option<DateRange> {
        self.active_dates
    }

    #[must_use]
    pub const fn status(&self) -> ArtistStatus {
        self.status
    }

    #[must_use]
    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    #[must_use]
    pub const fn updated_at(&self) -> OffsetDateTime {
        self.updated_at
    }
}

/// Classification of another name for the same Artist identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistAliasType {
    Search,
    LegalName,
    FormerName,
    Translation,
    Transliteration,
    Other,
}

/// Another name for the same Artist; never a collaboration or membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistAlias {
    pub id: ArtistAliasId,
    pub artist_id: ArtistId,
    pub value: String,
    pub normalized_value: String,
    pub alias_type: ArtistAliasType,
    pub locale: Option<String>,
    pub evidence_source: String,
    pub confidence: Confidence,
    pub locked: bool,
}

impl ArtistAlias {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistAliasId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        validate_nonblank("Artist alias", &value.value)?;
        validate_nonblank("normalized Artist alias", &value.normalized_value)?;
        validate_nonblank("alias evidence source", &value.evidence_source)?;
        Ok(value)
    }
}

/// Semantic owner of normalized provider evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalIdentityOwner {
    Artist(ArtistId),
    Release(ReleaseId),
    AudioIdentity(AudioIdentityId),
}

/// Provider identifier used as evidence, never as Melodee's semantic identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalIdentity {
    pub id: ExternalIdentityId,
    pub owner: ExternalIdentityOwner,
    pub provider: String,
    pub external_id: String,
    pub confidence: Confidence,
    pub evidence_source: String,
    pub verified_at: Option<OffsetDateTime>,
    pub locked: bool,
    pub provider_guarantees_uniqueness: bool,
}

impl ExternalIdentity {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ExternalIdentityId")?;
        match value.owner {
            ExternalIdentityOwner::Artist(id) => {
                validate_internal_id(id.is_uuid_v7(), "ArtistId")?;
            }
            ExternalIdentityOwner::Release(id) => {
                validate_internal_id(id.is_uuid_v7(), "ReleaseId")?;
            }
            ExternalIdentityOwner::AudioIdentity(id) => {
                validate_internal_id(id.is_uuid_v7(), "AudioIdentityId")?;
            }
        }
        validate_nonblank("external identity provider", &value.provider)?;
        validate_nonblank("external provider identifier", &value.external_id)?;
        validate_nonblank("external identity evidence source", &value.evidence_source)?;
        Ok(value)
    }
}

/// Open relationship code between two distinct Artist identities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtistRelationshipKind(String);

impl ArtistRelationshipKind {
    pub fn new(value: String) -> Result<Self, DomainInvariantError> {
        validate_nonblank("Artist relationship kind", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Evidence-backed relationship that does not imply identity equality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtistRelationship {
    pub id: ArtistRelationshipId,
    pub subject_artist_id: ArtistId,
    pub object_artist_id: ArtistId,
    pub kind: ArtistRelationshipKind,
    pub active_dates: Option<DateRange>,
    pub confidence: Confidence,
    pub evidence_source: String,
}

impl ArtistRelationship {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "ArtistRelationshipId")?;
        validate_internal_id(value.subject_artist_id.is_uuid_v7(), "ArtistId")?;
        validate_internal_id(value.object_artist_id.is_uuid_v7(), "ArtistId")?;
        if value.subject_artist_id == value.object_artist_id {
            return Err(DomainInvariantError::SelfArtistRelationship);
        }
        validate_nonblank("relationship evidence source", &value.evidence_source)?;
        Ok(value)
    }
}

/// Role of a person/identity within a group during a membership interval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MembershipRole(String);

impl MembershipRole {
    pub fn new(value: String) -> Result<Self, DomainInvariantError> {
        validate_nonblank("group membership role", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Date-bounded, evidence-backed membership between distinct Artist identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupMembership {
    pub id: GroupMembershipId,
    pub member_artist_id: ArtistId,
    pub group_artist_id: ArtistId,
    pub active_dates: DateRange,
    pub role: MembershipRole,
    pub confidence: Confidence,
    pub evidence_source: String,
    pub excluded_tracks: BTreeSet<TrackId>,
}

impl GroupMembership {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "GroupMembershipId")?;
        validate_internal_id(value.member_artist_id.is_uuid_v7(), "ArtistId")?;
        validate_internal_id(value.group_artist_id.is_uuid_v7(), "ArtistId")?;
        for track_id in &value.excluded_tracks {
            validate_internal_id(track_id.is_uuid_v7(), "TrackId")?;
        }
        if value.member_artist_id == value.group_artist_id {
            return Err(DomainInvariantError::SelfGroupMembership);
        }
        validate_nonblank("membership evidence source", &value.evidence_source)?;
        Ok(value)
    }

    /// Unknown dates and explicit exclusions deliberately fail closed.
    #[must_use]
    pub fn supports_inference(&self, track_id: TrackId, date: Option<time::Date>) -> bool {
        !self.excluded_tracks.contains(&track_id) && self.active_dates.contains_known(date)
    }
}
