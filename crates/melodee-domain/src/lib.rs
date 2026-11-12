#![forbid(unsafe_code)]

//! Framework-independent Melodee domain vocabulary and invariants.
//!
//! This crate must not depend on Axum, SQLx, Tokio, filesystem implementations, or protocol DTOs.

mod artist;
mod classification;
mod credit;
mod identity;
mod ids;
mod ingestion;
mod manifest;
mod novelty;
mod overlay;
mod policy;
mod release;
mod values;

pub use artist::{
    Artist, ArtistAlias, ArtistAliasType, ArtistRelationship, ArtistRelationshipKind, ArtistStatus,
    ArtistType, ExternalIdentity, ExternalIdentityOwner, GroupMembership, MembershipRole,
    NewArtist,
};
pub use classification::{
    ClassificationEvidence, ClassificationEvidenceKind, EvidenceSource,
    MAX_RELEASE_CLASSIFICATION_EVIDENCE, MAX_RELEASE_CLASSIFICATION_TRAITS, ReleaseClassification,
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
    ParsedArtistCandidate, SplitAssignment, parse_explicit_artist_billing,
    parse_slash_artist_billing,
};
pub use ids::{
    ArtistAliasId, ArtistId, ArtistMatchDecisionId, ArtistMatchEvidenceId, ArtistMergeId,
    ArtistNegativeAssertionId, ArtistPublicId, ArtistRelationshipId, ArtistSplitId,
    AudioIdentityDecisionId, AudioIdentityEvidenceId, AudioIdentityId, AudioIdentityMergeId,
    AudioIdentitySplitId, CatalogImportId, CatalogSemanticOverlayId, ContributionId,
    ExternalIdentityId, GroupMembershipId, MediaAssetId, ObservedArtistId, PolicyRuleId,
    PolicyRuleSetId, PolicyTestRunId, ReleaseArtistId, ReleaseClassificationId, ReleaseId,
    ReleasePolicyDecisionId, ReleasePublicId, TrackArtistId, TrackId, TrackNoveltyId,
    TrackPublicId,
};
pub use ingestion::{
    Disposition, FailureClass, IngestionPhase, IngestionState, InvalidStateTransition,
    ReleaseWorkItem, ReleaseWorkItemEvent, RetryPolicy, TransitionKind, WorkErrorCode,
    WorkEventType,
};
pub use manifest::{
    AudioTechnicalObservation, CANONICAL_LAYOUT_VERSION, CANONICAL_MP3_PROFILE_VERSION,
    CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind, CueSegmentIdentity, Finding,
    FindingSeverity, FingerprintObservation, INSPECTION_MANIFEST_VERSION,
    ManifestInspectionContext, ManifestPublication, ManifestReleaseObservation,
    NORMALIZATION_MANIFEST_VERSION, NormalizationExecutionProvenance, NormalizationObservations,
    NormalizedArtwork, NormalizedOutput, ObservedArtwork, ObservedFile, ObservedFileKind,
    ObservedMetadata, ObservedTrack, PUBLICATION_MANIFEST_VERSION, ParsedSidecar,
    PublicationAuthorization, PublicationInvariantError, PublicationJournalStep, PublicationPlan,
    ReleaseManifest, SourceDisposition, StructuralValidation, ValidationOutcome,
    acoustic_fingerprint_digest,
};
pub use novelty::{NoveltyEvaluation, NoveltyEvidenceKind, TrackNovelty, TrackNoveltyCategory};
pub use overlay::{
    CatalogSemanticOverlay, DisplayArtistOwner, NewCatalogSemanticOverlay, OverlayArtistResolution,
    OverlayAudioIdentityLink, OverlayDisplayArtist, OverlayReferences,
};
pub use policy::{
    AdministratorOverride, DecisionCompletionState, MAX_POLICY_TEST_MATCHED_RULES,
    MAX_POLICY_TEST_RELEASES, MAX_POLICY_TEST_SAMPLES, PolicyActor, PolicyEvaluation, PolicyRule,
    PolicyRuleSet, PolicyRuleType, PolicyTestSummary, ReleasePolicyDecision,
};
pub use release::{
    AudioIdentity, AudioIdentityArtist, AudioIdentityMerge, AudioIdentitySplit,
    AudioIdentityStatus, MediaAsset, MediaAssetRole, NewTrack, Release, ReleaseTrait, Track,
    TrackMediaAsset, validate_release_track_ordinals,
};
pub use values::{Confidence, DateRange, DomainInvariantError};
