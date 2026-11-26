//! Application services for bounded manifest-v3 catalog import.
//!
//! This crate coordinates PostgreSQL catalog transactions. It deliberately
//! has no filesystem, media-decoder, provider, or network boundary.

#![forbid(unsafe_code)]

mod authority;
mod cancellation;
mod config;
mod importer;
mod resolver;
mod scheduler;

pub use authority::{
    ArtistAuthority, AuthorityArtistCandidate, AuthorityArtistObservation, AuthorityCandidateKey,
    AuthorityConsultation, AuthorityDecisionContext, ProvisionalReason,
    consult_authority_for_catalog,
};
pub use config::{
    CatalogConfig, CatalogConfigError, ENV_CATALOG_BATCH_SIZE, ENV_CATALOG_CONCURRENCY,
    ENV_CATALOG_ENABLED, ENV_CATALOG_POLL_INTERVAL_SECONDS,
};
pub use importer::{CatalogImportError, import_once, reconcile_affected_identities_once};
pub use resolver::{
    ArtistResolutionError, ArtistResolutionEvaluation, ArtistResolutionRecommendation,
    CandidateAssessment, DEFAULT_MAX_ARTIST_CANDIDATES, DEFAULT_MAX_EVIDENCE_PER_CANDIDATE,
    LocalArtistCandidate, LocalArtistResolver, LocalProvisionalReason, LocalResolutionConstraints,
    ResolutionEvidence, ResolutionEvidenceSource,
};
pub use scheduler::{CatalogSchedulerHandle, start_catalog_loop};

/// Resolver version persisted with the initial local-only catalog import.
pub const LOCAL_RESOLVER_VERSION: &str = "melodee.catalog.local-v1";

/// Outcome counters for one bounded catalog-import pass.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct CatalogImportStats {
    pub claimed: usize,
    pub completed: usize,
    pub already_completed: usize,
    pub released: usize,
    pub failed: usize,
    pub errors: usize,
    pub duration_ms: u128,
}
