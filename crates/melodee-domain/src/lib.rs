#![forbid(unsafe_code)]

//! Framework-independent Melodee domain vocabulary and invariants.
//!
//! This crate must not depend on Axum, SQLx, Tokio, filesystem implementations, or protocol DTOs.

mod artist;
mod ingestion;
mod release;

pub use artist::{ArtistId, ArtistType, ContributionRole};
pub use ingestion::{Disposition, IngestionState, InvalidStateTransition};
pub use release::{ReleaseId, ReleaseTrait, TrackId};
