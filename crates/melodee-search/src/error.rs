//! Error types for the Melodee search application crate.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("index open error: {0}")]
    IndexOpen(String),
    #[error("invalid search root: {0}")]
    InvalidRoot(String),
    #[error("search root is not owned/validated: {0}")]
    UnownedRoot(String),
    #[error("database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("search persistence error: {0}")]
    Persistence(#[from] melodee_persistence::SearchStoreError),
    #[error("mql error: {0}")]
    Mql(#[from] melodee_mql::MqlError),
    #[error("invalid search query: {0}")]
    InvalidQuery(String),
    #[error("no active generation for root {0}")]
    NoActiveGeneration(String),
    #[error("generation {0} not found")]
    GenerationNotFound(String),
    #[error("rebuild already running: {0}")]
    RebuildRunning(String),
    #[error("stale write rejected (lease lost) for generation {0}")]
    StaleWrite(String),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("retryable index failure: {0}")]
    Retryable(String),
}
