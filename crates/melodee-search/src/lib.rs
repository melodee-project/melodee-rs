//! Melodee search application crate (ADR-0121, ADR-0122).
//!
//! Owns the Tantivy projection lifecycle: schema, generational writer/reader,
//! MQL translation, outbox consumption, rebuild, corruption recovery,
//! autocomplete/admin-lookup, and the CLI/server worker. It never owns catalog
//! truth or media bytes (SPEC-INV-006).

#![forbid(unsafe_code)]

pub mod config;
pub mod documents;
pub mod error;
pub mod generation;
pub mod rebuild;
pub mod schema;
mod secure_fs;
pub mod service;
pub mod store;
pub mod translator;
pub mod worker;

pub use config::{
    ENV_SEARCH_BATCH_SIZE, ENV_SEARCH_DEFAULT_LIMIT, ENV_SEARCH_ENABLED, ENV_SEARCH_LEASE_SECONDS,
    ENV_SEARCH_MAX_LIMIT, ENV_SEARCH_MAX_OUTBOX_ATTEMPTS, ENV_SEARCH_POLL_INTERVAL_MS,
    ENV_SEARCH_PROCESSED_OUTBOX_RETENTION_DAYS, ENV_SEARCH_ROOT, ENV_SEARCH_SNAPSHOT_PAGE_SIZE,
    SearchConfig, is_inside_root, reject_root_overlap, validate_search_root,
};
pub use documents::{ArtistDoc, ReleaseDoc, TrackDoc};
pub use error::SearchError;
pub use generation::{GenerationManifest, SearchRoot};
pub use rebuild::{generation_manifest, request_rebuild, run_rebuild_once};
pub use schema::{
    RAW_TOKENIZER, SCHEMA_VERSION, SearchSchema, TEXT_TOKENIZER, build_schema, register_tokenizers,
};
pub use service::{
    SearchDiagnostic, autocomplete_active, diagnostic, query_active, query_active_kind,
    query_plan_active, resolve_artist_ids_active,
};
pub use store::{GenerationStats, SearchHit, SearchStore};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use worker::{SearchApplyTestBarrier, SearchSchedulerTestProbe};
pub use worker::{SearchPassStats, SearchSchedulerHandle, apply_outbox_once, start_search_loop};
