use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable Melodee-owned Artist identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtistId(Uuid);

impl ArtistId {
    /// Wraps an existing UUID, including IDs imported from the legacy product.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the underlying UUID for persistence or protocol mapping.
    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

/// Identity-oriented Artist classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistType {
    Person,
    Group,
    Orchestra,
    Choir,
    Character,
    Other,
    Unknown,
}

/// A known Artist's relationship to a Track or Release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionRole {
    PrimaryArtist,
    FeaturedArtist,
    GuestArtist,
    Performer,
    GroupMember,
    Vocalist,
    Instrumentalist,
    Composer,
    Lyricist,
    Producer,
    Remixer,
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
                | Self::GroupMember
                | Self::Vocalist
                | Self::Instrumentalist
        )
    }
}
