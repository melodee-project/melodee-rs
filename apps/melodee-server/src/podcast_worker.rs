//! Bounded podcast feed refresh and enclosure download lifecycle.

use std::{
    collections::HashMap,
    env,
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use quick_xml::{Reader, events::Event};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use time::{
    OffsetDateTime,
    format_description::well_known::{Rfc2822, Rfc3339},
};
use tokio::{io::AsyncWriteExt, sync::watch, task::JoinHandle};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const ROOT_MARKER: &str = ".melodee-podcast-root";
const SETTING_MAX_FEED_BYTES: &str = "podcast.http.maxFeedBytes";
const SETTING_MAX_ITEMS_PER_CHANNEL: &str = "podcast.refresh.maxItemsPerChannel";
const SETTING_MAX_REDIRECTS: &str = "podcast.http.maxRedirects";
const SETTING_HTTP_TIMEOUT_SECONDS: &str = "podcast.http.timeoutSeconds";
const SETTING_MAX_ENCLOSURE_BYTES: &str = "podcast.download.maxEnclosureBytes";
const SETTING_MAX_CONCURRENT_GLOBAL: &str = "podcast.download.maxConcurrent.global";
const SETTING_MAX_CONCURRENT_PER_USER: &str = "podcast.download.maxConcurrent.perUser";
const SETTING_QUOTA_BYTES_PER_USER: &str = "podcast.quota.maxBytesPerUser";
const SETTING_RETENTION_DAYS: &str = "podcast.retention.downloadedEpisodesInDays";
const SETTING_RETENTION_KEEP_LAST_N: &str = "podcast.retention.keepLastNEpisodes";
const SETTING_RETENTION_KEEP_UNPLAYED_ONLY: &str = "podcast.retention.keepUnplayedOnly";
/// Legacy registry defaults (C# `MelodeeDbContext` seed values).
const DEFAULT_HTTP_TIMEOUT_SECONDS: u64 = 30;
const MAX_HTTP_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_MAX_ENCLOSURE_BYTES: u64 = 2_147_483_648;
const MAX_ENCLOSURE_BYTES_HARD_CAP: u64 = 4_294_967_296;
const DEFAULT_MAX_CONCURRENT_GLOBAL: i64 = 2;
const DEFAULT_MAX_CONCURRENT_PER_USER: i64 = 1;
const DEFAULT_QUOTA_BYTES_PER_USER: i64 = 5_368_709_120;
/// Hard safety caps for the operator-configured `podcast.http.maxFeedBytes`
/// and `podcast.refresh.maxItemsPerChannel` settings. Legacy C# read both
/// from configuration at refresh time (`PodcastService.RefreshChannelAsync`);
/// the defaults coincide with the legacy registry defaults, while the caps
/// keep one refresh bounded regardless of misconfiguration.
const MAX_FEED_BYTES_HARD_CAP: usize = 64 * 1024 * 1024;
const MAX_FEED_ITEMS_HARD_CAP: usize = 1000;
/// The configured-default fallbacks, matching the settings registry.
const DEFAULT_MAX_FEED_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_FEED_ITEMS: usize = 500;
const MAX_RECONCILE_FILES_PER_BATCH: usize = 256;
const MAX_RECONCILE_ENTRIES_PER_BATCH: usize = 1024;

/// Per-refresh feed bounds resolved from the layered settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedLimits {
    pub max_feed_bytes: usize,
    pub max_items_per_channel: usize,
    pub max_redirects: usize,
    /// Outbound HTTP timeout (`podcast.http.timeoutSeconds`, legacy default
    /// 30 s).
    pub timeout_seconds: u64,
}

impl Default for FeedLimits {
    fn default() -> Self {
        Self {
            max_feed_bytes: DEFAULT_MAX_FEED_BYTES,
            max_items_per_channel: DEFAULT_MAX_FEED_ITEMS,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            timeout_seconds: DEFAULT_HTTP_TIMEOUT_SECONDS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PodcastConfig {
    pub enabled: bool,
    pub discovery_endpoint: String,
    pub worker_enabled: bool,
    pub storage_root: Option<PathBuf>,
    pub poll_interval: Duration,
    pub claim_batch_size: i64,
    pub lease_duration: Duration,
    pub max_attempts: i32,
    pub max_download_bytes: u64,
    #[doc(hidden)]
    pub allow_loopback_http: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PodcastConfigError {
    #[error("invalid boolean for {0}")]
    Boolean(&'static str),
    #[error("invalid positive integer for {0}")]
    Integer(&'static str),
    #[error("MELODEE__PODCAST__STORAGE_ROOT is required when the podcast worker is enabled")]
    MissingRoot,
    #[error("podcast storage root must be an absolute, non-symlink directory")]
    InvalidRoot,
    #[error("non-empty podcast storage root is not owned by Melodee")]
    UnownedRoot,
    #[error("podcast storage root overlaps another configured Melodee root")]
    RootOverlap,
    #[error("podcast storage root operation failed")]
    RootIo(#[source] std::io::Error),
}

impl PodcastConfig {
    pub fn from_env() -> Result<Self, PodcastConfigError> {
        let enabled = env_bool("MELODEE__PODCAST__ENABLED", true)?;
        let worker_enabled = env_bool("MELODEE__PODCAST__WORKER__ENABLED", false)?;
        let storage_root = env::var("MELODEE__PODCAST__STORAGE_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);
        if enabled && worker_enabled && storage_root.is_none() {
            return Err(PodcastConfigError::MissingRoot);
        }
        let config = Self {
            enabled,
            discovery_endpoint: env::var("MELODEE__PODCAST__DISCOVERY_ENDPOINT")
                .unwrap_or_else(|_| "https://itunes.apple.com/".to_owned()),
            worker_enabled,
            storage_root,
            poll_interval: Duration::from_secs(env_u64(
                "MELODEE__PODCAST__WORKER__POLL_INTERVAL_SECONDS",
                30,
                1,
                86_400,
            )?),
            claim_batch_size: i64::from(env_i32("MELODEE__PODCAST__WORKER__BATCH_SIZE", 4, 1, 32)?),
            lease_duration: Duration::from_secs(env_u64(
                "MELODEE__PODCAST__WORKER__LEASE_SECONDS",
                300,
                60,
                3600,
            )?),
            max_attempts: env_i32("MELODEE__PODCAST__WORKER__MAX_ATTEMPTS", 5, 1, 100)?,
            max_download_bytes: env_u64(
                "MELODEE__PODCAST__MAX_DOWNLOAD_BYTES",
                536_870_912,
                1_048_576,
                4_294_967_296,
            )?,
            allow_loopback_http: false,
        };
        if let Some(root) = config.storage_root.as_deref() {
            if !root.is_absolute() {
                return Err(PodcastConfigError::InvalidRoot);
            }
            reject_symlink_components(root)?;
        }
        Ok(config)
    }

    pub fn reject_root_overlap(&self, other_roots: &[PathBuf]) -> Result<(), PodcastConfigError> {
        let Some(root) = self.storage_root.as_deref() else {
            return Ok(());
        };
        let root = resolve_candidate_path(root)?;
        for other in other_roots {
            let other = resolve_candidate_path(other)?;
            if root.starts_with(&other) || other.starts_with(&root) {
                return Err(PodcastConfigError::RootOverlap);
            }
        }
        Ok(())
    }

    /// Validate disjointness before writing the ownership marker.
    pub fn prepare_owned_root(&self, other_roots: &[PathBuf]) -> Result<(), PodcastConfigError> {
        self.reject_root_overlap(other_roots)?;
        if let Some(root) = self.storage_root.as_deref() {
            validate_owned_root(root)?;
        }
        Ok(())
    }

    /// Verify — creating nothing — that the configured storage root already is
    /// a Melodee-owned podcast root. Recovery tooling (CLI) must never
    /// initialize a directory or claim ownership of one; it may only operate
    /// on a root the server previously took ownership of.
    pub fn require_owned_root(&self) -> Result<(), PodcastConfigError> {
        let root = self
            .storage_root
            .as_deref()
            .ok_or(PodcastConfigError::MissingRoot)?;
        if !root.is_absolute() {
            return Err(PodcastConfigError::InvalidRoot);
        }
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
        {
            verify_root_marker(root)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        {
            reject_symlink_components(root)?;
            let metadata = std::fs::symlink_metadata(root).map_err(PodcastConfigError::RootIo)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(PodcastConfigError::InvalidRoot);
            }
            let marker = root.join(ROOT_MARKER);
            let marker_metadata =
                std::fs::symlink_metadata(&marker).map_err(|error| match error.kind() {
                    std::io::ErrorKind::NotFound => PodcastConfigError::UnownedRoot,
                    _ => PodcastConfigError::RootIo(error),
                })?;
            if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
                return Err(PodcastConfigError::InvalidRoot);
            }
            verify_root_marker(root)
        }
    }
}

impl Default for PodcastConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            discovery_endpoint: "https://itunes.apple.com/".to_owned(),
            worker_enabled: false,
            storage_root: None,
            poll_interval: Duration::from_secs(30),
            claim_batch_size: 4,
            lease_duration: Duration::from_secs(300),
            max_attempts: 5,
            max_download_bytes: 536_870_912,
            allow_loopback_http: false,
        }
    }
}

fn env_bool(key: &'static str, default: bool) -> Result<bool, PodcastConfigError> {
    match env::var(key) {
        Err(_) => Ok(default),
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(PodcastConfigError::Boolean(key)),
        },
    }
}

fn env_u64(key: &'static str, default: u64, min: u64, max: u64) -> Result<u64, PodcastConfigError> {
    let value = env::var(key)
        .ok()
        .map(|raw| raw.parse())
        .transpose()
        .map_err(|_| PodcastConfigError::Integer(key))?
        .unwrap_or(default);
    (min..=max)
        .contains(&value)
        .then_some(value)
        .ok_or(PodcastConfigError::Integer(key))
}

fn env_i32(key: &'static str, default: i32, min: i32, max: i32) -> Result<i32, PodcastConfigError> {
    let value = env::var(key)
        .ok()
        .map(|raw| raw.parse())
        .transpose()
        .map_err(|_| PodcastConfigError::Integer(key))?
        .unwrap_or(default);
    (min..=max)
        .contains(&value)
        .then_some(value)
        .ok_or(PodcastConfigError::Integer(key))
}

fn validate_owned_root(root: &Path) -> Result<(), PodcastConfigError> {
    if !root.is_absolute() {
        return Err(PodcastConfigError::InvalidRoot);
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    let directory = open_podcast_directory(root, true).map_err(PodcastConfigError::RootIo)?;
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        reject_symlink_components(root)?;
        std::fs::create_dir_all(root).map_err(PodcastConfigError::RootIo)?;
        reject_symlink_components(root)?;
        let metadata = std::fs::symlink_metadata(root).map_err(PodcastConfigError::RootIo)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(PodcastConfigError::InvalidRoot);
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    let marker_state = {
        use rustix::fs::{AtFlags, FileType, statat};
        match statat(&directory, ROOT_MARKER, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode).is_file() => Some(()),
            Ok(_) => return Err(PodcastConfigError::InvalidRoot),
            Err(error) if error == rustix::io::Errno::NOENT => None,
            Err(error) => return Err(PodcastConfigError::RootIo(std::io::Error::from(error))),
        }
    };
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    let marker_state = {
        let marker = root.join(ROOT_MARKER);
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Some(()),
            Ok(_) => return Err(PodcastConfigError::InvalidRoot),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(PodcastConfigError::RootIo(error)),
        }
    };
    if marker_state.is_some() {
        return verify_root_marker(root);
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    let is_empty = {
        use rustix::fs::Dir;
        let mut entries = Dir::read_from(&directory)
            .map_err(std::io::Error::from)
            .map_err(PodcastConfigError::RootIo)?;
        let mut empty = true;
        for entry in &mut entries {
            let entry = entry
                .map_err(std::io::Error::from)
                .map_err(PodcastConfigError::RootIo)?;
            if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
                empty = false;
                break;
            }
        }
        empty
    };
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    let is_empty = std::fs::read_dir(root)
        .map_err(PodcastConfigError::RootIo)?
        .next()
        .is_none();
    if !is_empty {
        return Err(PodcastConfigError::UnownedRoot);
    }
    create_root_marker(root)
}

fn resolve_candidate_path(path: &Path) -> Result<PathBuf, PodcastConfigError> {
    if !path.is_absolute() {
        return Err(PodcastConfigError::InvalidRoot);
    }
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or(PodcastConfigError::InvalidRoot)?;
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or(PodcastConfigError::InvalidRoot)?;
    }
    let mut resolved = std::fs::canonicalize(existing).map_err(PodcastConfigError::RootIo)?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn reject_symlink_components(path: &Path) -> Result<(), PodcastConfigError> {
    let mut component_path = PathBuf::new();
    for component in path.components() {
        component_path.push(component.as_os_str());
        match std::fs::symlink_metadata(&component_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PodcastConfigError::InvalidRoot);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(PodcastConfigError::RootIo(error)),
        }
    }
    Ok(())
}

/// Opens an absolute podcast root one component at a time. Creation is done
/// with `mkdirat` relative to the already-open parent, so a concurrent
/// replacement cannot redirect ownership initialization through a symlink.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn open_podcast_directory(root: &Path, create: bool) -> std::io::Result<std::fs::File> {
    use rustix::fs::{CWD, Mode, OFlags, mkdirat, openat};

    if !root.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "podcast root must be absolute",
        ));
    }
    let mut current = openat(
        CWD,
        b"/".as_slice(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    for component in root.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "podcast root contains traversal",
            ));
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        match openat(&current, name, flags, Mode::empty()) {
            Ok(next) => current = next,
            Err(error) if create && error == rustix::io::Errno::NOENT => {
                match mkdirat(&current, name, Mode::RWXU | Mode::RGRP | Mode::XGRP) {
                    Ok(()) => {}
                    Err(error) if error == rustix::io::Errno::EXIST => {}
                    Err(error) => return Err(std::io::Error::from(error)),
                }
                current =
                    openat(&current, name, flags, Mode::empty()).map_err(std::io::Error::from)?;
            }
            Err(error) => return Err(std::io::Error::from(error)),
        }
    }
    Ok(std::fs::File::from(current))
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn verify_root_marker(root: &Path) -> Result<(), PodcastConfigError> {
    use rustix::fs::{Mode, OFlags, openat};
    let directory = open_podcast_directory(root, false).map_err(PodcastConfigError::RootIo)?;
    let marker = openat(
        &directory,
        ROOT_MARKER,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)
    .map_err(PodcastConfigError::RootIo)?;
    let mut marker = std::fs::File::from(marker);
    let mut bytes = Vec::with_capacity(25);
    std::io::Read::read_to_end(&mut marker, &mut bytes).map_err(PodcastConfigError::RootIo)?;
    if bytes == b"melodee-podcast-root-v1\n" {
        Ok(())
    } else {
        Err(PodcastConfigError::UnownedRoot)
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn create_root_marker(root: &Path) -> Result<(), PodcastConfigError> {
    use rustix::fs::{Mode, OFlags, openat};
    let directory = open_podcast_directory(root, false).map_err(PodcastConfigError::RootIo)?;
    let marker = openat(
        &directory,
        ROOT_MARKER,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(std::io::Error::from)
    .map_err(PodcastConfigError::RootIo)?;
    let mut marker = std::fs::File::from(marker);
    std::io::Write::write_all(&mut marker, b"melodee-podcast-root-v1\n")
        .map_err(PodcastConfigError::RootIo)?;
    std::io::Write::flush(&mut marker).map_err(PodcastConfigError::RootIo)?;
    marker.sync_all().map_err(PodcastConfigError::RootIo)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn verify_root_marker(root: &Path) -> Result<(), PodcastConfigError> {
    let marker = root.join(ROOT_MARKER);
    let bytes = std::fs::read(marker).map_err(PodcastConfigError::RootIo)?;
    if bytes == b"melodee-podcast-root-v1\n" {
        Ok(())
    } else {
        Err(PodcastConfigError::UnownedRoot)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn create_root_marker(root: &Path) -> Result<(), PodcastConfigError> {
    use std::io::Write as _;
    let mut marker = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(ROOT_MARKER))
        .map_err(PodcastConfigError::RootIo)?;
    marker
        .write_all(b"melodee-podcast-root-v1\n")
        .map_err(PodcastConfigError::RootIo)?;
    marker.flush().map_err(PodcastConfigError::RootIo)
}

#[derive(Debug, thiserror::Error)]
pub enum PodcastWorkerError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("settings resolution failed")]
    Settings(#[from] melodee_persistence::SettingsError),
    #[error("outbound podcast request failed")]
    Http(#[from] reqwest::Error),
    #[error("podcast response was invalid or exceeded a configured bound")]
    InvalidResponse,
    #[error("podcast filesystem operation failed")]
    Io(#[from] std::io::Error),
}

fn http_client(
    host: &str,
    resolved: &[SocketAddr],
    timeout_seconds: u64,
) -> Result<reqwest::Client, PodcastWorkerError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5.min(timeout_seconds.max(1))))
        .timeout(Duration::from_secs(timeout_seconds.max(1)))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("melodee/", env!("CARGO_PKG_VERSION")))
        .resolve_to_addrs(host, resolved)
        .build()
        .map_err(PodcastWorkerError::Http)
}

/// Legacy C# follows redirects manually (`PodcastHttpClient.GetAsync`):
/// every hop re-runs SSRF validation (scheme/loopback policy, DNS public-
/// address checks) and the hop count is bounded by the configured
/// `podcast.http.maxRedirects` (default 10). Real enclosure/feed hosts
/// (e.g. blubrry/libsyn CDN fronts) routinely 302, so redirects must be
/// followed with the same per-hop protections, not rejected. The client
/// keeps `Policy::none()`: redirect targets are validated and DNS-pinned
/// here, hop by hop, exactly like the legacy `ValidateRedirectAsync` chain.
const MAX_REDIRECTS_HARD_CAP: usize = 10;
const DEFAULT_MAX_REDIRECTS: usize = 10;

/// Pure clamp/fallback for the configured redirect bound; non-positive
/// values are treated as unset (legacy default), never as "unbounded".
fn max_redirects_from_value(value: Option<i64>) -> usize {
    value
        .filter(|redirects| *redirects > 0)
        .map(|redirects| usize::try_from(redirects).unwrap_or(MAX_REDIRECTS_HARD_CAP))
        .unwrap_or(DEFAULT_MAX_REDIRECTS)
        .clamp(0, MAX_REDIRECTS_HARD_CAP)
}

/// One validated, DNS-pinned GET with bounded, SSRF-validated redirect
/// following (legacy `PodcastHttpClient.GetAsync` redirect loop).
async fn outbound_get_following(
    value: &str,
    allow_loopback_http: bool,
    max_redirects: usize,
    timeout_seconds: u64,
) -> Result<reqwest::Response, PodcastWorkerError> {
    let mut current =
        reqwest::Url::parse(value).map_err(|_| PodcastWorkerError::InvalidResponse)?;
    let mut redirects = 0_usize;
    loop {
        let url = validate_outbound_url(current.as_str(), allow_loopback_http)?;
        let host = url.host_str().ok_or(PodcastWorkerError::InvalidResponse)?;
        let port = url
            .port_or_known_default()
            .ok_or(PodcastWorkerError::InvalidResponse)?;
        let resolved = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| PodcastWorkerError::InvalidResponse)??
        .take(17)
        .collect::<Vec<_>>();
        if resolved.is_empty()
            || resolved.len() > 16
            || resolved
                .iter()
                .any(|address| !allow_loopback_http && !is_public_address(address.ip()))
        {
            return Err(PodcastWorkerError::InvalidResponse);
        }
        let response = http_client(host, &resolved, timeout_seconds)?
            .get(url.clone())
            .send()
            .await?;
        if response.status().is_redirection() {
            if redirects >= max_redirects {
                return Err(PodcastWorkerError::InvalidResponse);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|header| header.to_str().ok())
                .ok_or(PodcastWorkerError::InvalidResponse)?;
            current = url
                .join(location)
                .map_err(|_| PodcastWorkerError::InvalidResponse)?;
            redirects += 1;
            continue;
        }
        return Ok(response);
    }
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [first, second, third, _] = address.octets();
            !(first == 0
                || first == 10
                || first == 127
                || (first == 100 && (64..=127).contains(&second))
                || (first == 169 && second == 254)
                || (first == 172 && (16..=31).contains(&second))
                || (first == 192
                    && (second == 0 || second == 168 || (second == 88 && third == 99)))
                || (first == 198 && (18..=19).contains(&second))
                || (first == 198 && second == 51 && third == 100)
                || (first == 203 && second == 0 && third == 113)
                || first >= 224)
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(mapped));
            }
            let segments = address.segments();
            let first = segments[0];
            let embedded_translation = first == 0x0064 && segments[1] == 0xff9b;
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && (first & 0xe000) == 0x2000
                && (first & 0xfe00) != 0xfc00
                && (first & 0xffc0) != 0xfe80
                && (first & 0xffc0) != 0xfec0
                && !embedded_translation
                // The IETF protocol-assignment block contains many
                // non-routable transition/special-purpose subnets. Reject the
                // whole block instead of maintaining an incomplete denylist.
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && first != 0x2002
                && !(first == 0x3fff && (segments[1] & 0xf000) == 0)
        }
    }
}

fn validate_outbound_url(
    value: &str,
    allow_loopback_http: bool,
) -> Result<reqwest::Url, PodcastWorkerError> {
    let url = reqwest::Url::parse(value).map_err(|_| PodcastWorkerError::InvalidResponse)?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() == "https" || (url.scheme() == "http" && (!loopback || allow_loopback_http)) {
        Ok(url)
    } else {
        Err(PodcastWorkerError::InvalidResponse)
    }
}

async fn fetch_bounded(
    url: &str,
    maximum: usize,
    allow_loopback_http: bool,
    max_redirects: usize,
    timeout_seconds: u64,
) -> Result<Vec<u8>, PodcastWorkerError> {
    let response =
        outbound_get_following(url, allow_loopback_http, max_redirects, timeout_seconds).await?;
    // Fail closed on 206: the worker never sends `Range`, so a partial
    // response is by definition not the complete feed and must not be
    // parsed as one.
    if !response.status().is_success()
        || response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        || response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
    {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(PodcastWorkerError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Debug, Default)]
struct Feed {
    title: Option<String>,
    description: Option<String>,
    website: Option<String>,
    image_url: Option<String>,
    episodes: Vec<FeedEpisode>,
}

#[derive(Debug, Default)]
struct FeedEpisode {
    feed_item_id: String,
    title: String,
    description: String,
    published_at: Option<OffsetDateTime>,
    duration_ms: Option<i64>,
    enclosure_url: Option<String>,
}

fn parse_feed_with_limits(bytes: &[u8], max_items: usize) -> Result<Feed, PodcastWorkerError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut feed = Feed::default();
    let mut episode: Option<FeedEpisode> = None;
    let mut current = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                current = start.local_name().as_ref().to_ascii_lowercase();
                if matches!(current.as_str(), "item" | "entry") {
                    if feed.episodes.len() >= max_items {
                        // Legacy parity: `feed.Items.Take(maxItemsPerChannel)`
                        // truncates rather than failing the whole refresh.
                        // Stop collecting so the trailing channel markup
                        // cannot create an episode.
                        break;
                    }
                    episode = Some(FeedEpisode::default());
                }
            }
            Ok(Event::Empty(empty)) => {
                let name = empty.local_name().as_ref().to_ascii_lowercase();
                if name == "enclosure" || name == "link" {
                    for attribute in empty.attributes().flatten() {
                        let key = attribute.key.local_name();
                        if matches!(key.as_ref(), "url" | "href") {
                            let value = attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map_err(|_| PodcastWorkerError::InvalidResponse)?
                                .into_owned();
                            if let Some(item) = episode.as_mut() {
                                item.enclosure_url = Some(value);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(text)) => apply_feed_text(
                &mut feed,
                episode.as_mut(),
                &current,
                text.into_inner().into_owned(),
            ),
            Ok(Event::CData(text)) => apply_feed_text(
                &mut feed,
                episode.as_mut(),
                &current,
                text.into_inner().into_owned(),
            ),
            Ok(Event::End(end)) => {
                let name = end.local_name().as_ref().to_ascii_lowercase();
                if matches!(name.as_str(), "item" | "entry") {
                    let mut item = episode.take().ok_or(PodcastWorkerError::InvalidResponse)?;
                    if item.title.trim().is_empty() {
                        return Err(PodcastWorkerError::InvalidResponse);
                    }
                    if item.feed_item_id.trim().is_empty() {
                        item.feed_item_id = item.enclosure_url.clone().unwrap_or_else(|| {
                            let digest =
                                Sha256::digest(format!("{}|{:?}", item.title, item.published_at));
                            digest.iter().map(|byte| format!("{byte:02x}")).collect()
                        });
                    }
                    feed.episodes.push(item);
                }
                current.clear();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(PodcastWorkerError::InvalidResponse),
        }
    }
    // Defensive: with the collect-then-break above this cannot trigger, but
    // the legacy contract is "at most max items", never a failed refresh.
    feed.episodes.truncate(max_items);
    Ok(feed)
}

fn apply_feed_text(
    feed: &mut Feed,
    episode: Option<&mut FeedEpisode>,
    current: &str,
    value: String,
) {
    if let Some(item) = episode {
        match current {
            "guid" | "id" => item.feed_item_id = value,
            "title" => item.title = value,
            "description" | "summary" | "content" => item.description = value,
            "pubdate" | "published" | "updated" => {
                item.published_at = OffsetDateTime::parse(&value, &Rfc2822)
                    .or_else(|_| OffsetDateTime::parse(&value, &Rfc3339))
                    .ok();
            }
            "duration" => item.duration_ms = parse_duration(&value),
            _ => {}
        }
    } else {
        match current {
            "title" if feed.title.is_none() => feed.title = Some(value),
            "description" | "subtitle" if feed.description.is_none() => {
                feed.description = Some(value);
            }
            "link" if feed.website.is_none() => feed.website = Some(value),
            "url" if feed.image_url.is_none() => feed.image_url = Some(value),
            _ => {}
        }
    }
}

fn parse_duration(value: &str) -> Option<i64> {
    let parts = value
        .split(':')
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let seconds = match parts.as_slice() {
        [seconds] => *seconds,
        [minutes, seconds] => minutes.checked_mul(60)?.checked_add(*seconds)?,
        [hours, minutes, seconds] => hours
            .checked_mul(3600)?
            .checked_add(minutes.checked_mul(60)?)?
            .checked_add(*seconds)?,
        _ => return None,
    };
    seconds.checked_mul(1000).filter(|value| *value >= 0)
}

/// Resolves the operator-configured feed bounds from the layered settings,
/// clamped to hard caps (legacy C# read both at refresh time). A settings
/// failure is fatal for the refresh attempt: proceeding with the smaller
/// legacy-derived defaults could reject a feed the operator explicitly
/// sized the deployment for.
pub async fn resolve_feed_limits(pool: &PgPool) -> Result<FeedLimits, PodcastWorkerError> {
    let resolver = melodee_persistence::SettingsResolver::new(pool);
    let bytes = resolver
        .resolve(SETTING_MAX_FEED_BYTES)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let items = resolver
        .resolve(SETTING_MAX_ITEMS_PER_CHANNEL)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let redirects = resolver
        .resolve(SETTING_MAX_REDIRECTS)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let timeout = resolver
        .resolve(SETTING_HTTP_TIMEOUT_SECONDS)
        .await?
        .value
        .and_then(|value| value.as_i64());
    Ok(feed_limits_from_values(bytes, items, redirects, timeout))
}

/// Pure clamp/fallback mapping from raw setting values to effective limits.
/// Absent/non-numeric values fall back to the legacy defaults; values are
/// clamped into `[1, hard_cap]` so a misconfiguration cannot remove the
/// refresh bounds (negative/zero values are treated as unset).
fn feed_limits_from_values(
    bytes: Option<i64>,
    items: Option<i64>,
    redirects: Option<i64>,
    timeout_seconds: Option<i64>,
) -> FeedLimits {
    // Non-positive values are treated as unset (fall back to the legacy
    // defaults) rather than clamped to 1: a zero bound would make every
    // refresh fail, which is never a useful operator intent.
    let effective_bytes = bytes
        .filter(|value| *value > 0)
        .map(|value| usize::try_from(value).unwrap_or(MAX_FEED_BYTES_HARD_CAP))
        .unwrap_or(DEFAULT_MAX_FEED_BYTES)
        .clamp(1, MAX_FEED_BYTES_HARD_CAP);
    let effective_items = items
        .filter(|value| *value > 0)
        .map(|value| usize::try_from(value).unwrap_or(MAX_FEED_ITEMS_HARD_CAP))
        .unwrap_or(DEFAULT_MAX_FEED_ITEMS)
        .clamp(1, MAX_FEED_ITEMS_HARD_CAP);
    let effective_timeout = timeout_seconds
        .filter(|value| *value > 0)
        .map_or(DEFAULT_HTTP_TIMEOUT_SECONDS, |value| {
            u64::try_from(value).unwrap_or(MAX_HTTP_TIMEOUT_SECONDS)
        })
        .clamp(1, MAX_HTTP_TIMEOUT_SECONDS);
    FeedLimits {
        max_feed_bytes: effective_bytes,
        max_items_per_channel: effective_items,
        timeout_seconds: effective_timeout,
        max_redirects: max_redirects_from_value(redirects),
    }
}

/// Runtime download/retention policy resolved from the layered settings
/// (`podcast.download.*`, `podcast.retention.*`, ADR-0161 operator surface).
/// All bounds are *upper* bounds: the process keeps its own conservative
/// ceiling, so a misconfigured value can never remove a safety limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodcastPolicy {
    /// Effective maximum enclosure size: the smaller of the process cap
    /// (env/`PodcastConfig`) and the configured setting, capped at 4 GiB.
    pub max_enclosure_bytes: u64,
    /// Maximum claimed downloads per pass
    /// (`podcast.download.maxConcurrent.global`, legacy default 2).
    pub download_concurrency: usize,
    /// Maximum concurrently-downloading episodes per user
    /// (`podcast.download.maxConcurrent.perUser`, legacy default 1; `0`
    /// disables the per-user bound).
    pub per_user_concurrency: i64,
    /// Per-user downloaded-bytes quota (`podcast.quota.maxBytesPerUser`,
    /// legacy default 5 GiB). Claims stop at the quota; `0` disables it.
    pub quota_bytes_per_user: i64,
    /// Legacy cleanup policies (`PodcastCleanupJob`): delete downloads older
    /// than N days (`0` disables).
    pub retention_days: i64,
    /// Keep only the newest N downloaded episodes per channel (`0` keeps the
    /// per-channel bounds only).
    pub retention_keep_last_n: i64,
    /// Delete episodes the user has already played (`is_now_playing = false`
    /// history rows).
    pub retention_keep_unplayed_only: bool,
}

impl PodcastPolicy {
    #[must_use]
    pub const fn fallback(process_max_bytes: u64) -> Self {
        Self {
            max_enclosure_bytes: process_max_bytes,
            download_concurrency: DEFAULT_MAX_CONCURRENT_GLOBAL as usize,
            per_user_concurrency: DEFAULT_MAX_CONCURRENT_PER_USER,
            quota_bytes_per_user: DEFAULT_QUOTA_BYTES_PER_USER,
            retention_days: 0,
            retention_keep_last_n: 0,
            retention_keep_unplayed_only: false,
        }
    }

    /// Pure mapping from raw setting values plus the process cap. Exposed for
    /// unit tests so the clamp/fallback rules are locked.
    #[must_use]
    pub fn from_inputs(process_max_bytes: u64, inputs: PodcastPolicyInputs) -> Self {
        let setting_bytes = inputs
            .max_enclosure_bytes
            .filter(|value| *value > 0)
            .map_or(DEFAULT_MAX_ENCLOSURE_BYTES, |value| {
                u64::try_from(value).unwrap_or(MAX_ENCLOSURE_BYTES_HARD_CAP)
            })
            .min(MAX_ENCLOSURE_BYTES_HARD_CAP);
        let download_concurrency = inputs
            .global_concurrency
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_GLOBAL)
            .clamp(1, 32);
        let per_user_concurrency = inputs
            .per_user_concurrency
            .filter(|value| *value >= 0)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_PER_USER)
            .clamp(0, 32);
        let quota_bytes_per_user = inputs
            .quota_bytes_per_user
            .filter(|value| *value >= 0)
            .unwrap_or(DEFAULT_QUOTA_BYTES_PER_USER);
        Self {
            max_enclosure_bytes: process_max_bytes.min(setting_bytes).max(1),
            download_concurrency: usize::try_from(download_concurrency).unwrap_or(1),
            per_user_concurrency,
            quota_bytes_per_user,
            retention_days: inputs
                .retention_days
                .filter(|value| *value > 0)
                .unwrap_or(0),
            retention_keep_last_n: inputs.keep_last_n.filter(|value| *value > 0).unwrap_or(0),
            retention_keep_unplayed_only: inputs.keep_unplayed_only.unwrap_or(false),
        }
    }
}

/// Raw setting values feeding [`PodcastPolicy::from_inputs`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PodcastPolicyInputs {
    pub max_enclosure_bytes: Option<i64>,
    pub global_concurrency: Option<i64>,
    pub per_user_concurrency: Option<i64>,
    pub quota_bytes_per_user: Option<i64>,
    pub retention_days: Option<i64>,
    pub keep_last_n: Option<i64>,
    pub keep_unplayed_only: Option<bool>,
}

/// Resolves [`PodcastPolicy`] from the layered settings store.
pub async fn resolve_podcast_policy(
    pool: &PgPool,
    config: &PodcastConfig,
) -> Result<PodcastPolicy, PodcastWorkerError> {
    let resolver = melodee_persistence::SettingsResolver::new(pool);
    let max_enclosure = resolver
        .resolve(SETTING_MAX_ENCLOSURE_BYTES)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let global = resolver
        .resolve(SETTING_MAX_CONCURRENT_GLOBAL)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let per_user = resolver
        .resolve(SETTING_MAX_CONCURRENT_PER_USER)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let quota = resolver
        .resolve(SETTING_QUOTA_BYTES_PER_USER)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let days = resolver
        .resolve(SETTING_RETENTION_DAYS)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let keep_last_n = resolver
        .resolve(SETTING_RETENTION_KEEP_LAST_N)
        .await?
        .value
        .and_then(|value| value.as_i64());
    let keep_unplayed = resolver
        .resolve(SETTING_RETENTION_KEEP_UNPLAYED_ONLY)
        .await?
        .value
        .and_then(|value| value.as_bool());
    Ok(PodcastPolicy::from_inputs(
        config.max_download_bytes,
        PodcastPolicyInputs {
            max_enclosure_bytes: max_enclosure,
            global_concurrency: global,
            per_user_concurrency: per_user,
            quota_bytes_per_user: quota,
            retention_days: days,
            keep_last_n,
            keep_unplayed_only: keep_unplayed,
        },
    ))
}

pub async fn refresh_channel(
    pool: &PgPool,
    owner: Uuid,
    channel_id: i64,
    allow_loopback_http: bool,
) -> Result<(), PodcastWorkerError> {
    let feed_url = sqlx::query_scalar::<_, String>(
        "SELECT feed_url FROM native_podcast_channels WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    ).bind(channel_id).bind(owner).fetch_optional(pool).await?
        .ok_or(PodcastWorkerError::InvalidResponse)?;
    let limits = resolve_feed_limits(pool).await?;
    let parsed = parse_feed_with_limits(
        &fetch_bounded(
            &feed_url,
            limits.max_feed_bytes,
            allow_loopback_http,
            limits.max_redirects,
            limits.timeout_seconds,
        )
        .await?,
        limits.max_items_per_channel,
    )?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE native_podcast_channels SET title=COALESCE($3,title),description=COALESCE($4,description),\
         website=COALESCE($5,website),image_url=COALESCE($6,image_url),last_sync_at=now(),updated_at=now() \
         WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    ).bind(channel_id).bind(owner).bind(parsed.title).bind(parsed.description)
        .bind(parsed.website).bind(parsed.image_url).execute(&mut *tx).await?;
    let auto_download = sqlx::query_scalar::<_, bool>(
        "SELECT auto_download_enabled FROM native_podcast_channels
         WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    )
    .bind(channel_id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(PodcastWorkerError::InvalidResponse)?;
    for item in parsed.episodes {
        sqlx::query(
            "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,description,\
             published_at,duration_ms,enclosure_url,download_status,created_at,updated_at) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,CASE WHEN $9 AND $8::text IS NOT NULL THEN 'Queued' ELSE 'NotDownloaded' END,now(),now()) \
             ON CONFLICT(channel_id,feed_item_id) DO UPDATE SET title=EXCLUDED.title,description=EXCLUDED.description,\
             published_at=EXCLUDED.published_at,duration_ms=EXCLUDED.duration_ms,enclosure_url=EXCLUDED.enclosure_url,updated_at=now()",
        ).bind(Uuid::now_v7()).bind(channel_id).bind(item.feed_item_id).bind(item.title)
            .bind(item.description).bind(item.published_at).bind(item.duration_ms)
            .bind(item.enclosure_url).bind(auto_download).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, FromRow)]
struct DownloadClaim {
    id: i64,
    public_api_id: Uuid,
    enclosure_url: String,
    download_attempt_count: i32,
}

async fn claim_download(
    pool: &PgPool,
    config: &PodcastConfig,
    worker: Uuid,
    policy: &PodcastPolicy,
) -> Result<Option<DownloadClaim>, PodcastWorkerError> {
    let lease_seconds = i64::try_from(config.lease_duration.as_secs())
        .map_err(|_| PodcastWorkerError::InvalidResponse)?;
    Ok(sqlx::query_as::<_, DownloadClaim>(
        "WITH claimed AS (SELECT episode.id FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.enclosure_url IS NOT NULL AND episode.soft_deleted_at IS NULL
         AND channel.soft_deleted_at IS NULL AND episode.download_available_at<=now()
         AND (episode.download_status='Queued' OR
              (episode.download_status='Downloading' AND episode.download_lease_expires_at<=now())) \
         AND ($3::bigint <= 0 OR (SELECT count(*) FROM native_podcast_episodes active
              JOIN native_podcast_channels active_channel ON active_channel.id=active.channel_id
              WHERE active_channel.owner_user_id=channel.owner_user_id
                AND active.download_status='Downloading'
                AND active.download_lease_expires_at > now()) < $3::bigint) \
         AND ($4::bigint <= 0 OR (SELECT coalesce(sum(used.downloaded_bytes),0) \
              FROM native_podcast_episodes used \
              JOIN native_podcast_channels used_channel ON used_channel.id=used.channel_id \
              WHERE used_channel.owner_user_id=channel.owner_user_id \
                AND used.download_status='Downloaded') < $4::bigint) \
         ORDER BY episode.download_available_at,episode.id FOR UPDATE OF episode SKIP LOCKED LIMIT 1) \
         UPDATE native_podcast_episodes e SET download_status='Downloading',download_lease_owner=$1,\
         download_lease_expires_at=now()+make_interval(secs=>$2::double precision),updated_at=now() \
         FROM claimed WHERE e.id=claimed.id RETURNING e.id,e.public_api_id,e.enclosure_url,e.download_attempt_count",
    )
    .bind(worker)
    .bind(lease_seconds as f64)
    .bind(policy.per_user_concurrency)
    .bind(policy.quota_bytes_per_user)
    .fetch_optional(pool)
    .await?)
}

async fn download_one(
    pool: &PgPool,
    config: &PodcastConfig,
    policy: &PodcastPolicy,
    worker: Uuid,
    claim: DownloadClaim,
) -> Result<(), PodcastWorkerError> {
    let root = config
        .storage_root
        .as_deref()
        .ok_or(PodcastWorkerError::InvalidResponse)?;
    let temporary = root.join(format!("{}.{}.tmp", claim.public_api_id, worker));
    let final_path = root.join(format!("{}.media", claim.public_api_id));
    if let Ok(metadata) = tokio::fs::symlink_metadata(&final_path).await {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(PodcastWorkerError::InvalidResponse);
        }
        return publish_download(
            pool,
            worker,
            &claim,
            root,
            None,
            &final_path,
            metadata.len(),
        )
        .await;
    }
    remove_owned_file(root, &temporary).await?;
    let feed_limits = resolve_feed_limits(pool).await?;
    let response = outbound_get_following(
        &claim.enclosure_url,
        config.allow_loopback_http,
        feed_limits.max_redirects,
        feed_limits.timeout_seconds,
    )
    .await?;
    // Fail closed on 206: without a sent `Range` a partial enclosure is not
    // authoritative bytes and must never be published as complete media.
    if !response.status().is_success()
        || response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        || response
            .content_length()
            .is_some_and(|length| length > policy.max_enclosure_bytes)
    {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    let mut file = create_owned_temp(root, &temporary).await?;
    let mut total = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or(PodcastWorkerError::InvalidResponse)?;
        if total > policy.max_enclosure_bytes {
            drop(file);
            remove_owned_file(root, &temporary).await?;
            return Err(PodcastWorkerError::InvalidResponse);
        }
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    publish_download(
        pool,
        worker,
        &claim,
        root,
        Some(&temporary),
        &final_path,
        total,
    )
    .await
}

async fn publish_download(
    pool: &PgPool,
    worker: Uuid,
    claim: &DownloadClaim,
    root: &Path,
    temporary: Option<&Path>,
    final_path: &Path,
    total: u64,
) -> Result<(), PodcastWorkerError> {
    let mut tx = pool.begin().await?;
    let owns_lease = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.id=$1 AND episode.soft_deleted_at IS NULL
         AND channel.soft_deleted_at IS NULL AND episode.download_lease_owner=$2
         AND episode.download_status='Downloading'
         AND episode.download_lease_expires_at>now() FOR UPDATE OF episode)",
    )
    .bind(claim.id)
    .bind(worker)
    .fetch_one(&mut *tx)
    .await?;
    if !owns_lease {
        if let Some(temporary) = temporary {
            remove_owned_file(root, temporary).await?;
        }
        tx.rollback().await?;
        return Err(PodcastWorkerError::InvalidResponse);
    }
    if let Some(temporary) = temporary {
        match rename_owned_noreplace(root, temporary, final_path).await {
            Ok(()) => {}
            Err(PodcastWorkerError::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                let metadata = tokio::fs::symlink_metadata(final_path).await?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    tx.rollback().await?;
                    return Err(PodcastWorkerError::InvalidResponse);
                }
                remove_owned_file(root, temporary).await?;
            }
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        }
    }
    let changed = sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloaded',local_path=$4,downloaded_bytes=$5,\
         downloaded_at=now(),download_error=NULL,download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         WHERE id=$1 AND download_lease_owner=$2 AND download_status='Downloading'",
    ).bind(claim.id).bind(worker).bind(claim.download_attempt_count)
        .bind(final_path.to_string_lossy().as_ref()).bind(i64::try_from(total).map_err(|_| PodcastWorkerError::InvalidResponse)?)
        .execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        tx.rollback().await?;
        return Err(PodcastWorkerError::InvalidResponse);
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
async fn create_owned_temp(
    root: &Path,
    path: &Path,
) -> Result<tokio::fs::File, PodcastWorkerError> {
    use rustix::fs::{Mode, OFlags, openat};
    if path.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    let name = path
        .file_name()
        .ok_or(PodcastWorkerError::InvalidResponse)?;
    // Hold every ancestor with O_NOFOLLOW. Opening `root` by pathname with
    // O_NOFOLLOW would still permit an ancestor replacement to redirect the
    // download into an operator-managed tree.
    let directory = open_podcast_directory(root, false).map_err(PodcastWorkerError::Io)?;
    let file = openat(
        &directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(std::io::Error::from)?;
    Ok(tokio::fs::File::from_std(std::fs::File::from(file)))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
async fn create_owned_temp(
    root: &Path,
    path: &Path,
) -> Result<tokio::fs::File, PodcastWorkerError> {
    if path.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    Ok(tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
async fn rename_owned_noreplace(
    root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), PodcastWorkerError> {
    use rustix::fs::{RenameFlags, renameat_with};
    if source.parent() != Some(root) || destination.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    let directory = open_podcast_directory(root, false).map_err(PodcastWorkerError::Io)?;
    renameat_with(
        &directory,
        source
            .file_name()
            .ok_or(PodcastWorkerError::InvalidResponse)?,
        &directory,
        destination
            .file_name()
            .ok_or(PodcastWorkerError::InvalidResponse)?,
        RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    Ok(())
}

#[cfg(windows)]
async fn rename_owned_noreplace(
    root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), PodcastWorkerError> {
    if source.parent() != Some(root) || destination.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    tokio::fs::rename(source, destination).await?;
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
async fn rename_owned_noreplace(
    _root: &Path,
    _source: &Path,
    _destination: &Path,
) -> Result<(), PodcastWorkerError> {
    Err(PodcastWorkerError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported",
    )))
}

async fn record_failure(
    pool: &PgPool,
    config: &PodcastConfig,
    worker: Uuid,
    claim: &DownloadClaim,
    error: &PodcastWorkerError,
) -> Result<(), PodcastWorkerError> {
    let next = claim.download_attempt_count.saturating_add(1);
    let permanent = next >= config.max_attempts;
    let backoff = i64::from(2_i32.saturating_pow(u32::try_from(next.min(10)).unwrap_or(10)));
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status=CASE WHEN $4 THEN 'Failed' ELSE 'Queued' END,\
         download_attempt_count=$3,download_error=$5,download_available_at=now()+make_interval(secs=>$6::double precision),\
         download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         WHERE id=$1 AND download_lease_owner=$2",
    ).bind(claim.id).bind(worker).bind(next).bind(permanent).bind(error.to_string())
        .bind(backoff as f64).execute(pool).await?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
async fn remove_owned_file(root: &Path, path: &Path) -> Result<(), PodcastWorkerError> {
    use rustix::fs::{AtFlags, FileType, statat, unlinkat};

    if path.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    let name = path
        .file_name()
        .ok_or(PodcastWorkerError::InvalidResponse)?;
    let directory = open_podcast_directory(root, false).map_err(PodcastWorkerError::Io)?;
    let stat = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(()),
        Err(error) => return Err(PodcastWorkerError::Io(std::io::Error::from(error))),
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    unlinkat(&directory, name, AtFlags::empty()).map_err(std::io::Error::from)?;
    Ok(())
}

#[cfg(windows)]
async fn remove_owned_file(root: &Path, path: &Path) -> Result<(), PodcastWorkerError> {
    if path.parent() != Some(root) {
        return Err(PodcastWorkerError::InvalidResponse);
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_file(path).await?;
        }
        Ok(_) => return Err(PodcastWorkerError::InvalidResponse),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
async fn remove_owned_file(_root: &Path, _path: &Path) -> Result<(), PodcastWorkerError> {
    Err(PodcastWorkerError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "race-resistant owned-file cleanup is unsupported",
    )))
}

pub async fn run_podcast_once(
    pool: &PgPool,
    config: &PodcastConfig,
) -> Result<usize, PodcastWorkerError> {
    if !config.enabled || !config.worker_enabled {
        return Ok(0);
    }
    // Durable pool gate (ADR-0134 §8): a paused pool does no work this pass.
    // One pass is one claim batch; already-claimed downloads from earlier
    // passes keep their leases and finish or expire per the lease contract.
    if melodee_persistence::is_pool_paused(pool, melodee_persistence::POOL_PODCAST).await? {
        info!("podcast worker pool is durably paused; skipping pass");
        return Ok(0);
    }
    refresh_due_channels(pool, config).await?;
    // Legacy `PodcastDownloadJob`: one pass processes at most
    // `podcast.download.maxConcurrent.global` episodes. The Rust worker also
    // processes claims sequentially, and the per-user cap plus the per-user
    // byte quota are enforced inside the claim query so one user cannot
    // starve others across worker processes.
    let policy = resolve_podcast_policy(pool, config).await?;
    let batch = config.claim_batch_size.clamp(
        1,
        i64::try_from(policy.download_concurrency)
            .unwrap_or(1)
            .max(1),
    );
    debug!(
        download_concurrency = policy.download_concurrency,
        per_user_concurrency = policy.per_user_concurrency,
        retention_days = policy.retention_days,
        retention_keep_last_n = policy.retention_keep_last_n,
        retention_keep_unplayed_only = policy.retention_keep_unplayed_only,
        "podcast pass policy resolved"
    );
    let worker = Uuid::now_v7();
    let mut completed = 0;
    for _ in 0..batch {
        // Re-check between claims so a pause mid-batch stops new claims
        // immediately without touching in-flight leases.
        if melodee_persistence::is_pool_paused(pool, melodee_persistence::POOL_PODCAST).await? {
            info!("podcast worker pool paused mid-batch; stopping new claims");
            break;
        }
        let Some(claim) = claim_download(pool, config, worker, &policy).await? else {
            break;
        };
        match download_one(
            pool,
            config,
            &policy,
            worker,
            DownloadClaim {
                id: claim.id,
                public_api_id: claim.public_api_id,
                enclosure_url: claim.enclosure_url.clone(),
                download_attempt_count: claim.download_attempt_count,
            },
        )
        .await
        {
            Ok(()) => completed += 1,
            Err(error) => {
                warn!(episode_id=claim.id, %error, "podcast enclosure download failed");
                record_failure(pool, config, worker, &claim, &error).await?;
            }
        }
    }
    let orphans = reconcile_owned_files(pool, config).await?;
    if orphans > 0 {
        info!(
            orphans,
            "podcast orphan reconciliation removed unreferenced files"
        );
    }
    cleanup_retention(pool, config, &policy).await?;
    Ok(completed)
}

/// One failed or stuck enclosure download, joined with its channel for
/// operator reporting. "Stuck" means a `Downloading` row whose lease has
/// expired — the worker that claimed it is gone or wedged. This is exactly
/// the set [`claim_download`] is allowed to reclaim, so recovery tooling can
/// act on it without bypassing lease safety.
#[derive(Debug, FromRow)]
pub struct StuckPodcastDownload {
    pub episode_id: i64,
    pub public_api_id: Uuid,
    pub channel_id: i64,
    pub channel_title: String,
    pub episode_title: String,
    pub download_status: String,
    pub download_attempt_count: i32,
    pub download_error: Option<String>,
    pub download_lease_owner: Option<Uuid>,
    pub download_lease_expires_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

// "Stuck" predicate shared by the list/recover queries below: a `Downloading`
// row whose lease has expired. This is exactly the set `claim_download` is
// allowed to reclaim, so recovery tooling acting on it never steals a live
// worker's claim. Kept as adjacent literal SQL (sqlx requires static SQL).
/// List failed and stuck (expired-lease) enclosure downloads, oldest update
/// first, bounded by `limit`. Read-only; safe against a running worker.
pub async fn list_stuck_downloads(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<StuckPodcastDownload>, PodcastWorkerError> {
    Ok(sqlx::query_as::<_, StuckPodcastDownload>(
        "SELECT episode.id AS episode_id,episode.public_api_id,episode.channel_id,\
         channel.title AS channel_title,episode.title AS episode_title,episode.download_status,\
         episode.download_attempt_count,episode.download_error,episode.download_lease_owner,\
         episode.download_lease_expires_at,episode.updated_at \
         FROM native_podcast_episodes episode \
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id \
         WHERE episode.soft_deleted_at IS NULL AND channel.soft_deleted_at IS NULL \
         AND ((episode.download_status='Downloading' AND episode.download_lease_expires_at<=now()) \
           OR episode.download_status='Failed') \
         ORDER BY episode.updated_at,episode.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Requeue stuck downloads back to `Queued`, bounded by `limit`.
///
/// Lease safety: only rows whose lease has *expired* are touched — the same
/// predicate [`claim_download`] uses to reclaim — so a live worker's claim is
/// never stolen. Publication remains fenced by the lease re-check in
/// [`publish_download`]. With `retry_failed`, permanently `Failed` rows are
/// also requeued with their attempt counter and error reset, giving them a
/// fresh bounded retry budget. Rows skipped by `SKIP LOCKED` are left for a
/// later pass rather than blocking on a concurrent worker.
pub async fn recover_stuck_downloads(
    pool: &PgPool,
    limit: i64,
    retry_failed: bool,
) -> Result<u64, PodcastWorkerError> {
    let changed = sqlx::query(
        "WITH stuck AS (SELECT episode.id,episode.download_status FROM native_podcast_episodes episode \
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id \
         WHERE episode.soft_deleted_at IS NULL AND channel.soft_deleted_at IS NULL \
         AND ((episode.download_status='Downloading' AND episode.download_lease_expires_at<=now()) \
           OR ($2 AND episode.download_status='Failed')) \
         ORDER BY episode.updated_at,episode.id FOR UPDATE OF episode SKIP LOCKED LIMIT $1) \
         UPDATE native_podcast_episodes e SET download_status='Queued',\
         download_attempt_count=CASE WHEN stuck.download_status='Failed' THEN 0 ELSE e.download_attempt_count END,\
         download_error=NULL,download_available_at=now(),\
         download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         FROM stuck WHERE e.id=stuck.id",
    )
    .bind(limit)
    .bind(retry_failed)
    .execute(pool)
    .await?;
    Ok(changed.rows_affected())
}

async fn refresh_due_channels(
    pool: &PgPool,
    config: &PodcastConfig,
) -> Result<(), PodcastWorkerError> {
    let rows = sqlx::query_as::<_, (i64, Uuid)>(
        "SELECT id,owner_user_id FROM native_podcast_channels WHERE soft_deleted_at IS NULL \
         AND refresh_interval_hours IS NOT NULL \
         AND (last_sync_at IS NULL OR last_sync_at<=now()-make_interval(hours=>refresh_interval_hours)) \
         ORDER BY COALESCE(last_sync_at,'epoch'::timestamptz),id LIMIT $1",
    )
    .bind(config.claim_batch_size)
    .fetch_all(pool)
    .await?;
    for (channel_id, owner) in rows {
        if let Err(error) =
            refresh_channel(pool, owner, channel_id, config.allow_loopback_http).await
        {
            warn!(channel_id, %error, "podcast feed refresh failed");
        }
    }
    Ok(())
}

/// Remove files under the owned podcast storage root that no live database
/// row references (orphaned temporary and media files), in bounded batches.
/// Returns the number of files removed. Also used by the recovery CLI: the
/// same durable reference checks protect in-flight downloads (a `.tmp` file
/// whose episode row is `Downloading` under a live lease owned by the temp
/// file's worker is never removed).
pub async fn reconcile_owned_files(
    pool: &PgPool,
    config: &PodcastConfig,
) -> Result<u64, PodcastWorkerError> {
    let Some(root) = config.storage_root.as_deref() else {
        return Ok(0);
    };
    let mut removed = 0_u64;
    let mut entries = tokio::fs::read_dir(root).await?;
    let mut candidates = Vec::with_capacity(MAX_RECONCILE_FILES_PER_BATCH);
    let mut inspected_in_batch = 0_usize;
    while let Some(entry) = entries.next_entry().await? {
        inspected_in_batch += 1;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            if inspected_in_batch >= MAX_RECONCILE_ENTRIES_PER_BATCH {
                removed += reconcile_owned_file_batch(pool, root, &mut candidates).await?;
                inspected_in_batch = 0;
            }
            continue;
        };
        if name == ROOT_MARKER {
            // The marker still counts toward the directory-work bound.
        } else if let Some(kind) = owned_file_kind(name) {
            candidates.push((path, kind));
        }
        if candidates.len() >= MAX_RECONCILE_FILES_PER_BATCH
            || inspected_in_batch >= MAX_RECONCILE_ENTRIES_PER_BATCH
        {
            removed += reconcile_owned_file_batch(pool, root, &mut candidates).await?;
            inspected_in_batch = 0;
        }
    }
    removed += reconcile_owned_file_batch(pool, root, &mut candidates).await?;
    Ok(removed)
}

async fn reconcile_owned_file_batch(
    pool: &PgPool,
    root: &Path,
    candidates: &mut Vec<(PathBuf, OwnedFileKind)>,
) -> Result<u64, PodcastWorkerError> {
    if candidates.is_empty() {
        return Ok(0);
    }
    let ids = candidates
        .iter()
        .map(|(_, kind)| kind.public_id())
        .collect::<Vec<_>>();
    let facts = sqlx::query_as::<_, (Uuid, Option<String>, Option<Uuid>, bool)>(
        "SELECT episode.public_api_id,episode.local_path,episode.download_lease_owner,
                episode.download_status='Downloading' AND episode.download_lease_expires_at>now()
         FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.public_api_id=ANY($1) AND episode.soft_deleted_at IS NULL
           AND channel.soft_deleted_at IS NULL",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|fact| (fact.0, fact))
    .collect::<HashMap<_, _>>();
    let mut removed = 0_u64;
    for (path, kind) in candidates.drain(..) {
        let referenced = facts.get(&kind.public_id()).is_some_and(|fact| match kind {
            OwnedFileKind::Media(_) => fact.1.as_deref() == path.to_str(),
            OwnedFileKind::Temporary { worker, .. } => fact.3 && fact.2 == Some(worker),
        });
        if !referenced {
            remove_owned_file(root, &path).await?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[derive(Clone, Copy, Debug)]
enum OwnedFileKind {
    Media(Uuid),
    Temporary { public_id: Uuid, worker: Uuid },
}

impl OwnedFileKind {
    const fn public_id(self) -> Uuid {
        match self {
            Self::Media(public_id) | Self::Temporary { public_id, .. } => public_id,
        }
    }
}

fn owned_file_kind(name: &str) -> Option<OwnedFileKind> {
    if let Some(public_id) = name
        .strip_suffix(".media")
        .and_then(|value| value.parse::<Uuid>().ok())
    {
        return Some(OwnedFileKind::Media(public_id));
    }
    let stem = name.strip_suffix(".tmp")?;
    let (public_id, worker) = stem.split_once('.')?;
    Some(OwnedFileKind::Temporary {
        public_id: public_id.parse().ok()?,
        worker: worker.parse().ok()?,
    })
}

/// Applies the legacy `PodcastCleanupJob` policies plus the per-channel
/// bounds: keep-for-N-days, keep-last-N-per-channel, keep-unplayed-only, and
/// the channel storage/count limits. Bookmarked episodes are always retained
/// (a deliberate safety improvement: deleting a file a user bookmarked would
/// silently break resume state), and every deletion re-validates ownership and
/// the recorded path inside a row-locked transaction.
async fn cleanup_retention(
    pool: &PgPool,
    config: &PodcastConfig,
    policy: &PodcastPolicy,
) -> Result<(), PodcastWorkerError> {
    let Some(root) = config.storage_root.as_deref() else {
        return Ok(());
    };
    let rows = sqlx::query_as::<_, (i64, Uuid, String)>(
        "WITH ranked AS (SELECT e.id,e.public_api_id,e.local_path,e.downloaded_at,c.max_downloaded_episodes,\
         c.max_storage_bytes,row_number() OVER(PARTITION BY c.id ORDER BY e.published_at DESC NULLS LAST,e.id DESC) AS ordinal,\
         sum(COALESCE(e.downloaded_bytes,0)) OVER(PARTITION BY c.id ORDER BY e.published_at DESC NULLS LAST,e.id DESC) AS cumulative \
         FROM native_podcast_episodes e JOIN native_podcast_channels c ON c.id=e.channel_id \
         WHERE e.download_status='Downloaded' AND e.local_path IS NOT NULL
         AND e.soft_deleted_at IS NULL AND c.soft_deleted_at IS NULL \
         AND NOT EXISTS(SELECT 1 FROM native_podcast_bookmarks b WHERE b.episode_id=e.id)) \
         SELECT id,public_api_id,local_path FROM ranked WHERE \
         (max_downloaded_episodes IS NOT NULL AND ordinal>max_downloaded_episodes) OR \
         (max_storage_bytes IS NOT NULL AND cumulative>max_storage_bytes) OR \
         ($1::bigint > 0 AND ordinal > $1::bigint) OR \
         ($2::bigint > 0 AND downloaded_at IS NOT NULL
             AND downloaded_at < now() - make_interval(days => $2::integer)) OR \
         ($3::boolean AND EXISTS(SELECT 1 FROM native_podcast_play_history h
             WHERE h.episode_id=ranked.id AND NOT h.is_now_playing)) \
         ORDER BY id LIMIT 100",
    )
    .bind(policy.retention_keep_last_n)
    .bind(policy.retention_days)
    .bind(policy.retention_keep_unplayed_only)
    .fetch_all(pool)
    .await?;
    if !rows.is_empty() {
        info!(
            candidates = rows.len(),
            retention_days = policy.retention_days,
            retention_keep_last_n = policy.retention_keep_last_n,
            retention_keep_unplayed_only = policy.retention_keep_unplayed_only,
            "podcast retention cleanup candidates selected"
        );
    }
    for (id, public_id, stored_path) in rows {
        let path = PathBuf::from(stored_path);
        if path != root.join(format!("{public_id}.media")) {
            return Err(PodcastWorkerError::InvalidResponse);
        }
        let mut tx = pool.begin().await?;
        let retained_id = sqlx::query_scalar::<_, i64>(
            "SELECT episode.id FROM native_podcast_episodes episode
             WHERE episode.id=$1 AND episode.local_path=$2
               AND episode.download_status='Downloaded'
               AND NOT EXISTS(
                   SELECT 1 FROM native_podcast_bookmarks bookmark
                   WHERE bookmark.episode_id=episode.id
               )
             FOR UPDATE",
        )
        .bind(id)
        .bind(path.to_string_lossy().as_ref())
        .fetch_optional(&mut *tx)
        .await?;
        if retained_id.is_none() {
            tx.rollback().await?;
            continue;
        }
        sqlx::query(
            "UPDATE native_podcast_episodes SET download_status='NotDownloaded',local_path=NULL,\
             downloaded_bytes=NULL,downloaded_at=NULL,updated_at=now() WHERE id=$1 AND local_path=$2",
        )
        .bind(id)
        .bind(path.to_string_lossy().as_ref())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        remove_owned_file(root, &path).await?;
        debug!(
            episode_id = id,
            "podcast retention removed downloaded episode"
        );
    }
    Ok(())
}

pub struct PodcastSchedulerHandle {
    stop: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl PodcastSchedulerHandle {
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        if let Err(error) = self.join.await {
            error!(%error, "podcast scheduler task failed during shutdown");
        }
    }
}

pub fn start_podcast_loop(pool: PgPool, config: PodcastConfig) -> Option<PodcastSchedulerHandle> {
    if !config.enabled || !config.worker_enabled {
        return None;
    }
    let (stop, mut task_stop) = watch::channel(false);
    let join = tokio::spawn(async move {
        info!(
            batch_size = config.claim_batch_size,
            "starting podcast download worker"
        );
        let mut interval = tokio::time::interval(config.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let started = time::OffsetDateTime::now_utc();
                    let tick = melodee_persistence::scheduled_tick(started, config.poll_interval);
                    let history = melodee_persistence::JobRunHistoryStore;
                    let run = match history.start_tick(&pool, "podcast", tick).await {
                        Ok(run) => run,
                        Err(error) => {
                            error!(%error, "podcast tick history could not be recorded");
                            None
                        }
                    };
                    let Some(run) = run else {
                        continue;
                    };
                    let outcome = tokio::select! {
                        result = run_podcast_once(&pool, &config) => Some(result),
                        _ = task_stop.changed() => None,
                    };
                    let Some(outcome) = outcome else {
                        let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), false, Some("scheduler stopped during run")).await;
                        break;
                    };
                    match outcome {
                        Ok(_) => {
                            let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), true, None).await;
                        }
                        Err(error) => {
                            let message = error.to_string();
                            let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), false, Some(&message)).await;
                            error!(%error, "podcast worker pass failed");
                        }
                    }
                }
                _ = task_stop.changed() => break,
            }
        }
    });
    Some(PodcastSchedulerHandle { stop, join })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn feed_limits_fall_back_and_clamp() {
        // Unset values fall back to the legacy defaults (10 MiB / 500 / 10).
        let defaults = feed_limits_from_values(None, None, None, None);
        assert_eq!(defaults.max_feed_bytes, 10 * 1024 * 1024);
        assert_eq!(defaults.max_items_per_channel, 500);
        assert_eq!(defaults.max_redirects, 10);
        // A configured larger value (the operator's deployment, e.g. the
        // 2.1 MB Lex Fridman feed rejected by the old hardcode) is honored.
        let configured =
            feed_limits_from_values(Some(10 * 1024 * 1024), Some(500), Some(8), Some(45));
        assert_eq!(configured.max_feed_bytes, 10 * 1024 * 1024);
        assert_eq!(configured.max_redirects, 8);
        assert_eq!(configured.timeout_seconds, 45);
        // Zero/negative values are treated as unset (fall back), not as
        // "unbounded" or "zero".
        let zeroed = feed_limits_from_values(Some(0), Some(-5), Some(0), Some(0));
        assert_eq!(zeroed.max_feed_bytes, DEFAULT_MAX_FEED_BYTES);
        assert_eq!(zeroed.max_items_per_channel, DEFAULT_MAX_FEED_ITEMS);
        assert_eq!(zeroed.max_redirects, DEFAULT_MAX_REDIRECTS);
        assert_eq!(zeroed.timeout_seconds, DEFAULT_HTTP_TIMEOUT_SECONDS);
        // Misconfiguration is clamped to the hard caps, never unbounded.
        let huge = feed_limits_from_values(
            Some(i64::MAX),
            Some(i64::MAX),
            Some(i64::MAX),
            Some(i64::MAX),
        );
        assert_eq!(huge.max_feed_bytes, MAX_FEED_BYTES_HARD_CAP);
        assert_eq!(huge.max_items_per_channel, MAX_FEED_ITEMS_HARD_CAP);
        assert_eq!(huge.max_redirects, MAX_REDIRECTS_HARD_CAP);
        assert_eq!(huge.timeout_seconds, MAX_HTTP_TIMEOUT_SECONDS);
    }

    #[test]
    fn podcast_policy_clamps_and_keeps_the_process_ceiling() {
        // Unset values fall back to the legacy defaults; the process cap wins
        // when it is smaller (a configured setting can only tighten a bound).
        let defaults =
            PodcastPolicy::from_inputs(512 * 1024 * 1024, PodcastPolicyInputs::default());
        assert_eq!(defaults.max_enclosure_bytes, 512 * 1024 * 1024);
        assert_eq!(defaults.download_concurrency, 2);
        assert_eq!(defaults.per_user_concurrency, 1);
        assert_eq!(defaults.retention_days, 0);
        assert_eq!(defaults.quota_bytes_per_user, DEFAULT_QUOTA_BYTES_PER_USER);
        assert_eq!(defaults.retention_keep_last_n, 0);
        assert!(!defaults.retention_keep_unplayed_only);

        // A configured lower bound is honored; a larger one cannot exceed the
        // process cap.
        let tightened = PodcastPolicy::from_inputs(
            512 * 1024 * 1024,
            PodcastPolicyInputs {
                max_enclosure_bytes: Some(64 * 1024 * 1024),
                global_concurrency: Some(1),
                per_user_concurrency: Some(1),
                quota_bytes_per_user: Some(1024),
                retention_days: Some(30),
                keep_last_n: Some(1),
                keep_unplayed_only: Some(true),
            },
        );
        assert_eq!(tightened.max_enclosure_bytes, 64 * 1024 * 1024);
        assert_eq!(tightened.download_concurrency, 1);
        assert_eq!(tightened.retention_days, 30);
        assert_eq!(tightened.quota_bytes_per_user, 1024);
        assert_eq!(tightened.retention_keep_last_n, 1);
        assert!(tightened.retention_keep_unplayed_only);
        let capped = PodcastPolicy::from_inputs(
            512 * 1024 * 1024,
            PodcastPolicyInputs {
                max_enclosure_bytes: Some(i64::MAX),
                global_concurrency: Some(1000),
                per_user_concurrency: Some(1000),
                quota_bytes_per_user: Some(-1),
                retention_days: Some(-1),
                keep_last_n: Some(-1),
                keep_unplayed_only: None,
            },
        );
        assert_eq!(capped.max_enclosure_bytes, 512 * 1024 * 1024);
        assert_eq!(capped.download_concurrency, 32);
        assert_eq!(capped.per_user_concurrency, 32);
        assert_eq!(capped.retention_days, 0);
        assert_eq!(capped.retention_keep_last_n, 0);
        // 0 disables the per-user bound (legacy "unlimited") but never the
        // process-wide download cap.
        let unlimited_per_user = PodcastPolicy::from_inputs(
            1024,
            PodcastPolicyInputs {
                max_enclosure_bytes: Some(1024),
                global_concurrency: Some(0),
                per_user_concurrency: Some(0),
                quota_bytes_per_user: Some(0),
                retention_days: Some(0),
                keep_last_n: Some(0),
                keep_unplayed_only: Some(false),
            },
        );
        assert_eq!(unlimited_per_user.per_user_concurrency, 0);
        assert_eq!(unlimited_per_user.download_concurrency, 2);
    }

    #[test]
    fn parser_truncates_at_the_item_limit_like_legacy_take() {
        // Legacy `feed.Items.Take(maxItemsPerChannel)`: an over-limit feed is
        // truncated to the limit, never a failed refresh (a real corpus feed
        // with 502 items must still refresh with the default bound of 500).
        let item = "<item><guid>g</guid><title>T</title><enclosure url=\"https://example.test/x.mp3\"/></item>";
        let feed_xml = format!(
            "<rss><channel><title>Big</title>{}</channel></rss>",
            item.repeat(502)
        );
        let feed = parse_feed_with_limits(feed_xml.as_bytes(), 500).expect("truncated feed");
        assert_eq!(feed.episodes.len(), 500);
        // Every truncated item still carries its enclosure identity.
        assert!(
            feed.episodes
                .iter()
                .all(|episode| episode.enclosure_url.is_some())
        );
    }

    #[test]
    fn rss_parser_is_bounded_and_preserves_enclosure_identity() {
        let feed = parse_feed_with_limits(
            br#"<?xml version="1.0"?><rss><channel><title><![CDATA[Example & Audio]]></title>
          <item><guid>episode-1</guid><title><![CDATA[First <Episode>]]></title><description>Details</description>
          <pubDate>Wed, 12 Aug 2026 12:00:00 +0000</pubDate>
          <itunes:duration>01:02:03</itunes:duration>
          <enclosure url="https://example.test/one.mp3"/></item></channel></rss>"#,
            DEFAULT_MAX_FEED_ITEMS,
        )
        .expect("feed");
        assert_eq!(feed.title.as_deref(), Some("Example & Audio"));
        assert_eq!(feed.episodes.len(), 1);
        assert_eq!(feed.episodes[0].feed_item_id, "episode-1");
        assert_eq!(feed.episodes[0].title, "First <Episode>");
        assert_eq!(feed.episodes[0].duration_ms, Some(3_723_000));
        assert!(!is_public_address("127.0.0.1".parse().expect("loopback")));
        assert!(!is_public_address(
            "169.254.169.254".parse().expect("link local")
        ));
        assert!(!is_public_address("10.0.0.1".parse().expect("private")));
        for non_public in [
            "100.64.0.1",
            "192.0.0.9",
            "192.0.2.1",
            "192.88.99.1",
            "198.51.100.1",
            "203.0.113.1",
            "100::1",
            "2001:2::1",
            "2001:3::1",
            "2001:20::1",
            "2001:100::1",
            "2001:db8::1",
            "3fff::1",
            "64:ff9b::a00:1",
            "64:ff9b:1::a00:1",
            "2002:a00:1::",
        ] {
            assert!(
                !is_public_address(non_public.parse().expect("non-public fixture")),
                "{non_public} must never cross the podcast outbound boundary"
            );
        }
        assert!(is_public_address("8.8.8.8".parse().expect("public")));
        assert!(is_public_address(
            "2606:4700:4700::1111".parse().expect("public IPv6")
        ));
    }

    #[tokio::test]
    async fn owned_publication_rejects_symlink_and_existing_destination() {
        let directory = tempfile::tempdir().expect("temp");
        let root = directory.path();
        let temporary = root.join("source.tmp");
        let destination = root.join("destination.media");
        std::fs::write(&temporary, b"new").expect("temporary");
        std::fs::write(&destination, b"old").expect("destination");
        let error = rename_owned_noreplace(root, &temporary, &destination)
            .await
            .expect_err("cannot replace destination");
        assert!(
            matches!(error, PodcastWorkerError::Io(ref io) if io.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(&destination).expect("unchanged"), b"old");

        #[cfg(unix)]
        {
            let external = directory.path().join("external");
            std::fs::write(&external, b"external").expect("external");
            let link = root.join("link.tmp");
            std::os::unix::fs::symlink(&external, &link).expect("symlink");
            assert!(create_owned_temp(root, &link).await.is_err());
            assert_eq!(
                std::fs::read(external).expect("external unchanged"),
                b"external"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[tokio::test]
    async fn owned_publication_rejects_an_ancestor_symlink() {
        let directory = tempfile::tempdir().expect("temp");
        let real_parent = directory.path().join("real-parent");
        std::fs::create_dir(&real_parent).expect("real parent");
        let real_root = real_parent.join("podcasts");
        std::fs::create_dir(&real_root).expect("podcast root");
        std::fs::write(real_root.join(ROOT_MARKER), b"melodee-podcast-root-v1\n")
            .expect("root marker");
        let linked_parent = directory.path().join("linked-parent");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_parent, &linked_parent).expect("linked parent");
        let linked_root = linked_parent.join("podcasts");
        let temporary = linked_root.join("download.tmp");

        assert!(create_owned_temp(&linked_root, &temporary).await.is_err());
        assert!(!real_root.join("download.tmp").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_cleanup_rejects_symlink_without_touching_the_target() {
        let directory = tempfile::tempdir().expect("temp");
        let root = directory.path();
        let external = root.join("external.media");
        let link = root.join("owned.media");
        std::fs::write(&external, b"authoritative").expect("external");
        std::os::unix::fs::symlink(&external, &link).expect("symlink");

        assert!(remove_owned_file(root, &link).await.is_err());
        assert!(link.is_symlink());
        assert_eq!(
            std::fs::read(&external).expect("unchanged"),
            b"authoritative"
        );
    }

    #[test]
    fn podcast_storage_roots_must_be_owned_and_disjoint() {
        let directory = tempfile::tempdir().expect("temp");
        let root = directory.path().join("podcasts");
        let other = directory.path().join("catalog");
        std::fs::create_dir(&other).expect("other");
        let overlapping = other.join("podcasts");
        let overlapping_config = PodcastConfig {
            storage_root: Some(overlapping.clone()),
            ..PodcastConfig::default()
        };
        assert!(
            overlapping_config
                .prepare_owned_root(std::slice::from_ref(&other))
                .is_err()
        );
        assert!(
            !overlapping.exists(),
            "overlap is rejected before a directory or ownership marker is written"
        );
        validate_owned_root(&root).expect("owned root");
        let config = PodcastConfig {
            storage_root: Some(root.clone()),
            ..PodcastConfig::default()
        };
        config
            .reject_root_overlap(std::slice::from_ref(&other))
            .expect("disjoint");
        assert!(
            config
                .reject_root_overlap(std::slice::from_ref(&root))
                .is_err()
        );

        std::fs::write(root.join(ROOT_MARKER), b"not-a-melodee-marker\n")
            .expect("replace marker contents");
        assert!(
            validate_owned_root(&root).is_err(),
            "a marker filename without the ownership payload is not authority"
        );

        #[cfg(unix)]
        {
            let actual_parent = directory.path().join("actual-parent");
            let linked_parent = directory.path().join("linked-parent");
            std::fs::create_dir(&actual_parent).expect("actual parent");
            std::os::unix::fs::symlink(&actual_parent, &linked_parent).expect("linked parent");
            let linked_root = linked_parent.join("podcasts");
            assert!(
                validate_owned_root(&linked_root).is_err(),
                "an ancestor symlink is rejected before creating an owned root"
            );
            assert!(!actual_parent.join("podcasts").exists());
        }
    }
}
