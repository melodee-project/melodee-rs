//! Ingestion configuration and source definitions.

use std::{env, path::PathBuf};

use serde::{Deserialize, Serialize};
use time::Duration;
use uuid::Uuid;

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
pub const ENV_INSPECTION_CONCURRENCY: &str = "MELODEE__INSPECTION__CONCURRENCY";
pub const ENV_FFPROBE_PATH: &str = "MELODEE__INSPECTION__FFPROBE_PATH";
pub const ENV_FFPROBE_TIMEOUT_SECONDS: &str = "MELODEE__INSPECTION__FFPROBE_TIMEOUT_SECONDS";
pub const ENV_NORMALIZATION_CONCURRENCY: &str = "MELODEE__NORMALIZATION__CONCURRENCY";
pub const ENV_FFMPEG_PATH: &str = "MELODEE__NORMALIZATION__FFMPEG_PATH";
pub const ENV_FFMPEG_TIMEOUT_SECONDS: &str = "MELODEE__NORMALIZATION__FFMPEG_TIMEOUT_SECONDS";
pub const ENV_FPCALC_PATH: &str = "MELODEE__NORMALIZATION__FPCALC_PATH";
pub const ENV_FPCALC_TIMEOUT_SECONDS: &str = "MELODEE__NORMALIZATION__FPCALC_TIMEOUT_SECONDS";
pub const ENV_WORK_AREA_BASE: &str = "MELODEE__NORMALIZATION__WORK_AREA";

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
    pub work_area_base: PathBuf,
}

impl IngestionConfig {
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled && !self.sources.is_empty()
    }

    #[must_use]
    pub fn with_sources(sources: Vec<InboundSource>) -> Self {
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
            work_area_base: PathBuf::from(DEFAULT_WORK_AREA_BASE),
        }
    }

    /// Read configuration only from the documented environment variables.
    pub fn from_env() -> Result<Self, IngestionSourceParseError> {
        let sources = parse_sources()?;
        let enabled = env::var(ENV_INGEST_ENABLED)
            .ok()
            .map(|value| parse_bool(ENV_INGEST_ENABLED, &value))
            .transpose()?;

        Ok(Self {
            enabled: enabled.unwrap_or(true),
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
            work_area_base: env::var(ENV_WORK_AREA_BASE)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_WORK_AREA_BASE)),
        })
    }
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

    let metadata = std::fs::metadata(&absolute).map_err(|_| IngestionSourceParseError {
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
