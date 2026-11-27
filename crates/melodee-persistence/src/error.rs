//! Typed errors at the persistence boundary (root causes preserved via `source`).

use melodee_domain::{IngestionState, InvalidStateTransition, PublicationInvariantError};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum WorkStoreError {
    #[error("inbound discovery scan lease has expired or was replaced")]
    DiscoveryScanLeaseExpired,

    #[error("inbound discovery scan scope remains busy: {0}")]
    DiscoveryScanBusy(String),

    #[error("inbound discovery scan has retryable candidate failures: {0}")]
    DiscoveryScanIncomplete(String),

    #[error(
        "settling capture staging budget exhausted; raise its configured row or byte limit and retry"
    )]
    SettlingCaptureBudgetExhausted,

    #[error("custody hash spool quota exhausted; raise its configured limits and retry")]
    CustodyHashBudgetExhausted,

    #[error("retained source observation quota exhausted; raise its configured limits and retry")]
    RetainedSourceBudgetExhausted,

    #[error("retained source changed and requires a retention-safe fresh intake")]
    RetainedFreshIntakeRequired,

    #[error("review identification merge failed")]
    ReviewMerge(#[source] Box<crate::CatalogStoreError>),

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

    #[error("work item {0} is cataloged and cannot be purged")]
    Cataloged(Uuid),

    #[error("invalid work-item request: {0}")]
    InvalidRequest(&'static str),

    #[error("work item {0} has exhausted automatic retries")]
    RetriesExhausted(Uuid),

    #[error("persisted state is not a known ingestion state: {0}")]
    CorruptState(String),

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("manifest serialization error")]
    ManifestSerialization(#[from] serde_json::Error),

    #[error("manifest evidence storage failed")]
    ManifestEvidence(#[from] crate::manifest_evidence::ManifestEvidenceError),

    #[error("a different manifest is already authoritative for work item {0}")]
    ManifestConflict(Uuid),

    #[error("work item {0} has no persisted manifest")]
    ManifestNotFound(Uuid),

    #[error("invalid canonical publication contract")]
    InvalidPublication(#[from] PublicationInvariantError),

    #[error("work item {0} has no durable publication plan")]
    PublicationNotFound(Uuid),

    #[error("durable publication intent conflicts for work item {0}")]
    PublicationConflict(Uuid),

    #[error("publication journal key {1} conflicts for work item {0}")]
    PublicationJournalConflict(Uuid, String),
}
