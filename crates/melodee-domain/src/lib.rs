#![forbid(unsafe_code)]

//! Framework-independent Melodee domain vocabulary and invariants.
//!
//! This crate must not depend on Axum, SQLx, Tokio, filesystem implementations, or protocol DTOs.

mod artist;
mod credit;
mod identity;
mod ids;
mod ingestion;
mod manifest;
mod overlay;
mod release;
mod values;

pub use artist::{
    Artist, ArtistAlias, ArtistAliasType, ArtistRelationship, ArtistRelationshipKind, ArtistStatus,
    ArtistType, ExternalIdentity, ExternalIdentityOwner, GroupMembership, MembershipRole,
    NewArtist,
};
pub use credit::{
    ArtistTrackAssociation, Contribution, ContributionRole, ContributionTarget, DisplayArtist,
    ParticipationKind, PresentationRole, ReleaseArtist, TrackArtist, validate_release_artist_order,
    validate_track_artist_order,
};
pub use identity::{
    ArtistMatchDecision, ArtistMatchEvidence, ArtistMatchOutcome, ArtistMerge,
    ArtistNegativeAssertion, ArtistSplit, AudioIdentityEvidence, AudioIdentityEvidenceKind,
    AudioIdentityMatchDecision, AudioIdentityMatchOutcome, DecisionAuthority, EvidenceKind,
    EvidencePolarity, NewArtistMatchDecision, NewAudioIdentityMatchDecision, ObservedArtist,
    ParsedArtistCandidate, SplitAssignment,
};
pub use ids::{
    ArtistAliasId, ArtistId, ArtistMatchDecisionId, ArtistMatchEvidenceId, ArtistMergeId,
    ArtistNegativeAssertionId, ArtistPublicId, ArtistRelationshipId, ArtistSplitId,
    AudioIdentityDecisionId, AudioIdentityEvidenceId, AudioIdentityId, AudioIdentityMergeId,
    AudioIdentitySplitId, CatalogImportId, CatalogSemanticOverlayId, ContributionId,
    ExternalIdentityId, GroupMembershipId, MediaAssetId, ObservedArtistId, ReleaseArtistId,
    ReleaseId, ReleasePublicId, TrackArtistId, TrackId, TrackPublicId,
};
pub use ingestion::{
    Disposition, FailureClass, IngestionState, InvalidStateTransition, ReleaseWorkItem,
    ReleaseWorkItemEvent, RetryPolicy, TransitionKind, WorkErrorCode, WorkEventType,
};
pub use manifest::{
    AudioTechnicalObservation, CANONICAL_LAYOUT_VERSION, CANONICAL_MP3_PROFILE_VERSION,
    CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind, Finding, FindingSeverity,
    FingerprintObservation, INSPECTION_MANIFEST_VERSION, ManifestInspectionContext,
    ManifestPublication, ManifestReleaseObservation, NORMALIZATION_MANIFEST_VERSION,
    NormalizationExecutionProvenance, NormalizationObservations, NormalizedArtwork,
    NormalizedOutput, ObservedArtwork, ObservedFile, ObservedFileKind, ObservedMetadata,
    ObservedTrack, PUBLICATION_MANIFEST_VERSION, ParsedSidecar, PublicationAuthorization,
    PublicationInvariantError, PublicationJournalStep, PublicationPlan, ReleaseManifest,
    SourceDisposition, StructuralValidation, ValidationOutcome,
};
pub use overlay::{
    CatalogSemanticOverlay, DisplayArtistOwner, NewCatalogSemanticOverlay, OverlayArtistResolution,
    OverlayAudioIdentityLink, OverlayDisplayArtist, OverlayReferences,
};
pub use release::{
    AudioIdentity, AudioIdentityArtist, AudioIdentityMerge, AudioIdentitySplit,
    AudioIdentityStatus, MediaAsset, MediaAssetRole, NewTrack, Release, ReleaseTrait, Track,
    TrackMediaAsset, validate_release_track_ordinals,
};
pub use values::{Confidence, DateRange, DomainInvariantError};
