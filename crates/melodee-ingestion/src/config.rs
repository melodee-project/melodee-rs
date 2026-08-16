//! Ingestion configuration and source definitions.

use std::{
    env, fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use time::Duration;
use uuid::Uuid;

#[cfg(feature = "test-support")]
use crate::publication::PublicationCopyTestBarrier;

pub const ENV_INGEST_ENABLED: &str = "MELODEE__INGEST__ENABLED";
pub const ENV_RECONCILIATION_INTERVAL_SECONDS: &str =
    "MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS";
pub const ENV_SETTLING_QUIET_SECONDS: &str = "MELODEE__INGEST__SETTLING_QUIET_SECONDS";
pub const ENV_READY_MARKER_NAME: &str = "MELODEE__INGEST__READY_MARKER_NAME";
pub const ENV_READY_MARKER_SHORTCUT_SECONDS: &str =
    "MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS";
pub const ENV_SOURCES_JSON: &str = "MELODEE__INGEST__SOURCES_JSON";
pub const ENV_SOURCE_ROOT: &str = "MELODEE__INGEST__SOURCE_ROOT";
pub const ENV_SOURCE_LIBRARY_ID: &str = "MELODEE__INGEST__SOURCE_LIBRARY_ID";
pub const ENV_SOURCE_LIBRARY_NAME: &str = "MELODEE__INGEST__SOURCE_LIBRARY_NAME";
/// Durable Melodee-owned source storage. This is not normalization scratch:
/// it retains original source bytes for retry, review, and quarantine.
pub const ENV_STAGING_ROOT: &str = "MELODEE__INGEST__STAGING_ROOT";
pub const ENV_INSPECTION_CONCURRENCY: &str = "MELODEE__INSPECTION__CONCURRENCY";
pub const ENV_FFPROBE_PATH: &str = "MELODEE__INSPECTION__FFPROBE_PATH";
pub const ENV_FFPROBE_TIMEOUT_SECONDS: &str = "MELODEE__INSPECTION__FFPROBE_TIMEOUT_SECONDS";
pub const ENV_NORMALIZATION_CONCURRENCY: &str = "MELODEE__NORMALIZATION__CONCURRENCY";
pub const ENV_FFMPEG_PATH: &str = "MELODEE__NORMALIZATION__FFMPEG_PATH";
pub const ENV_FFMPEG_TIMEOUT_SECONDS: &str = "MELODEE__NORMALIZATION__FFMPEG_TIMEOUT_SECONDS";
pub const ENV_FPCALC_PATH: &str = "MELODEE__NORMALIZATION__FPCALC_PATH";
pub const ENV_FPCALC_TIMEOUT_SECONDS: &str = "MELODEE__NORMALIZATION__FPCALC_TIMEOUT_SECONDS";
pub const ENV_WORK_AREA_BASE: &str = "MELODEE__NORMALIZATION__WORK_AREA";
pub const ENV_VALIDATION_CONCURRENCY: &str = "MELODEE__VALIDATION__CONCURRENCY";
pub const ENV_PUBLICATION_CONCURRENCY: &str = "MELODEE__PUBLICATION__CONCURRENCY";
pub const ENV_STORAGE_ROOT: &str = "MELODEE__STORAGE__ROOT";

pub const DEFAULT_READY_MARKER: &str = ".melodee-ready";

const DEFAULT_RECONCILIATION_INTERVAL_SECONDS: i64 = 30;
const DEFAULT_SETTLING_QUIET_SECONDS: i64 = 10;
const DEFAULT_READY_MARKER_SHORTCUT_SECONDS: i64 = 0;
const DEFAULT_INSPECTION_CONCURRENCY: u32 = 2;
const DEFAULT_FFPROBE_TIMEOUT_SECONDS: i64 = 30;
const DEFAULT_NORMALIZATION_CONCURRENCY: u32 = 2;
const DEFAULT_FFMPEG_TIMEOUT_SECONDS: i64 = 300;
const DEFAULT_FPCALC_TIMEOUT_SECONDS: i64 = 60;
const DEFAULT_WORK_AREA_BASE: &str = "/tmp/melodee-work";
const DEFAULT_VALIDATION_CONCURRENCY: u32 = 2;
const DEFAULT_PUBLICATION_CONCURRENCY: u32 = 2;

#[derive(Debug, Clone)]
pub struct IngestionConfig {
    pub enabled: bool,
    pub sources: Vec<InboundSource>,
    pub reconciliation_interval: Duration,
    pub settling_quiet_period: Duration,
    pub ready_marker_name: String,
    pub ready_marker_shortcut: Duration,
    pub inspection_concurrency: u32,
    pub ffprobe_path: PathBuf,
    pub ffprobe_timeout: Duration,
    pub normalization_concurrency: u32,
    pub ffmpeg_path: PathBuf,
    pub ffmpeg_timeout: Duration,
    pub fpcalc_path: PathBuf,
    pub fpcalc_timeout: Duration,
    /// Required durable source storage whenever inbound ingestion is enabled.
    pub staging_root: Option<PathBuf>,
    pub work_area_base: PathBuf,
    pub validation_concurrency: u32,
    pub publication_concurrency: u32,
    /// Canonical publication is deliberately opt-in. An inbound source must
    /// never imply a destination with destructive operational consequences.
    pub storage_root: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub publication_copy_test_barrier: Option<PublicationCopyTestBarrier>,
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub acquisition_test_hook: Option<crate::acquisition::AcquisitionTestHook>,
}

impl IngestionConfig {
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled && !self.sources.is_empty()
    }

    /// Whether the canonical publication workers have an explicit target.
    #[must_use]
    pub const fn publication_enabled(&self) -> bool {
        self.enabled() && self.storage_root.is_some()
    }

    #[must_use]
    pub fn with_sources(sources: Vec<InboundSource>) -> Self {
        let staging_root = sources
            .first()
            .and_then(|source| source.root.parent())
            .map(|parent| parent.join(".melodee-staging-test"));
        Self {
            enabled: true,
            sources,
            reconciliation_interval: Duration::seconds(DEFAULT_RECONCILIATION_INTERVAL_SECONDS),
            settling_quiet_period: Duration::seconds(DEFAULT_SETTLING_QUIET_SECONDS),
            ready_marker_name: DEFAULT_READY_MARKER.to_owned(),
            ready_marker_shortcut: Duration::seconds(DEFAULT_READY_MARKER_SHORTCUT_SECONDS),
            inspection_concurrency: DEFAULT_INSPECTION_CONCURRENCY,
            ffprobe_path: PathBuf::from("ffprobe"),
            ffprobe_timeout: Duration::seconds(DEFAULT_FFPROBE_TIMEOUT_SECONDS),
            normalization_concurrency: DEFAULT_NORMALIZATION_CONCURRENCY,
            ffmpeg_path: PathBuf::from("ffmpeg"),
            ffmpeg_timeout: Duration::seconds(DEFAULT_FFMPEG_TIMEOUT_SECONDS),
            fpcalc_path: PathBuf::from("fpcalc"),
            fpcalc_timeout: Duration::seconds(DEFAULT_FPCALC_TIMEOUT_SECONDS),
            staging_root,
            work_area_base: PathBuf::from(DEFAULT_WORK_AREA_BASE),
            validation_concurrency: DEFAULT_VALIDATION_CONCURRENCY,
            publication_concurrency: DEFAULT_PUBLICATION_CONCURRENCY,
            storage_root: None,
            #[cfg(feature = "test-support")]
            publication_copy_test_barrier: None,
            #[cfg(feature = "test-support")]
            acquisition_test_hook: None,
        }
    }

    /// Read configuration only from the documented environment variables.
    pub fn from_env() -> Result<Self, IngestionSourceParseError> {
        let sources = parse_sources()?;
        let enabled = env::var(ENV_INGEST_ENABLED)
            .ok()
            .map(|value| parse_bool(ENV_INGEST_ENABLED, &value))
            .transpose()?;

        let enabled = enabled.unwrap_or(true);
        let staging_root = env::var_os(ENV_STAGING_ROOT).map(PathBuf::from);
        if enabled && !sources.is_empty() && staging_root.is_none() {
            return Err(IngestionSourceParseError {
                detail: format!(
                    "{ENV_STAGING_ROOT} is required when inbound ingestion has configured source roots; inbound is a transient inbox, not processing storage"
                ),
            });
        }

        Ok(Self {
            enabled,
            sources,
            reconciliation_interval: parse_positive_duration_seconds(
                ENV_RECONCILIATION_INTERVAL_SECONDS,
                DEFAULT_RECONCILIATION_INTERVAL_SECONDS,
            )?,
            settling_quiet_period: parse_non_negative_duration_seconds(
                ENV_SETTLING_QUIET_SECONDS,
                DEFAULT_SETTLING_QUIET_SECONDS,
            )?,
            ready_marker_name: env::var(ENV_READY_MARKER_NAME)
                .unwrap_or_else(|_| DEFAULT_READY_MARKER.to_owned()),
            ready_marker_shortcut: parse_non_negative_duration_seconds(
                ENV_READY_MARKER_SHORTCUT_SECONDS,
                DEFAULT_READY_MARKER_SHORTCUT_SECONDS,
            )?,
            inspection_concurrency: parse_positive_u32(
                ENV_INSPECTION_CONCURRENCY,
                DEFAULT_INSPECTION_CONCURRENCY,
            )?,
            ffprobe_path: env::var(ENV_FFPROBE_PATH)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("ffprobe")),
            ffprobe_timeout: parse_positive_duration_seconds(
                ENV_FFPROBE_TIMEOUT_SECONDS,
                DEFAULT_FFPROBE_TIMEOUT_SECONDS,
            )?,
            normalization_concurrency: parse_positive_u32(
                ENV_NORMALIZATION_CONCURRENCY,
                DEFAULT_NORMALIZATION_CONCURRENCY,
            )?,
            ffmpeg_path: env::var(ENV_FFMPEG_PATH)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("ffmpeg")),
            ffmpeg_timeout: parse_positive_duration_seconds(
                ENV_FFMPEG_TIMEOUT_SECONDS,
                DEFAULT_FFMPEG_TIMEOUT_SECONDS,
            )?,
            fpcalc_path: env::var(ENV_FPCALC_PATH)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("fpcalc")),
            fpcalc_timeout: parse_positive_duration_seconds(
                ENV_FPCALC_TIMEOUT_SECONDS,
                DEFAULT_FPCALC_TIMEOUT_SECONDS,
            )?,
            staging_root,
            work_area_base: env::var(ENV_WORK_AREA_BASE)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_WORK_AREA_BASE)),
            validation_concurrency: parse_positive_u32(
                ENV_VALIDATION_CONCURRENCY,
                DEFAULT_VALIDATION_CONCURRENCY,
            )?,
            publication_concurrency: parse_positive_u32(
                ENV_PUBLICATION_CONCURRENCY,
                DEFAULT_PUBLICATION_CONCURRENCY,
            )?,
            storage_root: env::var_os(ENV_STORAGE_ROOT).map(PathBuf::from),
            #[cfg(feature = "test-support")]
            publication_copy_test_barrier: None,
            #[cfg(feature = "test-support")]
            acquisition_test_hook: None,
        })
    }

    /// Resolves and validates roots used by publication without creating them.
    ///
    /// All roots must be disjoint. This prevents canonical writes from ever
    /// landing in the read-only inbound tree or normalization work area.
    pub fn publication_roots(&self) -> Result<Option<PublicationRoots>, IngestionSourceParseError> {
        let roots = self.storage_roots()?;
        let Some(storage_root) = roots.storage_root else {
            return Ok(None);
        };
        Ok(Some(PublicationRoots {
            storage_root,
            work_area_base: roots.work_area_base,
            source_roots: roots.source_roots,
        }))
    }

    /// Resolves every configured media/data root and proves inbound is
    /// disjoint from every writable destination. Physical staging is distinct
    /// from the existing logical ingestion state `Staged`.
    pub fn storage_roots(&self) -> Result<StorageRoots, IngestionSourceParseError> {
        let source_roots = self
            .sources
            .iter()
            .map(|source| resolve_configured_root(&source.root, "inbound source root"))
            .collect::<Result<Vec<_>, _>>()?;
        let work_area_base =
            resolve_configured_root(&self.work_area_base, "normalization work-area root")?;
        let staging_root = match &self.staging_root {
            Some(root) => Some(resolve_configured_root(root, "durable staging root")?),
            None if self.enabled() => {
                return Err(IngestionSourceParseError {
                    detail: format!(
                        "{ENV_STAGING_ROOT} is required when inbound ingestion is enabled"
                    ),
                });
            }
            None => None,
        };
        let storage_root = self
            .storage_root
            .as_ref()
            .map(|root| resolve_configured_root(root, "canonical storage root"))
            .transpose()?;

        let mut named_roots = Vec::with_capacity(source_roots.len() + 3);
        for source_root in &source_roots {
            named_roots.push(("inbound source root", source_root.as_path()));
        }
        if let Some(root) = &staging_root {
            named_roots.push(("durable staging root", root.as_path()));
        }
        named_roots.push(("normalization work-area root", work_area_base.as_path()));
        if let Some(root) = &storage_root {
            named_roots.push(("canonical storage root", root.as_path()));
        }
        for (index, (left_name, left)) in named_roots.iter().enumerate() {
            for (right_name, right) in named_roots.iter().skip(index + 1) {
                if paths_overlap(left, right) {
                    return Err(IngestionSourceParseError {
                        detail: format!(
                            "{left_name} {left:?} overlaps {right_name} {right:?}; configured storage roots must be disjoint"
                        ),
                    });
                }
            }
        }

        Ok(StorageRoots {
            staging_root,
            work_area_base,
            storage_root,
            source_roots,
        })
    }
}

/// Absolute, symlink-free, pairwise-disjoint publication roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationRoots {
    pub storage_root: PathBuf,
    pub work_area_base: PathBuf,
    pub source_roots: Vec<PathBuf>,
}

/// Absolute, symlink-free storage roots. Staging is absent only when inbound
/// ingestion is disabled or has no configured source roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRoots {
    pub staging_root: Option<PathBuf>,
    pub work_area_base: PathBuf,
    pub storage_root: Option<PathBuf>,
    pub source_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundSource {
    pub id: Option<Uuid>,
    pub root: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundSourceConfig {
    pub id: Option<Uuid>,
    pub root: PathBuf,
    pub name: Option<String>,
}

impl InboundSource {
    pub fn from_root(
        root: PathBuf,
        id: Option<Uuid>,
        name: Option<String>,
    ) -> Result<Self, IngestionSourceParseError> {
        let canonical_root = canonicalize_root(root)?;
        let fallback_name = canonical_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("inbound");

        Ok(Self {
            id,
            name: name.unwrap_or_else(|| fallback_name.to_owned()),
            root: canonical_root,
        })
    }

    pub fn from_config(config: InboundSourceConfig) -> Result<Self, IngestionSourceParseError> {
        Self::from_root(config.root, config.id, config.name)
    }
}

#[derive(Debug, Clone)]
pub struct IngestionSourceParseError {
    pub detail: String,
}

impl std::fmt::Display for IngestionSourceParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for IngestionSourceParseError {}

fn parse_sources() -> Result<Vec<InboundSource>, IngestionSourceParseError> {
    if let Ok(raw) = env::var(ENV_SOURCES_JSON) {
        let source_configs =
            serde_json::from_str::<Vec<InboundSourceConfig>>(&raw).map_err(|error| {
                IngestionSourceParseError {
                    detail: format!(
                        "{ENV_SOURCES_JSON} must be JSON array of {{ id?, root, name? }}: {error}"
                    ),
                }
            })?;

        return source_configs
            .into_iter()
            .map(InboundSource::from_config)
            .collect();
    }

    if let Ok(raw_root) = env::var(ENV_SOURCE_ROOT) {
        let root = PathBuf::from(raw_root);
        let id = env::var(ENV_SOURCE_LIBRARY_ID)
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok());
        let name = env::var(ENV_SOURCE_LIBRARY_NAME).ok();
        return Ok(vec![InboundSource::from_root(root, id, name)?]);
    }

    Ok(Vec::new())
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, IngestionSourceParseError> {
    raw.parse::<bool>()
        .map_err(|error| IngestionSourceParseError {
            detail: format!("{key} must be true/false: {error}"),
        })
}

fn parse_positive_duration_seconds(
    key: &str,
    default: i64,
) -> Result<Duration, IngestionSourceParseError> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    let value = raw.parse::<i64>().map_err(|_| IngestionSourceParseError {
        detail: format!("{key} must be a positive integer"),
    })?;
    if value <= 0 {
        return Err(IngestionSourceParseError {
            detail: format!("{key} must be positive"),
        });
    }
    Ok(Duration::seconds(value))
}

fn parse_non_negative_duration_seconds(
    key: &str,
    default: i64,
) -> Result<Duration, IngestionSourceParseError> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    let value = raw.parse::<i64>().map_err(|_| IngestionSourceParseError {
        detail: format!("{key} must be a non-negative integer"),
    })?;
    if value < 0 {
        return Err(IngestionSourceParseError {
            detail: format!("{key} must be zero or positive"),
        });
    }
    Ok(Duration::seconds(value))
}

fn parse_positive_u32(key: &str, default: u32) -> Result<u32, IngestionSourceParseError> {
    let raw = env::var(key).unwrap_or_else(|_| default.to_string());
    let value = raw.parse::<u32>().map_err(|_| IngestionSourceParseError {
        detail: format!("{key} must be a positive integer"),
    })?;
    if value == 0 {
        return Err(IngestionSourceParseError {
            detail: format!("{key} must be positive"),
        });
    }
    Ok(value)
}

fn canonicalize_root(root: PathBuf) -> Result<PathBuf, IngestionSourceParseError> {
    let absolute = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|_| IngestionSourceParseError {
                detail: "failed to read current directory for relative source root".to_owned(),
            })?
            .join(root)
    };

    let link_metadata = fs::symlink_metadata(&absolute).map_err(|_| IngestionSourceParseError {
        detail: format!("inbound source root not found: {absolute:?}"),
    })?;
    if link_metadata.file_type().is_symlink() {
        return Err(IngestionSourceParseError {
            detail: format!("inbound source root must not be a symlink: {absolute:?}"),
        });
    }
    let metadata = fs::metadata(&absolute).map_err(|_| IngestionSourceParseError {
        detail: format!("inbound source root not found: {absolute:?}"),
    })?;
    if !metadata.is_dir() {
        return Err(IngestionSourceParseError {
            detail: format!("inbound source root is not a directory: {absolute:?}"),
        });
    }

    absolute
        .canonicalize()
        .map_err(|_| IngestionSourceParseError {
            detail: format!("inbound source root is not readable: {absolute:?}"),
        })
}

fn resolve_configured_root(root: &Path, label: &str) -> Result<PathBuf, IngestionSourceParseError> {
    if !root.is_absolute() {
        return Err(IngestionSourceParseError {
            detail: format!("{label} must be an absolute path: {root:?}"),
        });
    }
    let absolute = root.to_path_buf();
    let normalized = normalize_absolute(&absolute).ok_or_else(|| IngestionSourceParseError {
        detail: format!("{label} is not a valid absolute path: {absolute:?}"),
    })?;
    if normalized.parent().is_none() {
        return Err(IngestionSourceParseError {
            detail: format!("{label} must not be the filesystem root"),
        });
    }

    let mut current = PathBuf::new();
    let mut missing_seen = false;
    for component in normalized.components() {
        current.push(component.as_os_str());
        if missing_seen {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(IngestionSourceParseError {
                        detail: format!("{label} contains a symlink component: {current:?}"),
                    });
                }
                if current == normalized && !metadata.is_dir() {
                    return Err(IngestionSourceParseError {
                        detail: format!("{label} is not a directory: {current:?}"),
                    });
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => missing_seen = true,
            Err(error) => {
                return Err(IngestionSourceParseError {
                    detail: format!("failed to inspect {label} {current:?}: {error}"),
                });
            }
        }
    }
    Ok(normalized)
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => result.push(prefix.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            Component::Normal(value) => result.push(value),
        }
    }
    result.is_absolute().then_some(result)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "melodee-config-{label}-{}",
            Uuid::now_v7().simple()
        ))
    }

    #[test]
    fn publication_is_opt_in() {
        let source_root = temporary_root("opt-in-source");
        fs::create_dir_all(&source_root).unwrap();
        let source = InboundSource::from_root(source_root.clone(), None, None).unwrap();
        let config = IngestionConfig::with_sources(vec![source]);

        assert!(config.enabled());
        assert!(!config.publication_enabled());
        assert_eq!(None, config.publication_roots().unwrap());

        fs::remove_dir_all(source_root).unwrap();
    }

    #[test]
    fn publication_roots_resolve_nonexistent_leaves() {
        let base = temporary_root("resolved");
        let source_root = base.join("inbound");
        fs::create_dir_all(&source_root).unwrap();
        let source = InboundSource::from_root(source_root.clone(), None, None).unwrap();
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.work_area_base = base.join("work/not-created");
        config.storage_root = Some(base.join("storage/not-created"));

        let roots = config.publication_roots().unwrap().unwrap();
        assert!(roots.storage_root.is_absolute());
        assert!(roots.work_area_base.is_absolute());
        assert_eq!(
            vec![source_root.canonicalize().unwrap()],
            roots.source_roots
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn publication_roots_reject_overlap() {
        let base = temporary_root("overlap");
        let source_root = base.join("inbound");
        fs::create_dir_all(&source_root).unwrap();
        let source = InboundSource::from_root(source_root.clone(), None, None).unwrap();
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.work_area_base = base.join("work");
        config.storage_root = Some(source_root.join("canonical"));

        let error = config.publication_roots().unwrap_err();
        assert!(error.detail.contains("overlaps"));

        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn publication_roots_reject_symlink_component() {
        use std::os::unix::fs::symlink;

        let base = temporary_root("symlink");
        let source_root = base.join("inbound");
        let real_storage = base.join("real-storage");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&real_storage).unwrap();
        let linked_storage = base.join("linked-storage");
        symlink(&real_storage, &linked_storage).unwrap();
        let source = InboundSource::from_root(source_root, None, None).unwrap();
        let mut config = IngestionConfig::with_sources(vec![source]);
        config.work_area_base = base.join("work");
        config.storage_root = Some(linked_storage);

        let error = config.publication_roots().unwrap_err();
        assert!(error.detail.contains("symlink component"));

        fs::remove_dir_all(base).unwrap();
    }
}
