//! PostgreSQL persistence for Melodee (ADR-0105, ADR-0110).
//!
//! Durable Release work items: schema migrations, transactional claiming
//! (`FOR UPDATE SKIP LOCKED`), leases, recovery, retries, failure evidence,
//! and append-only transition history. Domain rules live in `melodee-domain`;
//! this crate maps them to SQL and never invents state transitions itself.

#![forbid(unsafe_code)]

mod catalog;
mod catalog_identity;
mod catalog_mutations;
mod config;
mod error;
mod migrate;
mod policy;
mod work_items;
mod worker;

pub use catalog::{
    CatalogCorrectionDimension, CatalogCounts, CatalogImportRequest, CatalogImportResult,
    CatalogStore, CatalogStoreError, LegacyCatalogEntityType, LegacyCatalogIdMapping,
    MAX_CATALOG_CORRECTION_IDENTITY_ROWS, MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES,
    MAX_CATALOG_CORRECTION_RELEASES, MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS,
    ProviderNegativeAssertion, StoredArtistCandidate, StoredArtistTrackAssociation,
    StoredCatalogImport,
};
pub use config::{
    ENV_CLAIM_BATCH_SIZE, ENV_DATABASE_URL, ENV_LEASE_SECONDS, ENV_MAX_ATTEMPTS,
    ENV_RETRY_BASE_SECONDS, ENV_RETRY_MAX_SECONDS, WorkConfigError, WorkEngineConfig,
};
pub use error::WorkStoreError;
pub use migrate::{MIGRATOR, migrate};
pub use policy::{
    CatalogTrackFact, MAX_POLICY_TEST_SAMPLES, PolicyStore, PolicyStoreError, StoredPolicyDecision,
};
pub use work_items::{
    ActiveLease, PublicationJournalEntry, SettlingSnapshot, SettlingSnapshotFile, StateCount,
    StoredManifest, StoredPublication, WorkItemStore,
};
pub use worker::WorkerId;
