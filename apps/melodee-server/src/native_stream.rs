//! Native-v1 direct-play primitives governed by ADR-0125.
//!
//! Request paths never select filesystem locations. Callers first resolve a
//! Track public ID to its catalog-authoritative playable `MediaAsset`, then
//! this module opens that relative path beneath a pre-opened canonical root.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    net::{IpAddr, SocketAddr},
    path::{Component, Path},
    str::FromStr,
    sync::Arc,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose};
use futures_util::stream;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::resource_bounds::ResourceGauge;
use uuid::Uuid;

/// Upper bound for one filesystem read before yielding back to the HTTP body.
const STREAM_CHUNK_SIZE: usize = 64 * 1024;
const STREAM_TOKEN_LIFETIME_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_ENCODED_STREAM_TOKEN_BYTES: usize = 4096;

type HmacSha256 = Hmac<Sha256>;

/// Native-v1 HMAC URL codec. The user-owned signing key is supplied to each
/// operation and is never retained or exposed through `Debug` output.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamTokenCodec;

impl StreamTokenCodec {
    /// Issues the exact captured seven-day, client-bound native-v1 token.
    pub fn issue(
        self,
        signing_key: &str,
        user_public_id: Uuid,
        track_public_id: Uuid,
        client_binding: &str,
        now_unix_seconds: i64,
    ) -> Result<String, StreamTokenError> {
        if signing_key.is_empty() || client_binding.is_empty() {
            return Err(StreamTokenError);
        }
        let expires_at = now_unix_seconds
            .checked_add(STREAM_TOKEN_LIFETIME_SECONDS)
            .ok_or(StreamTokenError)?;
        let data = format!("{user_public_id}:{track_public_id}:{client_binding}");
        let signed = format!("{data}|{expires_at}");
        let signature = sign(signing_key, signed.as_bytes())?;
        let token = format!("{signed}|{}", general_purpose::STANDARD.encode(signature));
        Ok(general_purpose::URL_SAFE_NO_PAD.encode(token))
    }

    /// Validates shape, expiry, URL claims, client binding, and HMAC without
    /// revealing which credential fact failed. Successfully replaying the
    /// same token before expiry is intentional for HTTP range/seek requests.
    pub fn validate(
        self,
        signing_key: &str,
        encoded_token: &str,
        expected_user_public_id: Uuid,
        expected_track_public_id: Uuid,
        expected_client_binding: &str,
        now_unix_seconds: i64,
    ) -> Result<(), StreamTokenError> {
        if signing_key.is_empty()
            || expected_client_binding.is_empty()
            || encoded_token.is_empty()
            || encoded_token.len() > MAX_ENCODED_STREAM_TOKEN_BYTES
        {
            return Err(StreamTokenError);
        }
        let decoded = general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_token)
            .or_else(|_| general_purpose::URL_SAFE.decode(encoded_token))
            .map_err(|_| StreamTokenError)?;
        let token = std::str::from_utf8(&decoded).map_err(|_| StreamTokenError)?;
        let mut parts = token.split('|');
        let data = parts.next().ok_or(StreamTokenError)?;
        let timestamp = parts.next().ok_or(StreamTokenError)?;
        let signature = parts.next().ok_or(StreamTokenError)?;
        if parts.next().is_some() {
            return Err(StreamTokenError);
        }
        let expires_at = timestamp.parse::<i64>().map_err(|_| StreamTokenError)?;
        if now_unix_seconds > expires_at {
            return Err(StreamTokenError);
        }
        let mut data_parts = data.splitn(3, ':');
        let user_public_id = data_parts
            .next()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(StreamTokenError)?;
        let track_public_id = data_parts
            .next()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(StreamTokenError)?;
        let client_binding = data_parts.next().ok_or(StreamTokenError)?;
        if user_public_id != expected_user_public_id
            || track_public_id != expected_track_public_id
            || !client_binding.eq_ignore_ascii_case(expected_client_binding)
        {
            return Err(StreamTokenError);
        }
        let signature = general_purpose::STANDARD
            .decode(signature)
            .map_err(|_| StreamTokenError)?;
        let signed = format!("{data}|{timestamp}");
        let mut verifier =
            HmacSha256::new_from_slice(signing_key.as_bytes()).map_err(|_| StreamTokenError)?;
        verifier.update(signed.as_bytes());
        verifier
            .verify_slice(&signature)
            .map_err(|_| StreamTokenError)
    }
}

fn sign(signing_key: &str, data: &[u8]) -> Result<Vec<u8>, StreamTokenError> {
    let mut mac =
        HmacSha256::new_from_slice(signing_key.as_bytes()).map_err(|_| StreamTokenError)?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

/// Uniform credential rejection. It intentionally carries no failure detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid stream credential")]
pub struct StreamTokenError;

/// Explicit exact-address/prefix trusted proxy entry. The prefix is bounded
/// by the IP family and contains no hostname or implicit trust expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxy {
    network: IpAddr,
    prefix: u8,
}

impl FromStr for TrustedProxy {
    type Err = ClientAddressError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (
                address.parse::<IpAddr>().map_err(|_| ClientAddressError)?,
                prefix.parse::<u8>().map_err(|_| ClientAddressError)?,
            ),
            None => {
                let address = value.parse::<IpAddr>().map_err(|_| ClientAddressError)?;
                let prefix = if address.is_ipv4() { 32 } else { 128 };
                (address, prefix)
            }
        };
        let bits = if address.is_ipv4() { 32 } else { 128 };
        if prefix > bits || !host_bits_are_zero(address, prefix) {
            return Err(ClientAddressError);
        }
        Ok(Self {
            network: address,
            prefix,
        })
    }
}

impl TrustedProxy {
    #[must_use]
    pub fn contains(self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                prefix_matches(u32::from(network), u32::from(address), self.prefix, 32)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                prefix_matches(u128::from(network), u128::from(address), self.prefix, 128)
            }
            _ => false,
        }
    }
}

fn host_bits_are_zero(address: IpAddr, prefix: u8) -> bool {
    match address {
        IpAddr::V4(address) => {
            let host_bits = u32::from(32 - prefix);
            host_bits == 32 || u32::from(address) & ((1_u32 << host_bits) - 1) == 0
        }
        IpAddr::V6(address) => {
            let host_bits = u32::from(128 - prefix);
            host_bits == 128 || u128::from(address) & ((1_u128 << host_bits) - 1) == 0
        }
    }
}

fn prefix_matches<T>(left: T, right: T, prefix: u8, bits: u8) -> bool
where
    T: Copy
        + Eq
        + std::ops::BitAnd<Output = T>
        + std::ops::Not<Output = T>
        + std::ops::Shl<u32, Output = T>
        + From<u8>,
{
    if prefix == 0 {
        return true;
    }
    let host_bits = u32::from(bits - prefix);
    let mask = !T::from(0_u8) << host_bits;
    left & mask == right & mask
}

/// Trusted-proxy-safe client address resolution shared by token issuance and
/// the public stream handler. It never trusts forwarding headers presented by
/// a direct peer outside the configured proxy set.
#[derive(Debug, Clone, Default)]
pub struct ClientAddressResolver {
    trusted_proxies: Arc<[TrustedProxy]>,
}

impl ClientAddressResolver {
    #[must_use]
    pub fn new(trusted_proxies: Vec<TrustedProxy>) -> Self {
        Self {
            trusted_proxies: trusted_proxies.into(),
        }
    }

    /// Walks the forwarded chain from the trusted edge toward the client and
    /// selects the first untrusted hop. `Forwarded` takes precedence over
    /// `X-Forwarded-For`; otherwise the direct peer is authoritative.
    pub fn resolve(
        &self,
        peer: SocketAddr,
        forwarded: Option<&str>,
        x_forwarded_for: Option<&str>,
    ) -> Result<IpAddr, ClientAddressError> {
        if !self
            .trusted_proxies
            .iter()
            .any(|proxy| proxy.contains(peer.ip()))
        {
            return Ok(peer.ip());
        }
        if let Some(value) = forwarded {
            return self.first_untrusted(parse_forwarded(value)?);
        }
        if let Some(value) = x_forwarded_for {
            return self.first_untrusted(parse_x_forwarded_for(value)?);
        }
        Ok(peer.ip())
    }

    /// Whether the direct peer is a configured trusted proxy whose forwarded
    /// headers (`Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`) may be
    /// honored.
    pub fn is_trusted(&self, address: IpAddr) -> bool {
        self.trusted_proxies
            .iter()
            .any(|proxy| proxy.contains(address))
    }

    fn first_untrusted(&self, chain: Vec<IpAddr>) -> Result<IpAddr, ClientAddressError> {
        chain
            .into_iter()
            .rev()
            .find(|address| {
                !self
                    .trusted_proxies
                    .iter()
                    .any(|proxy| proxy.contains(*address))
            })
            .ok_or(ClientAddressError)
    }
}

const MAX_FORWARDED_HOPS: usize = 32;

fn parse_x_forwarded_for(value: &str) -> Result<Vec<IpAddr>, ClientAddressError> {
    let values = value.split(',').collect::<Vec<_>>();
    if values.is_empty() || values.len() > MAX_FORWARDED_HOPS {
        return Err(ClientAddressError);
    }
    values
        .into_iter()
        .map(|value| {
            value
                .trim()
                .parse::<IpAddr>()
                .map_err(|_| ClientAddressError)
        })
        .collect()
}

fn parse_forwarded(value: &str) -> Result<Vec<IpAddr>, ClientAddressError> {
    let values = value.split(',').collect::<Vec<_>>();
    if values.is_empty() || values.len() > MAX_FORWARDED_HOPS {
        return Err(ClientAddressError);
    }
    values.into_iter().map(parse_forwarded_element).collect()
}

fn parse_forwarded_element(value: &str) -> Result<IpAddr, ClientAddressError> {
    for pair in value.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("for") {
            continue;
        }
        let value = value.trim().trim_matches('"');
        if value.starts_with('_') || value.eq_ignore_ascii_case("unknown") {
            return Err(ClientAddressError);
        }
        if let Ok(address) = value.parse::<IpAddr>() {
            return Ok(address);
        }
        if let Ok(address) = value.parse::<SocketAddr>() {
            return Ok(address.ip());
        }
        if let Some(ipv6) = value
            .strip_prefix('[')
            .and_then(|value| value.split(']').next())
        {
            return ipv6.parse::<IpAddr>().map_err(|_| ClientAddressError);
        }
        return Err(ClientAddressError);
    }
    Err(ClientAddressError)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid client address")]
pub struct ClientAddressError;

/// Catalog facts required to serve one direct-play asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayableAsset {
    pub canonical_relative_path: String,
    pub byte_size: u64,
    pub mime_type: String,
}

/// A normalized single HTTP byte range, inclusive at both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    #[must_use]
    pub const fn length(self) -> u64 {
        self.end - self.start + 1
    }
}

/// Errors deliberately collapse unsafe filesystem details before reaching a
/// native-v1 response or log record.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("invalid byte range")]
    InvalidRange,
    #[error("stream capacity is unavailable")]
    CapacityClosed,
    #[error("media asset is unavailable")]
    AssetUnavailable,
    #[error("media asset metadata is invalid")]
    InvalidAssetMetadata,
}

/// Parses the captured native-v1 single-range grammar.
///
/// The legacy contract intentionally maps malformed, multipart, inverted, and
/// unsatisfiable ranges to its typed 400 response rather than RFC 7233's 416.
pub fn parse_byte_range(value: &str, full_length: u64) -> Result<ByteRange, StreamError> {
    if full_length == 0 {
        return Err(StreamError::InvalidRange);
    }
    let value = value.trim();
    let unit = value.get(..6).ok_or(StreamError::InvalidRange)?;
    if !unit.eq_ignore_ascii_case("bytes=") {
        return Err(StreamError::InvalidRange);
    }
    let range = &value[6..];
    if range.is_empty()
        || range.contains(',')
        || range.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(StreamError::InvalidRange);
    }
    let (start, end) = range.split_once('-').ok_or(StreamError::InvalidRange)?;
    match (start.is_empty(), end.is_empty()) {
        (true, true) => Err(StreamError::InvalidRange),
        (true, false) => {
            let suffix = parse_u64(end)?;
            if suffix == 0 {
                return Err(StreamError::InvalidRange);
            }
            let length = suffix.min(full_length);
            Ok(ByteRange {
                start: full_length - length,
                end: full_length - 1,
            })
        }
        (false, true) => {
            let start = parse_u64(start)?;
            if start >= full_length {
                return Err(StreamError::InvalidRange);
            }
            Ok(ByteRange {
                start,
                end: full_length - 1,
            })
        }
        (false, false) => {
            let start = parse_u64(start)?;
            let requested_end = parse_u64(end)?;
            if start > requested_end || start >= full_length {
                return Err(StreamError::InvalidRange);
            }
            Ok(ByteRange {
                start,
                end: requested_end.min(full_length - 1),
            })
        }
    }
}

fn parse_u64(value: &str) -> Result<u64, StreamError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StreamError::InvalidRange);
    }
    value.parse().map_err(|_| StreamError::InvalidRange)
}

/// A root handle used for race-resistant, no-follow asset opens.
#[derive(Debug, Clone)]
pub struct CanonicalAssetRoot {
    root: Arc<File>,
}

impl CanonicalAssetRoot {
    /// Opens an existing absolute directory without following any symlink
    /// component. Unsupported platforms fail closed.
    pub fn open(path: &Path) -> Result<Self, StreamError> {
        platform::open_root(path)
            .map(|root| Self {
                root: Arc::new(root),
            })
            .map_err(|_| StreamError::AssetUnavailable)
    }

    /// Opens the exact catalog-provided relative asset path. The final object
    /// must be a regular, non-symlink file with the catalog-recorded size.
    pub fn open_asset(&self, asset: &PlayableAsset) -> Result<File, StreamError> {
        let path = Path::new(&asset.canonical_relative_path);
        validate_relative_asset_path(path)?;
        let file =
            platform::open_relative(&self.root, path).map_err(|_| StreamError::AssetUnavailable)?;
        let metadata = file.metadata().map_err(|_| StreamError::AssetUnavailable)?;
        if !metadata.is_file() || metadata.len() != asset.byte_size {
            return Err(StreamError::AssetUnavailable);
        }
        Ok(file)
    }

    /// Returns the byte length of a canonical relative file, or `None` when it
    /// does not exist. Used for sibling files (`.lrc` lyrics) whose size is
    /// not catalog-recorded.
    pub fn relative_len(&self, relative_path: &str) -> Result<Option<u64>, StreamError> {
        let path = Path::new(relative_path);
        validate_relative_asset_path(path)?;
        let file = match platform::open_relative(&self.root, path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(StreamError::AssetUnavailable),
        };
        let metadata = file.metadata().map_err(|_| StreamError::AssetUnavailable)?;
        if !metadata.is_file() {
            return Err(StreamError::AssetUnavailable);
        }
        Ok(Some(metadata.len()))
    }

    /// Opens a canonical relative file (no symlinks, no traversal) whose size
    /// was obtained out-of-band, e.g. via [`Self::relative_len`].
    pub fn open_relative_file(&self, relative_path: &str) -> Result<File, StreamError> {
        let path = Path::new(relative_path);
        validate_relative_asset_path(path)?;
        platform::open_relative(&self.root, path).map_err(|_| StreamError::AssetUnavailable)
    }
}

fn validate_relative_asset_path(path: &Path) -> Result<(), StreamError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(StreamError::AssetUnavailable);
    }
    let mut count = 0_usize;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(StreamError::AssetUnavailable);
        }
        count = count.saturating_add(1);
    }
    if count == 0 {
        return Err(StreamError::AssetUnavailable);
    }
    Ok(())
}

/// Bounded direct-play response builder. The semaphore permit stays inside
/// the response stream and is released on EOF or as soon as the client drops
/// the body, so disconnect cancellation does not strand capacity.
#[derive(Debug, Clone)]
pub struct DirectPlayService {
    root: CanonicalAssetRoot,
    permits: Arc<Semaphore>,
    gauge: Option<Arc<crate::resource_bounds::ResourceGauge>>,
}

/// A held direct-play admission: the semaphore permit plus the optional
/// resource gauge guard, released together when the response body (or a
/// cancelled handler) drops.
#[derive(Debug)]
struct Admission {
    _semaphore: OwnedSemaphorePermit,
    _gauge: Option<crate::resource_bounds::GaugePermit>,
}

impl DirectPlayService {
    #[must_use]
    pub fn new(root: CanonicalAssetRoot, max_concurrent_streams: usize) -> Option<Self> {
        (max_concurrent_streams > 0).then(|| Self {
            root,
            permits: Arc::new(Semaphore::new(max_concurrent_streams)),
            gauge: None,
        })
    }

    /// Attaches the process resource-bounds gauge for the direct-play class
    ///: records queued waiters, in-flight utilization, and saturation
    /// on the admin health surface. Admission behavior is unchanged.
    #[must_use]
    pub fn with_gauge(mut self, gauge: Arc<crate::resource_bounds::ResourceGauge>) -> Self {
        self.gauge = Some(gauge);
        self
    }

    /// Blocking acquisition with gauge instrumentation: waiters are visible
    /// while queued and cancellation of the future simply never admits.
    async fn acquire(&self) -> Result<Admission, StreamError> {
        if let Some(gauge) = &self.gauge {
            gauge.begin_wait();
        }
        let permit = match self.permits.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                if let Some(gauge) = &self.gauge {
                    gauge.wait_failed();
                }
                let _ = error;
                return Err(StreamError::CapacityClosed);
            }
        };
        let gauge = self.gauge.as_ref().map(ResourceGauge::admit);
        Ok(Admission {
            _semaphore: permit,
            _gauge: gauge,
        })
    }

    /// Builds a full, ranged, or HEAD response from catalog-owned facts.
    pub async fn response(
        &self,
        asset: &PlayableAsset,
        range_header: Option<&str>,
        head_only: bool,
    ) -> Result<Response<Body>, StreamError> {
        let range = range_header
            .map(|value| parse_byte_range(value, asset.byte_size))
            .transpose()?;
        let selected = range.unwrap_or(ByteRange {
            start: 0,
            end: asset
                .byte_size
                .checked_sub(1)
                .ok_or(StreamError::AssetUnavailable)?,
        });
        let content_type = HeaderValue::from_str(&asset.mime_type)
            .map_err(|_| StreamError::InvalidAssetMetadata)?;
        let permit = self.acquire().await?;
        let file = self.root.open_asset(asset)?;

        let status = if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(header::CONTENT_TYPE, content_type);
        headers.insert(header::CONTENT_LENGTH, header_value_u64(selected.length())?);
        if range.is_some() {
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!(
                    "bytes {}-{}/{}",
                    selected.start, selected.end, asset.byte_size
                ))
                .map_err(|_| StreamError::InvalidAssetMetadata)?,
            );
        }

        let body = if head_only {
            drop(permit);
            Body::empty()
        } else {
            stream_file(file, selected, permit).await?
        };
        let mut response = Response::new(body);
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        Ok(response)
    }

    /// Reads one catalog-owned asset under the same no-follow root and
    /// concurrency boundary as streaming. The explicit byte cap is checked
    /// against durable metadata before opening and again while reading.
    pub async fn read_asset_bounded(
        &self,
        asset: &PlayableAsset,
        max_bytes: u64,
    ) -> Result<Vec<u8>, StreamError> {
        if asset.byte_size == 0 || asset.byte_size > max_bytes {
            return Err(StreamError::AssetUnavailable);
        }
        let permit = self.acquire().await?;
        let mut file = self.root.open_asset(asset)?;
        let capacity =
            usize::try_from(asset.byte_size).map_err(|_| StreamError::AssetUnavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut bytes = Vec::with_capacity(capacity);
            file.by_ref()
                .take(max_bytes.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|_| StreamError::AssetUnavailable)?;
            if bytes.len() != capacity {
                return Err(StreamError::AssetUnavailable);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| StreamError::AssetUnavailable)?
    }

    /// Streams an explicit inclusive byte range. The Jellyfin adapter parses
    /// ranges with its captured semantics and reuses this bounded stream.
    pub async fn stream_range(
        &self,
        asset: &PlayableAsset,
        start: u64,
        end_inclusive: u64,
    ) -> Result<Body, StreamError> {
        if asset.byte_size == 0 || end_inclusive < start || end_inclusive >= asset.byte_size {
            return Err(StreamError::InvalidRange);
        }
        let permit = self.acquire().await?;
        let file = self.root.open_asset(asset)?;
        stream_file(
            file,
            ByteRange {
                start,
                end: end_inclusive,
            },
            permit,
        )
        .await
    }

    /// Streams the whole asset (no Range handling).
    pub async fn stream_full(&self, asset: &PlayableAsset) -> Result<Body, StreamError> {
        if asset.byte_size == 0 {
            return Err(StreamError::AssetUnavailable);
        }
        self.stream_range(asset, 0, asset.byte_size - 1).await
    }

    /// Whether the catalog-recorded asset currently exists on disk with the
    /// recorded byte size (the captured "Audio file not found." precheck).
    pub fn asset_exists(&self, asset: &PlayableAsset) -> bool {
        self.root
            .relative_len(&asset.canonical_relative_path)
            .ok()
            .flatten()
            == Some(asset.byte_size)
    }

    /// Reads a sibling/relative file of unknown size with a hard byte cap —
    /// used for the captured `.lrc` lyrics sidecar lookup. Returns `None`
    /// when the file does not exist.
    pub async fn read_relative_bounded(
        &self,
        canonical_relative_path: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, StreamError> {
        let Some(byte_len) = self.root.relative_len(canonical_relative_path)? else {
            return Ok(None);
        };
        if byte_len == 0 || byte_len > max_bytes {
            return Err(StreamError::AssetUnavailable);
        }
        let permit = self.acquire().await?;
        let mut file = self.root.open_relative_file(canonical_relative_path)?;
        let capacity = usize::try_from(byte_len).map_err(|_| StreamError::AssetUnavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut bytes = Vec::with_capacity(capacity);
            file.by_ref()
                .take(max_bytes.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|_| StreamError::AssetUnavailable)?;
            if bytes.len() != capacity {
                return Err(StreamError::AssetUnavailable);
            }
            Ok(Some(bytes))
        })
        .await
        .map_err(|_| StreamError::AssetUnavailable)?
    }
}

fn header_value_u64(value: u64) -> Result<HeaderValue, StreamError> {
    HeaderValue::from_str(&value.to_string()).map_err(|_| StreamError::InvalidAssetMetadata)
}

async fn stream_file(
    mut file: File,
    range: ByteRange,
    permit: Admission,
) -> Result<Body, StreamError> {
    file.seek(SeekFrom::Start(range.start))
        .map_err(|_| StreamError::AssetUnavailable)?;
    let state = FileStreamState {
        file,
        remaining: range.length(),
        _permit: permit,
    };
    Ok(Body::from_stream(stream::try_unfold(
        state,
        |state| async move {
            if state.remaining == 0 {
                return Ok::<_, io::Error>(None);
            }
            let chunk_size = state.remaining.min(STREAM_CHUNK_SIZE as u64) as usize;
            let read = tokio::task::spawn_blocking(move || {
                let mut state = state;
                let mut buffer = vec![0_u8; chunk_size];
                let read = state.file.read(&mut buffer)?;
                buffer.truncate(read);
                Ok::<_, io::Error>((state, buffer, read))
            })
            .await
            .map_err(io::Error::other)??;
            let (mut state, buffer, read) = read;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "media asset ended before its catalog-recorded size",
                ));
            }
            state.remaining -= read as u64;
            Ok(Some((Bytes::from(buffer), state)))
        },
    )))
}

struct FileStreamState {
    file: File,
    remaining: u64,
    _permit: Admission,
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod platform {
    use std::{os::fd::AsFd, os::unix::ffi::OsStrExt, path::Path};

    use rustix::{
        fd::OwnedFd,
        fs::{CWD, Mode, OFlags, openat},
    };

    use super::*;

    fn io_error(error: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(error.raw_os_error())
    }

    pub fn open_root(path: &Path) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "canonical media root must be absolute",
            ));
        }
        let mut current = openat(
            CWD,
            b"/".as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io_error)?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                if matches!(component, Component::RootDir) {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "canonical media root contains traversal",
                ));
            };
            current = openat(
                &current,
                name.as_bytes(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(io_error)?;
        }
        Ok(File::from(current))
    }

    pub fn open_relative(root: &File, path: &Path) -> io::Result<File> {
        let mut components = path.components().peekable();
        let mut current: Option<OwnedFd> = None;
        while let Some(Component::Normal(name)) = components.next() {
            let is_leaf = components.peek().is_none();
            let flags = if is_leaf {
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW
            } else {
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
            };
            let next = match current.as_ref() {
                Some(parent) => openat(parent, name.as_bytes(), flags, Mode::empty()),
                None => openat(root.as_fd(), name.as_bytes(), flags, Mode::empty()),
            }
            .map_err(io_error)?;
            current = Some(next);
        }
        current
            .map(File::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "asset path is empty"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
mod platform {
    use super::*;

    pub fn open_root(_path: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant media streaming is unsupported on this platform",
        ))
    }

    pub fn open_relative(_root: &File, _path: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "race-resistant media streaming is unsupported on this platform",
        ))
    }
}
