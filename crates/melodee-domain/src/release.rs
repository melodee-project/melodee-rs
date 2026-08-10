use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable Melodee-owned Release identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReleaseId(Uuid);

impl ReleaseId {
    /// Allocates a new time-ordered Melodee-owned Release identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ReleaseId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ReleaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable Melodee-owned Track identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(Uuid);

impl TrackId {
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

/// Non-exclusive classification inferred for a Release.
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
