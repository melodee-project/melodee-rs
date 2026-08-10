use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! internal_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        // Identity allocation is intentionally explicit; `Default` would let aggregate
        // construction silently allocate a replacement identity.
        #[allow(clippy::new_without_default)]
        impl $name {
            /// Allocates a new time-ordered Melodee-owned identity.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Rehydrates an identity at a persistence or migration boundary.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }

            #[must_use]
            pub fn is_uuid_v7(self) -> bool {
                self.0.get_version_num() == 7
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

macro_rules! public_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Preserves a public UUID verbatim, including a legacy UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

internal_id!(ArtistId, "Stable Melodee-owned Artist identity.");
internal_id!(ReleaseId, "Stable Melodee-owned Release identity.");
internal_id!(TrackId, "Stable Melodee-owned Track occurrence identity.");
internal_id!(
    MediaAssetId,
    "Stable Melodee-owned encoded-byte asset identity."
);
internal_id!(
    AudioIdentityId,
    "Stable Melodee-owned underlying audible recording/performance identity."
);
internal_id!(ArtistAliasId, "Stable Artist alias row identity.");
internal_id!(
    ExternalIdentityId,
    "Stable external-identity evidence row identity."
);
internal_id!(ArtistRelationshipId, "Stable Artist relationship identity.");
internal_id!(GroupMembershipId, "Stable group-membership identity.");
internal_id!(
    ReleaseArtistId,
    "Stable Release billing association identity."
);
internal_id!(TrackArtistId, "Stable Track billing association identity.");
internal_id!(ContributionId, "Stable contribution identity.");
internal_id!(ObservedArtistId, "Stable raw Artist observation identity.");
internal_id!(
    ArtistMatchEvidenceId,
    "Stable Artist-match evidence identity."
);
internal_id!(
    ArtistMatchDecisionId,
    "Stable Artist-match decision identity."
);
internal_id!(
    ArtistNegativeAssertionId,
    "Stable negative assertion identity."
);
internal_id!(
    ArtistMergeId,
    "Stable append-only Artist merge-event identity."
);
internal_id!(
    ArtistSplitId,
    "Stable append-only Artist split-event identity."
);
internal_id!(
    AudioIdentityMergeId,
    "Stable AudioIdentity merge-event identity."
);
internal_id!(
    AudioIdentitySplitId,
    "Stable AudioIdentity split-event identity."
);
internal_id!(
    AudioIdentityEvidenceId,
    "Stable AudioIdentity-match evidence identity."
);
internal_id!(
    AudioIdentityDecisionId,
    "Stable AudioIdentity-match decision identity."
);
internal_id!(CatalogImportId, "Stable catalog import identity.");
internal_id!(
    CatalogSemanticOverlayId,
    "Stable semantic overlay revision identity."
);
internal_id!(
    ReleaseClassificationId,
    "Stable Release classification record identity."
);
internal_id!(TrackNoveltyId, "Stable Track novelty assessment identity.");
internal_id!(
    PolicyRuleSetId,
    "Stable versioned policy rule-set identity."
);
internal_id!(PolicyRuleId, "Stable policy rule identity.");
internal_id!(
    ReleasePolicyDecisionId,
    "Stable durable Release policy decision identity."
);
internal_id!(
    PolicyTestRunId,
    "Stable policy dry-run/test result identity."
);

public_id!(
    ArtistPublicId,
    "Immutable Artist compatibility/public API UUID."
);
public_id!(
    ReleasePublicId,
    "Immutable Release compatibility/public API UUID."
);
public_id!(
    TrackPublicId,
    "Immutable Track compatibility/public API UUID."
);

impl From<ArtistId> for ArtistPublicId {
    fn from(value: ArtistId) -> Self {
        Self::from_uuid(value.into_uuid())
    }
}

impl From<ReleaseId> for ReleasePublicId {
    fn from(value: ReleaseId) -> Self {
        Self::from_uuid(value.into_uuid())
    }
}

impl From<TrackId> for TrackPublicId {
    fn from(value: TrackId) -> Self {
        Self::from_uuid(value.into_uuid())
    }
}
