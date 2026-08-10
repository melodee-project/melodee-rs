//! Error types for durable discovery and settling.

use thiserror::Error;

use melodee_persistence::WorkStoreError;

use crate::config::IngestionSourceParseError;
use crate::snapshot::SnapshotCaptureError;
use crate::source::SourceDiscoveryError;
use melodee_media::InspectionError;

use crate::{PublicationError, StructuralValidationError};

#[derive(Debug, Error)]
pub enum IngestionError {
    #[error("ingestion configuration is invalid: {0}")]
    Config(#[from] IngestionSourceParseError),

    #[error("source discovery failed: {0}")]
    Source(#[from] SourceDiscoveryError),

    #[error("snapshot capture failed: {0}")]
    Snapshot(#[from] SnapshotCaptureError),

    #[error("persistence failed: {0}")]
    Persistence(#[from] WorkStoreError),

    #[error("media inspection failed: {0}")]
    Inspection(#[from] InspectionError),

    #[error("inspection task failed: {0}")]
    InspectionTask(String),

    #[error("structural validation failed: {0}")]
    Validation(#[from] StructuralValidationError),

    #[error("canonical publication failed: {0}")]
    Publication(#[from] PublicationError),
}

pub type Result<T> = std::result::Result<T, IngestionError>;
