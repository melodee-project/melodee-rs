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
    AudioTechnicalObservation, CURRENT_MANIFEST_VERSION, Finding, FindingSeverity,
    ManifestInspectionContext, ManifestReleaseObservation, ObservedArtwork, ObservedFile,
    ObservedFileKind, ObservedMetadata, ObservedTrack, ReleaseManifest,
};
pub use release::{ReleaseId, ReleaseTrait, TrackId};
