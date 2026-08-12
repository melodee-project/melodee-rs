//! Error types for the Melodee search application crate.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("search filesystem operation failed")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("search index operation failed")]
    Tantivy(#[from] tantivy::TantivyError),
    #[error("search index could not be opened")]
    IndexOpen(String),
    #[error("invalid search root configuration")]
    InvalidRoot(String),
    #[error("search root ownership validation failed")]
    UnownedRoot(String),
    #[error("search database operation failed")]
    Sql(#[from] sqlx::Error),
    #[error("search persistence operation failed")]
    Persistence(#[from] melodee_persistence::SearchStoreError),
    #[error("mql error: {0}")]
    Mql(#[from] melodee_mql::MqlError),
    #[error("invalid search query: {0}")]
    InvalidQuery(String),
    #[error("no active search generation")]
    NoActiveGeneration(String),
    #[error("generation {0} not found")]
    GenerationNotFound(String),
    #[error("rebuild already running: {0}")]
    RebuildRunning(String),
    #[error("stale write rejected (lease lost) for generation {0}")]
    StaleWrite(String),
    #[error("search metadata serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("retryable search operation failed")]
    Retryable(String),
}

impl SearchError {
    /// Stable operator-facing summary that does not expose configured paths or
    /// storage-engine internals. Callers should log the original error with
    /// structured tracing before presenting this summary.
    #[must_use]
    pub fn operator_message(&self) -> &'static str {
        match self {
            Self::InvalidQuery(_) | Self::Mql(_) => "the search query is invalid",
            Self::NoActiveGeneration(_) => "no active search generation is available",
            Self::GenerationNotFound(_) => "the requested search generation was not found",
            Self::RebuildRunning(_) => "a search rebuild is already running",
            Self::StaleWrite(_) => "the search worker lost its lease; retry is safe",
            Self::InvalidRoot(_) | Self::UnownedRoot(_) => {
                "search root ownership or configuration is invalid"
            }
            Self::Io { .. }
            | Self::Tantivy(_)
            | Self::IndexOpen(_)
            | Self::Sql(_)
            | Self::Persistence(_)
            | Self::Json(_)
            | Self::Retryable(_) => {
                "search operation failed; inspect structured server logs for details"
            }
        }
    }

    /// Stable machine-readable classification for operator diagnostics.
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::InvalidQuery(_) | Self::Mql(_) => "search.invalid_query",
            Self::NoActiveGeneration(_) => "search.no_active_generation",
            Self::GenerationNotFound(_) => "search.generation_not_found",
            Self::RebuildRunning(_) => "search.rebuild_running",
            Self::StaleWrite(_) => "search.stale_write",
            Self::InvalidRoot(_) | Self::UnownedRoot(_) => "search.invalid_root",
            Self::Io { .. } => "search.filesystem",
            Self::Tantivy(_) | Self::IndexOpen(_) => "search.index_unavailable",
            Self::Sql(_) | Self::Persistence(_) => "search.persistence",
            Self::Json(_) => "search.invalid_metadata",
            Self::Retryable(_) => "search.retryable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SearchError;

    #[test]
    fn operator_message_redacts_internal_paths() {
        let sentinel = "/sensitive/internal/search/root/generation";
        let error = SearchError::Io {
            path: sentinel.into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        let message = error.operator_message();
        assert!(!message.contains(sentinel));
        assert!(!message.contains("generation"));
        assert!(!error.to_string().contains(sentinel));
        assert!(format!("{error:?}").contains(sentinel));
        assert_eq!(error.reason_code(), "search.filesystem");
    }
}
