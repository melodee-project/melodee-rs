//! PostgreSQL persistence for Melodee (ADR-0105, ADR-0110).
//!
//! Phase 1A scope: durable Release work items — schema migrations, transactional
//! claiming (`FOR UPDATE SKIP LOCKED`), leases, recovery, retries, failure
//! evidence, and append-only transition history. Domain rules live in
//! `melodee-domain`; this crate maps them to SQL and never invents state
//! transitions itself.

#![forbid(unsafe_code)]

mod config;
mod error;
mod migrate;
mod work_items;
mod worker;

pub use config::{
    ENV_CLAIM_BATCH_SIZE, ENV_DATABASE_URL, ENV_LEASE_SECONDS, ENV_MAX_ATTEMPTS,
    ENV_RETRY_BASE_SECONDS, ENV_RETRY_MAX_SECONDS, WorkEngineConfig,
};
pub use error::WorkStoreError;
pub use migrate::{MIGRATOR, migrate};
pub use work_items::{StateCount, WorkItemStore};
pub use worker::WorkerId;
