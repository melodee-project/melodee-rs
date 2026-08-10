//! Typed errors at the persistence boundary (root causes preserved via `source`).

use melodee_domain::{IngestionState, InvalidStateTransition};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum WorkStoreError {
    #[error("work item not found: {0}")]
    NotFound(Uuid),

    #[error("illegal state transition")]
    IllegalTransition(#[from] InvalidStateTransition),

    #[error("work item {0} is leased by another worker")]
    NotLeaseOwner(Uuid),

    #[error("work item {0} lease has expired")]
    LeaseExpired(Uuid),

    #[error("work item {0} has an active lease held by another worker")]
    LeaseConflict(Uuid),

    #[error("work item {0} is in state {1:?}, which does not allow this operation")]
    InvalidState(Uuid, IngestionState),

    #[error("work item {0} has exhausted automatic retries")]
    RetriesExhausted(Uuid),

    #[error("persisted state is not a known ingestion state: {0}")]
    CorruptState(String),

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("manifest serialization error")]
    ManifestSerialization(#[from] serde_json::Error),

    #[error("a different manifest is already authoritative for work item {0}")]
    ManifestConflict(Uuid),

    #[error("work item {0} has no persisted manifest")]
    ManifestNotFound(Uuid),
}
