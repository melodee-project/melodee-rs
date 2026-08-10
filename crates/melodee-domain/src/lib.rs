#![forbid(unsafe_code)]

//! Framework-independent Melodee domain vocabulary and invariants.
//!
//! This crate must not depend on Axum, SQLx, Tokio, filesystem implementations, or protocol DTOs.

mod artist;
mod ingestion;
mod manifest;
mod release;

pub use artist::{ArtistId, ArtistType, ContributionRole};
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
pub use release::{ReleaseId, ReleaseTrait, TrackId};
