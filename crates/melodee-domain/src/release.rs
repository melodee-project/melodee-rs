use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::values::{is_blake3, validate_internal_id, validate_nonblank};
use crate::{
    ArtistId, AudioIdentityId, AudioIdentityMergeId, AudioIdentitySplitId, DisplayArtist,
    DomainInvariantError, MediaAssetId, ReleaseId, ReleasePublicId, TrackId, TrackPublicId,
};

impl ReleaseId {
    /// Reserves the catalog Release identity during Phase 3 publication.
    ///
    /// Phase 4 catalog construction must adopt this existing ID rather than call this method.
    #[must_use]
    pub fn reserve_for_publication() -> Self {
        Self::new()
    }
}

/// Non-exclusive classification inferred for a Release in Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTrait {
    Studio,
    Ep,
    Single,
    Live,
    Compilation,
    GreatestHits,
    Collection,
    BoxSet,
    Bootleg,
    Demo,
    Promo,
    Broadcast,
    RadioShow,
    DjMix,
    Remix,
    Tribute,
    Soundtrack,
    CastRecording,
    SpokenWord,
    Comedy,
    Audiobook,
    Reissue,
    Remaster,
    Other,
    Unknown,
}

/// Catalog Release created from an already-published manifest or migrated legacy row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    id: ReleaseId,
    public_api_id: ReleasePublicId,
    title: String,
    display_artist: DisplayArtist,
    release_date: Option<Date>,
    created_at: OffsetDateTime,
}

impl Release {
    pub fn from_published_manifest(
        id: ReleaseId,
        public_api_id: ReleasePublicId,
        title: String,
        display_artist: DisplayArtist,
        release_date: Option<Date>,
        created_at: OffsetDateTime,
    ) -> Result<Self, DomainInvariantError> {
        validate_internal_id(id.is_uuid_v7(), "ReleaseId")?;
        validate_nonblank("Release title", &title)?;
        Ok(Self {
            id,
            public_api_id,
            title,
            display_artist,
            release_date,
            created_at,
        })
    }

    #[must_use]
    pub const fn id(&self) -> ReleaseId {
        self.id
    }

    #[must_use]
    pub const fn public_api_id(&self) -> ReleasePublicId {
        self.public_api_id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub const fn display_artist(&self) -> &DisplayArtist {
        &self.display_artist
    }

    #[must_use]
    pub const fn release_date(&self) -> Option<Date> {
        self.release_date
    }

    #[must_use]
    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }
}

/// Relationship role between a Track occurrence and encoded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAssetRole {
    Playable,
    Alternate,
}

/// Many-to-many link between a Track occurrence and a MediaAsset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackMediaAsset {
    pub track_id: TrackId,
    pub media_asset_id: MediaAssetId,
    pub role: MediaAssetRole,
    pub ordinal: u32,
}

impl TrackMediaAsset {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.track_id.is_uuid_v7(), "TrackId")?;
        validate_internal_id(value.media_asset_id.is_uuid_v7(), "MediaAssetId")?;
        if value.ordinal == 0 {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
        Ok(value)
    }
}

/// One ordered playable occurrence on one Release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
    id: TrackId,
    public_api_id: TrackPublicId,
    release_id: ReleaseId,
    audio_identity_id: AudioIdentityId,
    ordinal: u32,
    title: String,
    display_artist: DisplayArtist,
    observed_disc_number: Option<u32>,
    observed_track_number: Option<u32>,
    media_assets: Vec<TrackMediaAsset>,
}

/// Validated Track creation data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrack {
    pub id: TrackId,
    pub public_api_id: TrackPublicId,
    pub release_id: ReleaseId,
    pub audio_identity_id: AudioIdentityId,
    pub ordinal: u32,
    pub title: String,
    pub display_artist: DisplayArtist,
    pub observed_disc_number: Option<u32>,
    pub observed_track_number: Option<u32>,
    pub media_assets: Vec<TrackMediaAsset>,
}

impl Track {
    pub fn new(value: NewTrack) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "TrackId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.audio_identity_id.is_uuid_v7(), "AudioIdentityId")?;
        if value.ordinal == 0 {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
        validate_nonblank("Track title", &value.title)?;
        if value.media_assets.iter().any(|link| link.ordinal == 0) {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
        for link in &value.media_assets {
            validate_internal_id(link.track_id.is_uuid_v7(), "TrackId")?;
            validate_internal_id(link.media_asset_id.is_uuid_v7(), "MediaAssetId")?;
        }
        if value
            .media_assets
            .iter()
            .any(|link| link.track_id != value.id)
        {
            return Err(DomainInvariantError::ForeignTrackMediaAsset);
        }
        if !value
            .media_assets
            .iter()
            .any(|link| link.role == MediaAssetRole::Playable)
        {
            return Err(DomainInvariantError::MissingPlayableMediaAsset);
        }
        Ok(Self {
            id: value.id,
            public_api_id: value.public_api_id,
            release_id: value.release_id,
            audio_identity_id: value.audio_identity_id,
            ordinal: value.ordinal,
            title: value.title,
            display_artist: value.display_artist,
            observed_disc_number: value.observed_disc_number,
            observed_track_number: value.observed_track_number,
            media_assets: value.media_assets,
        })
    }

    #[must_use]
    pub const fn id(&self) -> TrackId {
        self.id
    }

    #[must_use]
    pub const fn public_api_id(&self) -> TrackPublicId {
        self.public_api_id
    }

    #[must_use]
    pub const fn release_id(&self) -> ReleaseId {
        self.release_id
    }

    #[must_use]
    pub const fn audio_identity_id(&self) -> AudioIdentityId {
        self.audio_identity_id
    }

    #[must_use]
    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub const fn display_artist(&self) -> &DisplayArtist {
        &self.display_artist
    }

    #[must_use]
    pub const fn observed_disc_number(&self) -> Option<u32> {
        self.observed_disc_number
    }

    #[must_use]
    pub const fn observed_track_number(&self) -> Option<u32> {
        self.observed_track_number
    }

    #[must_use]
    pub fn media_assets(&self) -> &[TrackMediaAsset] {
        &self.media_assets
    }
}

/// Canonical encoded-byte asset. Hash and path are attributes, not semantic identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaAsset {
    id: MediaAssetId,
    canonical_path: String,
    byte_hash: String,
    size_bytes: u64,
}

impl MediaAsset {
    pub fn new(
        id: MediaAssetId,
        canonical_path: String,
        byte_hash: String,
        size_bytes: u64,
    ) -> Result<Self, DomainInvariantError> {
        validate_internal_id(id.is_uuid_v7(), "MediaAssetId")?;
        validate_nonblank("canonical MediaAsset path", &canonical_path)
            .map_err(|_| DomainInvariantError::InvalidCanonicalPath)?;
        if !is_blake3(&byte_hash) {
            return Err(DomainInvariantError::InvalidBlake3);
        }
        Ok(Self {
            id,
            canonical_path,
            byte_hash,
            size_bytes,
        })
    }

    #[must_use]
    pub const fn id(&self) -> MediaAssetId {
        self.id
    }

    #[must_use]
    pub fn canonical_path(&self) -> &str {
        &self.canonical_path
    }

    #[must_use]
    pub fn byte_hash(&self) -> &str {
        &self.byte_hash
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// Current lifecycle state of an underlying audible recording/performance identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioIdentityStatus {
    Provisional,
    Verified,
    MergedInto(AudioIdentityId),
    Deprecated,
}

/// Melodee-owned identity distinct from Track occurrences, bytes, and fingerprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudioIdentity {
    id: AudioIdentityId,
    status: AudioIdentityStatus,
}

impl AudioIdentity {
    pub fn new(
        id: AudioIdentityId,
        status: AudioIdentityStatus,
    ) -> Result<Self, DomainInvariantError> {
        validate_internal_id(id.is_uuid_v7(), "AudioIdentityId")?;
        if let AudioIdentityStatus::MergedInto(target) = status {
            validate_internal_id(target.is_uuid_v7(), "AudioIdentityId")?;
            if target == id {
                return Err(DomainInvariantError::SelfMerge);
            }
        }
        Ok(Self { id, status })
    }

    #[must_use]
    pub const fn id(self) -> AudioIdentityId {
        self.id
    }

    #[must_use]
    pub const fn status(self) -> AudioIdentityStatus {
        self.status
    }
}

/// Append-only correction event for equivalent AudioIdentities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioIdentityMerge {
    pub id: AudioIdentityMergeId,
    pub source_id: AudioIdentityId,
    pub target_id: AudioIdentityId,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

impl AudioIdentityMerge {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "AudioIdentityMergeId")?;
        validate_internal_id(value.source_id.is_uuid_v7(), "AudioIdentityId")?;
        validate_internal_id(value.target_id.is_uuid_v7(), "AudioIdentityId")?;
        if value.source_id == value.target_id {
            return Err(DomainInvariantError::SelfMerge);
        }
        validate_nonblank("AudioIdentity merge reason", &value.reason)?;
        Ok(value)
    }
}

/// Append-only correction event moving selected Track occurrences to a new AudioIdentity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioIdentitySplit {
    pub id: AudioIdentitySplitId,
    pub source_id: AudioIdentityId,
    pub new_identity_id: AudioIdentityId,
    pub track_ids: BTreeSet<TrackId>,
    pub reason: String,
    pub occurred_at: OffsetDateTime,
}

impl AudioIdentitySplit {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "AudioIdentitySplitId")?;
        validate_internal_id(value.source_id.is_uuid_v7(), "AudioIdentityId")?;
        validate_internal_id(value.new_identity_id.is_uuid_v7(), "AudioIdentityId")?;
        for track_id in &value.track_ids {
            validate_internal_id(track_id.is_uuid_v7(), "TrackId")?;
        }
        if value.source_id == value.new_identity_id || value.track_ids.is_empty() {
            return Err(DomainInvariantError::EmptyOrSelfSplit);
        }
        validate_nonblank("AudioIdentity split reason", &value.reason)?;
        Ok(value)
    }
}

/// Validates importer-assigned Track ordinals without treating observed numbers as keys.
pub fn validate_release_track_ordinals(tracks: &[Track]) -> Result<(), DomainInvariantError> {
    let mut seen = BTreeSet::new();
    for track in tracks {
        if !seen.insert((track.release_id(), track.ordinal())) {
            return Err(DomainInvariantError::InvalidOrdinal);
        }
    }
    Ok(())
}

/// Artist participation directly attached to an AudioIdentity remains a catalog relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioIdentityArtist {
    pub audio_identity_id: AudioIdentityId,
    pub artist_id: ArtistId,
}

impl AudioIdentityArtist {
    pub fn new(value: Self) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.audio_identity_id.is_uuid_v7(), "AudioIdentityId")?;
        validate_internal_id(value.artist_id.is_uuid_v7(), "ArtistId")?;
        Ok(value)
    }
}
