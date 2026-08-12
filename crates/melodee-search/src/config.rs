//! Search configuration and search-root path safety (ADR-0121).
//!
//! The search root is the single directory under which all Tantivy generations
//! live. It is validated at startup: absolute, canonical, free of symlink
//! components, and not overlapping another configured root. No `rm -rf` is ever
//! used; only ownership-validated, path-checked removal under this root is
//! permitted.

use std::path::{Path, PathBuf};

use melodee_mql::Bounds;
use serde::{Deserialize, Serialize};

use crate::error::SearchError;

#[cfg(feature = "test-support")]
use crate::worker::{SearchApplyTestBarrier, SearchSchedulerTestProbe};

pub const ENV_SEARCH_ENABLED: &str = "MELODEE__SEARCH__ENABLED";
pub const ENV_SEARCH_ROOT: &str = "MELODEE__SEARCH__ROOT";
pub const ENV_SEARCH_LEASE_SECONDS: &str = "MELODEE__SEARCH__LEASE_SECONDS";
pub const ENV_SEARCH_BATCH_SIZE: &str = "MELODEE__SEARCH__BATCH_SIZE";
pub const ENV_SEARCH_MAX_OUTBOX_ATTEMPTS: &str = "MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS";
pub const ENV_SEARCH_POLL_INTERVAL_MS: &str = "MELODEE__SEARCH__POLL_INTERVAL_MS";
pub const ENV_SEARCH_SNAPSHOT_PAGE_SIZE: &str = "MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE";
pub const ENV_SEARCH_DEFAULT_LIMIT: &str = "MELODEE__SEARCH__DEFAULT_LIMIT";
pub const ENV_SEARCH_MAX_LIMIT: &str = "MELODEE__SEARCH__MAX_LIMIT";

const MAX_SEARCH_LEASE_SECONDS: u64 = 3_600;
const MAX_SEARCH_POLL_INTERVAL_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Root directory owning all generations. Created if missing.
    pub search_root: PathBuf,
    /// How long a process may own a generation/write lease, in seconds.
    pub lease_seconds: u64,
    /// Rows claimed per outbox poll.
    pub claim_batch_size: usize,
    /// Failed attempts before a retryable row becomes a durable permanent
    /// failure instead of blocking the projection forever.
    pub max_outbox_attempts: u32,
    /// Idle delay between outbox polls, in milliseconds.
    pub poll_interval_ms: u64,
    /// Rows streamed from PostgreSQL at once during a rebuild.
    pub snapshot_page_size: usize,
    /// Default query result limit.
    pub default_limit: usize,
    /// Maximum query result limit.
    pub max_limit: usize,
    /// Whether the search worker is enabled at server startup.
    pub enabled: bool,
    #[cfg(feature = "test-support")]
    #[serde(skip)]
    #[doc(hidden)]
    pub apply_test_barrier: Option<SearchApplyTestBarrier>,
    #[cfg(feature = "test-support")]
    #[serde(skip)]
    #[doc(hidden)]
    pub scheduler_test_probe: Option<SearchSchedulerTestProbe>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            search_root: PathBuf::from("/var/lib/melodee/search"),
            lease_seconds: 60,
            claim_batch_size: 256,
            max_outbox_attempts: 5,
            poll_interval_ms: 1_000,
            snapshot_page_size: 1_000,
            default_limit: 200,
            max_limit: 1000,
            enabled: false,
            #[cfg(feature = "test-support")]
            apply_test_barrier: None,
            #[cfg(feature = "test-support")]
            scheduler_test_probe: None,
        }
    }
}

impl SearchConfig {
    pub fn from_env() -> Result<Self, String> {
        let defaults = Self::default();
        let enabled = parse_bool(ENV_SEARCH_ENABLED, defaults.enabled)?;
        let search_root = std::env::var_os(ENV_SEARCH_ROOT)
            .map(PathBuf::from)
            .unwrap_or(defaults.search_root);
        let config = Self {
            search_root,
            enabled,
            lease_seconds: parse_u64(ENV_SEARCH_LEASE_SECONDS, defaults.lease_seconds)?,
            claim_batch_size: parse_usize(ENV_SEARCH_BATCH_SIZE, defaults.claim_batch_size)?,
            max_outbox_attempts: parse_u32(
                ENV_SEARCH_MAX_OUTBOX_ATTEMPTS,
                defaults.max_outbox_attempts,
            )?,
            poll_interval_ms: parse_u64(ENV_SEARCH_POLL_INTERVAL_MS, defaults.poll_interval_ms)?,
            snapshot_page_size: parse_usize(
                ENV_SEARCH_SNAPSHOT_PAGE_SIZE,
                defaults.snapshot_page_size,
            )?,
            default_limit: parse_usize(ENV_SEARCH_DEFAULT_LIMIT, defaults.default_limit)?,
            max_limit: parse_usize(ENV_SEARCH_MAX_LIMIT, defaults.max_limit)?,
            #[cfg(feature = "test-support")]
            apply_test_barrier: None,
            #[cfg(feature = "test-support")]
            scheduler_test_probe: None,
        };
        config.validate()?;
        if config.enabled && std::env::var_os(ENV_SEARCH_ROOT).is_none() {
            return Err(format!(
                "{ENV_SEARCH_ROOT} is required when {ENV_SEARCH_ENABLED}=true"
            ));
        }
        Ok(config)
    }

    /// The MQL resource bounds used when parsing/planning queries.
    pub fn mql_bounds(&self) -> Bounds {
        Bounds::default()
    }

    /// Validate positive/finite bounds and the search root, failing fast on
    /// invalid configuration (rule F: validate positive bounds and path safety).
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_SEARCH_LEASE_SECONDS).contains(&self.lease_seconds) {
            return Err(format!(
                "lease_seconds must be between 1 and {MAX_SEARCH_LEASE_SECONDS}"
            ));
        }
        if self.claim_batch_size == 0 {
            return Err("claim_batch_size must be > 0".to_string());
        }
        if self.claim_batch_size > melodee_persistence::MAX_SEARCH_OUTBOX_BATCH {
            return Err(format!(
                "claim_batch_size must be <= {}",
                melodee_persistence::MAX_SEARCH_OUTBOX_BATCH
            ));
        }
        if !(1..=melodee_persistence::MAX_SEARCH_OUTBOX_ATTEMPTS)
            .contains(&self.max_outbox_attempts)
        {
            return Err(format!(
                "max_outbox_attempts must be between 1 and {}",
                melodee_persistence::MAX_SEARCH_OUTBOX_ATTEMPTS
            ));
        }
        if !(1..=melodee_persistence::MAX_SEARCH_SNAPSHOT_PAGE).contains(&self.snapshot_page_size) {
            return Err(format!(
                "snapshot_page_size must be between 1 and {}",
                melodee_persistence::MAX_SEARCH_SNAPSHOT_PAGE
            ));
        }
        if !(1..=MAX_SEARCH_POLL_INTERVAL_MS).contains(&self.poll_interval_ms) {
            return Err(format!(
                "poll_interval_ms must be between 1 and {MAX_SEARCH_POLL_INTERVAL_MS}"
            ));
        }
        if self.default_limit == 0 || self.default_limit > self.max_limit {
            return Err("default_limit must be in (0, max_limit]".to_string());
        }
        if self.max_limit == 0 || self.max_limit > crate::store::MAX_SEARCH_RESULTS {
            return Err(format!(
                "max_limit must be between 1 and {}",
                crate::store::MAX_SEARCH_RESULTS
            ));
        }
        Ok(())
    }

    /// Validate and canonicalize the search root. Rejects relative, non-absolute,
    /// symlink-containing, or otherwise unsafe paths.
    pub fn resolve_root(&self) -> Result<PathBuf, SearchError> {
        validate_search_root(&self.search_root)
    }

    /// Resolve the search root and apply the shared source/storage overlap
    /// policy used by server and administrative entrypoints.
    pub fn resolve_root_against(&self, other_roots: &[PathBuf]) -> Result<PathBuf, SearchError> {
        let root = self.resolve_root()?;
        reject_root_overlap(&root, other_roots)?;
        Ok(root)
    }
}

/// Validate a candidate search root path.
///
/// Rules (ADR-0121 path safety):
/// - must be absolute;
/// - must not contain symlink components (each ancestor resolved without
///   following symlinks);
/// - the canonicalized directory must be created if missing with restrictive
///   permissions.
pub fn validate_search_root(path: &Path) -> Result<PathBuf, SearchError> {
    if !path.is_absolute() {
        return Err(SearchError::InvalidRoot(format!(
            "search root must be absolute: {}",
            path.display()
        )));
    }
    if path.parent().is_none() {
        return Err(SearchError::InvalidRoot(
            "filesystem root cannot be used as the search root".to_string(),
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(SearchError::InvalidRoot(format!(
            "search root must not contain parent traversal: {}",
            path.display()
        )));
    }
    // Reject symlink components anywhere in the path prefix.
    let mut accum = PathBuf::new();
    for component in path.components() {
        accum.push(component.as_os_str());
        let meta = std::fs::symlink_metadata(&accum);
        match meta {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(SearchError::InvalidRoot(format!(
                    "search root contains a symlink component: {}",
                    accum.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Not yet created; that's fine, we create it below.
            }
            Err(e) => {
                return Err(SearchError::Io {
                    path: accum.clone(),
                    source: e,
                });
            }
        }
    }
    // Create if missing.
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|source| SearchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(
                |source| SearchError::Io {
                    path: path.to_path_buf(),
                    source,
                },
            )?;
        }
    }
    let canonical = path.canonicalize().map_err(|source| SearchError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if canonical.parent().is_none() {
        return Err(SearchError::InvalidRoot(
            "filesystem root cannot be used as the search root".to_string(),
        ));
    }
    Ok(canonical)
}

/// Reject roots that overlap a canonical source, storage, or other application
/// root. Equality and either parent direction are unsafe.
pub fn reject_root_overlap(search_root: &Path, other_roots: &[PathBuf]) -> Result<(), SearchError> {
    let search = search_root
        .canonicalize()
        .map_err(|source| SearchError::Io {
            path: search_root.to_path_buf(),
            source,
        })?;
    for other in other_roots {
        let canonical = other.canonicalize().map_err(|source| SearchError::Io {
            path: other.clone(),
            source,
        })?;
        if search.starts_with(&canonical) || canonical.starts_with(&search) {
            return Err(SearchError::InvalidRoot(format!(
                "search root {} overlaps configured root {}",
                search.display(),
                canonical.display()
            )));
        }
    }
    Ok(())
}

/// True if `child` is safely inside `root` (no `..` escape), used to reject
/// unowned deletion/retirement targets.
pub fn is_inside_root(root: &Path, child: &Path) -> bool {
    match (root.canonicalize(), child.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(r),
        _ => false,
    }
}

fn parse_bool(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => match value.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(format!("{name} must be true, false, 1, or 0")),
        },
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(format!("cannot read {name}: {error}")),
    }
}

fn parse_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(format!("cannot read {name}: {error}")),
    }
}

fn parse_u32(name: &str, default: u32) -> Result<u32, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(format!("cannot read {name}: {error}")),
    }
}

fn parse_usize(name: &str, default: usize) -> Result<usize, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be a non-negative integer")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(format!("cannot read {name}: {error}")),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn configuration_rejects_zero_and_excessive_bounds() {
        let mut config = SearchConfig::default();
        config.lease_seconds = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.claim_batch_size = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.lease_seconds = MAX_SEARCH_LEASE_SECONDS + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.claim_batch_size = melodee_persistence::MAX_SEARCH_OUTBOX_BATCH + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.snapshot_page_size = melodee_persistence::MAX_SEARCH_SNAPSHOT_PAGE + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.max_outbox_attempts = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.max_outbox_attempts = melodee_persistence::MAX_SEARCH_OUTBOX_ATTEMPTS + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.poll_interval_ms = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.poll_interval_ms = MAX_SEARCH_POLL_INTERVAL_MS + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.default_limit = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.default_limit = config.max_limit + 1;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.max_limit = 0;
        assert!(config.validate().is_err());

        config = SearchConfig::default();
        config.max_limit = crate::store::MAX_SEARCH_RESULTS + 1;
        assert!(config.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn root_validation_rejects_a_symlink_component() {
        let directory = tempfile::tempdir().expect("temporary root");
        let real = directory.path().join("real");
        std::fs::create_dir(&real).expect("real directory");
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink fixture");
        assert!(validate_search_root(&link).is_err());
        assert!(validate_search_root(&link.join("search")).is_err());
    }
}
