//! Rust-native external-provider boundary (ADR-0154).
//!
//! Providers are optional evidence sources. This module owns HTTP composition,
//! cache use, bounded calls, and normalization; it never allocates a
//! Melodee identity or promotes a provider result to a catalog decision.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Cursor,
    path::Path,
    sync::Arc,
    time::Duration,
};

use decentdb::{Db, DbConfig, Value};
use image::{ImageFormat, ImageReader};
use melodee_persistence::{
    KNOWN_PROVIDERS, ProviderCacheEntry, ProviderCacheStore, SettingsResolver,
};
use reqwest::{Client, StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::{sync::Semaphore, time::sleep};
use uuid::Uuid;

pub const MELODEE_PROVIDER_ID: &str = "018A798D-7B68-4F3E-80CD-1BAF03998C0B";
pub const SPOTIFY_PROVIDER_ID: &str = "BBAC49B7-0EDF-4D31-8A54-C9126509C2CE";
pub const ITUNES_PROVIDER_ID: &str = "36E4DB7A-18BB-4A7D-86F7-A3C653A21611";
pub const LAST_FM_PROVIDER_ID: &str = "3E43D998-2E9C-45B0-8128-EE6F0A41419E";
pub const DISCOGS_PROVIDER_ID: &str = "5A8D2E5B-3C9D-4E7F-9B7A-A1B2C3D4E5F6";
pub const WIKIDATA_PROVIDER_ID: &str = "7B9C8E1F-2A3D-4E5B-8C7D-9E0F1A2B3C4D";
pub const BRAVE_PROVIDER_ID: &str = "F8B2C3D1-4E5A-6B7C-8D9E-0F1A2B3C4D5E";
pub const BRAVE_ALBUM_PROVIDER_ID: &str = "A7C8D9E0-1F2A-3B4C-5D6E-7F8A9B0C1D2E";
pub const METAL_API_PROVIDER_ID: &str = "2B7F8D4E-5C9A-4E1B-8F3D-A7C2E6B9D4F1";
pub const METAL_API_ALBUM_PROVIDER_ID: &str = "8F3E5C42-7A9B-4D1E-9F2A-B8C4D7E9F1A3";
pub const DEEZER_PROVIDER_ID: &str = "A1528971-44B3-42A7-B658-DB96ABCEF10D";

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_RETRIES: usize = 2;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROVIDER_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_PROVIDER_IMAGE_PIXELS: u64 = 16_777_216;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCandidate {
    pub provider: String,
    #[serde(default)]
    pub local: bool,
    pub external_id: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub image_url: Option<String>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapability {
    pub provider: String,
    pub display_name: &'static str,
    pub wire_id: &'static str,
    pub enabled: bool,
    pub configured: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LookupResult {
    pub candidates: Vec<ProviderCandidate>,
    pub failed_providers: Vec<ProviderFailure>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub provider: String,
    pub category: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub category: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ProviderEnrichmentResponse {
    pub payload: serde_json::Value,
    pub candidates: Vec<ProviderCandidate>,
    pub cache_key: String,
    pub cache_hit: bool,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.category, self.message)
    }
}

impl std::error::Error for ProviderError {}

#[derive(Clone)]
pub struct ProviderRuntime {
    pool: PgPool,
    client: Client,
    permits: Arc<Semaphore>,
    provider_permits: Arc<BTreeMap<String, Arc<Semaphore>>>,
}

impl fmt::Debug for ProviderRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRuntime")
            .field("pool", &"<postgres>")
            .field("client", &"<http-client>")
            .finish_non_exhaustive()
    }
}

impl ProviderRuntime {
    pub fn new(pool: PgPool) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .connect_timeout(DEFAULT_TIMEOUT)
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|error| ProviderError {
                category: "client",
                message: error.to_string(),
            })?;
        Ok(Self {
            pool,
            client,
            permits: Arc::new(Semaphore::new(8)),
            provider_permits: Arc::new(
                descriptors()
                    .into_iter()
                    .map(|descriptor| (descriptor.provider.to_owned(), Arc::new(Semaphore::new(2))))
                    .collect(),
            ),
        })
    }

    pub async fn capabilities(&self) -> Result<Vec<ProviderCapability>, ProviderError> {
        let resolver = SettingsResolver::from_env(&self.pool).map_err(settings_error)?;
        let mut capabilities = Vec::with_capacity(KNOWN_PROVIDERS.len() + 1);
        capabilities.push(ProviderCapability {
            provider: "melodee".to_owned(),
            display_name: "Melodee Database",
            wire_id: MELODEE_PROVIDER_ID,
            enabled: true,
            configured: true,
        });
        for descriptor in descriptors() {
            let enabled = setting_bool(&resolver, descriptor.enabled_key, false).await?;
            let mut configured = enabled;
            for key in descriptor.credential_keys {
                if setting_text(&resolver, key)
                    .await?
                    .is_none_or(|value| value.trim().is_empty())
                {
                    configured = false;
                }
            }
            capabilities.push(ProviderCapability {
                provider: descriptor.provider.to_owned(),
                display_name: descriptor.display_name,
                wire_id: descriptor.wire_id,
                enabled: enabled && configured,
                configured,
            });
        }
        Ok(capabilities)
    }

    pub async fn lookup_artists(
        &self,
        query: &str,
        limit: usize,
        user_id: Option<Uuid>,
        selected: Option<&BTreeSet<String>>,
    ) -> Result<LookupResult, ProviderError> {
        let query = query.trim();
        let resolver = SettingsResolver::from_env(&self.pool).map_err(settings_error)?;
        let mut result = LookupResult::default();

        // Melodee self-search is always available and is deliberately first;
        // it provides useful results offline and has no provider authority.
        if selected.is_none_or(|providers| providers.contains("melodee")) {
            result.candidates = self.search_catalog(query, limit, user_id).await?;
        }

        let capabilities = self.capabilities().await?;
        for capability in capabilities.into_iter().skip(1) {
            if !capability.enabled
                || selected.is_some_and(|providers| !providers.contains(&capability.provider))
            {
                continue;
            }
            let Some(descriptor) = descriptors()
                .into_iter()
                .find(|descriptor| descriptor.provider == capability.provider)
            else {
                continue;
            };
            match self.lookup_one(&resolver, descriptor, query, limit).await {
                Ok((mut candidates, cache_hit)) => {
                    result.cache_hit |= cache_hit;
                    result.candidates.append(&mut candidates);
                }
                Err(error) => result.failed_providers.push(ProviderFailure {
                    provider: capability.provider,
                    category: error.category.to_owned(),
                    message: error.message,
                }),
            }
        }
        result.candidates.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.external_id.cmp(&right.external_id))
        });
        // Self-search wins ordering and duplicate suppression. This keeps the
        // established native response stable when external sources are down.
        result
            .candidates
            .sort_by_key(|candidate| if candidate.local { 0_u8 } else { 1_u8 });
        result.candidates.truncate(limit);
        Ok(result)
    }

    /// Fetches and caches one provider's Release-purpose response. The raw
    /// payload remains evidence so provider-specific fields can be interpreted
    /// later without making the provider a catalog identity authority.
    pub async fn enrich_release(
        &self,
        query: &str,
        provider: &str,
        limit: usize,
    ) -> Result<ProviderEnrichmentResponse, ProviderError> {
        let resolver = SettingsResolver::from_env(&self.pool).map_err(settings_error)?;
        if provider == "melodee" {
            return Err(ProviderError {
                category: "unsupported_purpose",
                message: "Melodee self-search provides Artist lookup only".to_owned(),
            });
        }
        let descriptor = descriptors()
            .into_iter()
            .find(|descriptor| descriptor.provider == provider)
            .ok_or(ProviderError {
                category: "unconfigured",
                message: "provider is not in the accepted provider registry".to_owned(),
            })?;
        let key = request_cache_key(provider, "release_metadata", query, limit);
        let cache = ProviderCacheStore::new();
        if let Some(entry) = cache
            .get_if_fresh(&self.pool, provider, &key, OffsetDateTime::now_utc())
            .await
            .map_err(|error| ProviderError {
                category: "cache",
                message: error.to_string(),
            })?
        {
            let candidates = parse_cached_release_candidates(provider, &entry.payload)?;
            return Ok(ProviderEnrichmentResponse {
                payload: entry.payload,
                candidates,
                cache_key: key,
                cache_hit: true,
            });
        }
        let base_url = setting_text(&resolver, descriptor.base_url_key)
            .await?
            .ok_or(ProviderError {
                category: "unconfigured",
                message: "provider base URL is not configured".to_owned(),
            })?;
        let mut payload = self
            .fetch_json(
                &resolver,
                descriptor,
                &base_url,
                query,
                limit,
                "release_metadata",
            )
            .await?;
        if provider == "musicbrainz"
            && extract_json_image_url(&payload).is_none()
            && let Some(release_id) = payload
                .pointer("/releases/0/id")
                .and_then(serde_json::Value::as_str)
        {
            payload["coverArtArchiveUrl"] = serde_json::Value::String(format!(
                "https://coverartarchive.org/release/{release_id}/front-500"
            ));
        }
        let candidates = parse_provider_release_candidates(provider, &payload)?;
        let now = OffsetDateTime::now_utc();
        let ttl_hours = setting_u64(&resolver, "searchEngine.provider.cacheTtlHours", 6).await?;
        let stored_payload = serde_json::json!({
            "purpose": "release_metadata",
            "provider": provider,
            "query": query,
            "payload": payload,
            "candidates": candidates,
            "provenance": { "fetchedAt": now, "cacheKey": key }
        });
        cache
            .upsert(
                &self.pool,
                &ProviderCacheEntry {
                    provider: provider.to_owned(),
                    request_key: key.clone(),
                    payload: stored_payload,
                    fetched_at: now,
                    expires_at: now + time::Duration::hours(i64::try_from(ttl_hours).unwrap_or(6)),
                },
            )
            .await
            .map_err(|error| ProviderError {
                category: "cache",
                message: error.to_string(),
            })?;
        Ok(ProviderEnrichmentResponse {
            payload,
            candidates,
            cache_key: key,
            cache_hit: false,
        })
    }

    /// Downloads and decodes one provider image, normalizing successful input
    /// to bounded PNG bytes before the caller persists it.
    pub async fn download_image(
        &self,
        provider: &str,
        url: &str,
    ) -> Result<(String, Vec<u8>), ProviderError> {
        let _permits = self.acquire_permits(provider).await?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| ProviderError {
                category: "transport",
                message: error.to_string(),
            })?;
        if !response.status().is_success() {
            return Err(ProviderError {
                category: "provider_error",
                message: format!("image endpoint returned HTTP {}", response.status()),
            });
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.to_ascii_lowercase().starts_with("image/"))
        {
            return Err(ProviderError {
                category: "invalid_image",
                message: "provider image response did not declare an image content type".to_owned(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PROVIDER_IMAGE_BYTES as u64)
        {
            return Err(ProviderError {
                category: "image_too_large",
                message: "provider image exceeded the configured byte bound".to_owned(),
            });
        }
        let bytes = response.bytes().await.map_err(|error| ProviderError {
            category: "transport",
            message: error.to_string(),
        })?;
        if bytes.is_empty() || bytes.len() > MAX_PROVIDER_IMAGE_BYTES {
            return Err(ProviderError {
                category: "image_too_large",
                message: "provider image exceeded the configured byte bound".to_owned(),
            });
        }
        let reader = ImageReader::new(Cursor::new(bytes.as_ref()))
            .with_guessed_format()
            .map_err(|error| ProviderError {
                category: "invalid_image",
                message: error.to_string(),
            })?;
        let image = reader.decode().map_err(|error| ProviderError {
            category: "invalid_image",
            message: error.to_string(),
        })?;
        let pixels = u64::from(image.width()).saturating_mul(u64::from(image.height()));
        if pixels == 0 || pixels > MAX_PROVIDER_IMAGE_PIXELS {
            return Err(ProviderError {
                category: "invalid_image",
                message: "provider image dimensions exceeded the configured bound".to_owned(),
            });
        }
        let mut normalized = Cursor::new(Vec::new());
        image
            .write_to(&mut normalized, ImageFormat::Png)
            .map_err(|error| ProviderError {
                category: "image_normalization",
                message: error.to_string(),
            })?;
        if normalized.get_ref().len() > MAX_PROVIDER_IMAGE_BYTES {
            return Err(ProviderError {
                category: "image_too_large",
                message: "normalized provider image exceeded the configured byte bound".to_owned(),
            });
        }
        Ok(("image/png".to_owned(), normalized.into_inner()))
    }

    async fn search_catalog(
        &self,
        query: &str,
        limit: usize,
        user_id: Option<Uuid>,
    ) -> Result<Vec<ProviderCandidate>, ProviderError> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT identity.provider, identity.external_id, artist.canonical_name, artist.sort_name
             FROM artist_external_identities identity
             JOIN artists artist ON artist.id = identity.artist_id
             WHERE artist.status <> 'merged' AND artist.canonical_name ILIKE '%' || $1 || '%'
               AND ($3::uuid IS NULL OR native_user_can_access_artist($3, artist.id))
             ORDER BY artist.sort_name, artist.public_api_id, identity.provider
             LIMIT $2",
        )
        .bind(query)
        .bind(i64::try_from(limit).unwrap_or(100))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| ProviderError {
            category: "database",
            message: error.to_string(),
        })?;
        Ok(rows
            .into_iter()
            .map(
                |(provider, external_id, name, sort_name)| ProviderCandidate {
                    provider,
                    local: true,
                    external_id,
                    name,
                    sort_name: Some(sort_name),
                    image_url: None,
                    thumbnail_url: None,
                },
            )
            .collect())
    }

    async fn lookup_one(
        &self,
        resolver: &SettingsResolver<'_>,
        descriptor: ProviderDescriptor,
        query: &str,
        limit: usize,
    ) -> Result<(Vec<ProviderCandidate>, bool), ProviderError> {
        let ttl_hours = setting_u64(resolver, "searchEngine.provider.cacheTtlHours", 6).await?;
        let base_url = setting_text(resolver, descriptor.base_url_key)
            .await?
            .ok_or(ProviderError {
                category: "unconfigured",
                message: "provider base URL is not configured".to_owned(),
            })?;
        let request_key = request_cache_key(descriptor.provider, "artist_metadata", query, limit);
        let cache = ProviderCacheStore::new();
        if let Some(entry) = cache
            .get_if_fresh(
                &self.pool,
                descriptor.provider,
                &request_key,
                OffsetDateTime::now_utc(),
            )
            .await
            .map_err(|error| ProviderError {
                category: "cache",
                message: error.to_string(),
            })?
        {
            return parse_cached_candidates(&entry.payload, descriptor.provider)
                .map(|candidates| (candidates, true));
        }

        let (payload, local_replica) = if descriptor.provider == "musicbrainz" {
            let replica_path =
                setting_text(resolver, "searchEngine.musicbrainz.storagePath").await?;
            let permits = self.acquire_permits("musicbrainz").await?;
            let local = local_musicbrainz_payload(replica_path.as_deref(), query).await;
            drop(permits);
            if let Some(payload) = local? {
                (payload, true)
            } else {
                (
                    self.fetch_json(
                        resolver,
                        descriptor,
                        &base_url,
                        query,
                        limit,
                        "artist_metadata",
                    )
                    .await?,
                    false,
                )
            }
        } else {
            (
                self.fetch_json(
                    resolver,
                    descriptor,
                    &base_url,
                    query,
                    limit,
                    "artist_metadata",
                )
                .await?,
                false,
            )
        };
        let candidates = parse_provider_candidates(descriptor.provider, &payload)?;
        let now = OffsetDateTime::now_utc();
        cache
            .upsert(
                &self.pool,
                &ProviderCacheEntry {
                    provider: descriptor.provider.to_owned(),
                    request_key: request_key.clone(),
                    payload: serde_json::json!({
                        "purpose": "artist_metadata",
                        "provider": descriptor.provider,
                        "query": query,
                        "transport": if local_replica { "local_replica" } else { "http" },
                        "candidates": candidates,
                        "provenance": { "fetchedAt": now, "cacheKey": request_key }
                    }),
                    fetched_at: now,
                    expires_at: now + time::Duration::hours(i64::try_from(ttl_hours).unwrap_or(6)),
                },
            )
            .await
            .map_err(|error| ProviderError {
                category: "cache",
                message: error.to_string(),
            })?;
        parse_cached_candidates(
            &serde_json::json!({ "candidates": candidates }),
            descriptor.provider,
        )
        .map(|candidates| (candidates, false))
    }

    async fn fetch_json(
        &self,
        resolver: &SettingsResolver<'_>,
        descriptor: ProviderDescriptor,
        base_url: &str,
        query: &str,
        limit: usize,
        purpose: &str,
    ) -> Result<serde_json::Value, ProviderError> {
        let user_agent = setting_text(resolver, "searchEngine.userAgent")
            .await?
            .unwrap_or_else(|| "Melodee/0.1 provider-client".to_owned());
        let timeout_key = format!("searchEngine.{}.timeoutMs", descriptor.provider);
        let timeout_ms = setting_u64(resolver, &timeout_key, 5_000).await?;
        let api_key = if let Some(key) = descriptor.credential_keys.first() {
            setting_text(resolver, key).await?
        } else {
            None
        };
        let mut url = endpoint(base_url, descriptor.path);
        let mut request = self
            .client
            .get(&url)
            .header("User-Agent", user_agent.clone())
            .timeout(Duration::from_millis(timeout_ms));
        match descriptor.provider {
            "itunes" => {
                request = request.query(&[
                    ("term", query.to_owned()),
                    (
                        "entity",
                        if purpose == "release_metadata" {
                            "album".to_owned()
                        } else {
                            "musicArtist".to_owned()
                        },
                    ),
                    ("limit", limit.to_string()),
                ]);
            }
            "deezer" => {
                url = endpoint(
                    base_url,
                    if purpose == "release_metadata" {
                        "search/album"
                    } else {
                        "search/artist"
                    },
                );
                request = self
                    .client
                    .get(url)
                    .query(&[("q", query.to_owned()), ("limit", limit.to_string())]);
            }
            "lastfm" => {
                request = request.query(&[
                    (
                        "method",
                        if purpose == "release_metadata" {
                            "album.search".to_owned()
                        } else {
                            "artist.search".to_owned()
                        },
                    ),
                    (
                        if purpose == "release_metadata" {
                            "album"
                        } else {
                            "artist"
                        },
                        query.to_owned(),
                    ),
                    ("format", "json".to_owned()),
                    ("limit", limit.to_string()),
                ]);
                if let Some(api_key) = api_key.as_deref().filter(|key| !key.trim().is_empty()) {
                    request = request.query(&[("api_key", api_key.to_owned())]);
                }
            }
            "wikidata" => {
                request = request.query(&[
                    ("query", format!("SELECT ?item ?itemLabel WHERE {{ ?item rdfs:label ?itemLabel . FILTER(CONTAINS(LCASE(?itemLabel), LCASE(\"{query}\"))) FILTER(LANG(?itemLabel)=\"en\") }} LIMIT {limit}")),
                    ("format", "json".to_owned()),
                ]);
            }
            "spotify" => {
                request = request.query(&[
                    ("q", query.to_owned()),
                    (
                        "type",
                        if purpose == "release_metadata" {
                            "album".to_owned()
                        } else {
                            "artist".to_owned()
                        },
                    ),
                    ("limit", limit.to_string()),
                ]);
                if let Some(access_token) = api_key.as_deref().filter(|key| !key.trim().is_empty())
                {
                    request = request.header("Authorization", format!("Bearer {access_token}"));
                }
            }
            "brave" => {
                let path = setting_text(resolver, "searchEngine.brave.imageSearchPath")
                    .await?
                    .unwrap_or_else(|| "res/v1/images/search".to_owned());
                url = endpoint(base_url, &path);
                request = self
                    .client
                    .get(url)
                    .query(&[("q", query.to_owned()), ("count", limit.to_string())]);
                if let Some(key) = api_key.as_deref().filter(|key| !key.is_empty()) {
                    request = request.header("X-Subscription-Token", key);
                }
            }
            "discogs" => {
                request = request.query(&[
                    ("q", query.to_owned()),
                    (
                        "type",
                        if purpose == "release_metadata" {
                            "release".to_owned()
                        } else {
                            "artist".to_owned()
                        },
                    ),
                    ("per_page", limit.to_string()),
                ]);
                if let Some(key) = api_key.as_deref().filter(|key| !key.is_empty()) {
                    request = request.header("Authorization", format!("Discogs token={key}"));
                }
            }
            "musicbrainz" => {
                url = endpoint(
                    base_url,
                    if purpose == "release_metadata" {
                        "release"
                    } else {
                        "artist"
                    },
                );
                request = self
                    .client
                    .get(url)
                    .header("User-Agent", user_agent.clone());
                request = request.query(&[
                    (
                        "query",
                        format!(
                            "{}:{query}",
                            if purpose == "release_metadata" {
                                "release"
                            } else {
                                "artist"
                            }
                        ),
                    ),
                    ("fmt", "json".to_owned()),
                    ("limit", limit.to_string()),
                ]);
            }
            "metalapi" => {
                url = endpoint(
                    base_url,
                    if purpose == "release_metadata" {
                        "releases"
                    } else {
                        "artists"
                    },
                );
                request = self
                    .client
                    .get(url)
                    .query(&[("search", query.to_owned()), ("limit", limit.to_string())]);
            }
            _ => {}
        }
        let _permits = self.acquire_permits(descriptor.provider).await?;
        for attempt in 0..MAX_RETRIES {
            let response = request
                .try_clone()
                .ok_or(ProviderError {
                    category: "request",
                    message: "provider request could not be cloned".to_owned(),
                })?
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let bytes = response.bytes().await.map_err(|error| ProviderError {
                        category: "transport",
                        message: error.to_string(),
                    })?;
                    if bytes.len() > MAX_RESPONSE_BYTES {
                        return Err(ProviderError {
                            category: "response_too_large",
                            message: "provider response exceeded the bounded response size"
                                .to_owned(),
                        });
                    }
                    return serde_json::from_slice(&bytes).map_err(|error| ProviderError {
                        category: "malformed_response",
                        message: error.to_string(),
                    });
                }
                Ok(response)
                    if response.status() == StatusCode::TOO_MANY_REQUESTS
                        || response.status().is_server_error() =>
                {
                    if attempt + 1 == MAX_RETRIES {
                        return Err(ProviderError {
                            category: if response.status() == StatusCode::TOO_MANY_REQUESTS {
                                "rate_limited"
                            } else {
                                "provider_error"
                            },
                            message: format!("provider returned HTTP {}", response.status()),
                        });
                    }
                    let delay = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .map_or(
                            Duration::from_millis(50 * (attempt as u64 + 1)),
                            Duration::from_secs,
                        );
                    sleep(delay.min(Duration::from_secs(2))).await;
                }
                Ok(response) => {
                    return Err(ProviderError {
                        category: "provider_error",
                        message: format!("provider returned HTTP {}", response.status()),
                    });
                }
                Err(error) => {
                    if attempt + 1 == MAX_RETRIES {
                        return Err(ProviderError {
                            category: if error.is_timeout() {
                                "timeout"
                            } else {
                                "transport"
                            },
                            message: error.to_string(),
                        });
                    }
                    sleep(Duration::from_millis(50 * (attempt as u64 + 1))).await;
                }
            }
        }
        Err(ProviderError {
            category: "provider_error",
            message: "provider call exhausted retries".to_owned(),
        })
    }

    async fn acquire_permits(
        &self,
        provider: &str,
    ) -> Result<
        (
            tokio::sync::OwnedSemaphorePermit,
            tokio::sync::OwnedSemaphorePermit,
        ),
        ProviderError,
    > {
        let global = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ProviderError {
                category: "cancelled",
                message: "provider concurrency gate closed".to_owned(),
            })?;
        let Some(provider_gate) = self.provider_permits.get(provider).cloned() else {
            return Err(ProviderError {
                category: "unconfigured",
                message: "provider concurrency gate is not registered".to_owned(),
            });
        };
        // Each accepted provider has a separate two-reader gate in addition
        // to the process-wide eight-request ceiling. This prevents one slow
        // provider from consuming the complete external budget.
        let provider_permit = provider_gate
            .acquire_owned()
            .await
            .map_err(|_| ProviderError {
                category: "cancelled",
                message: format!("{provider} provider concurrency gate closed"),
            })?;
        Ok((global, provider_permit))
    }
}

#[derive(Clone, Copy)]
struct ProviderDescriptor {
    provider: &'static str,
    display_name: &'static str,
    wire_id: &'static str,
    enabled_key: &'static str,
    base_url_key: &'static str,
    path: &'static str,
    credential_keys: &'static [&'static str],
}

fn descriptors() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            provider: "musicbrainz",
            display_name: "Music Brainz Database",
            wire_id: MELODEE_PROVIDER_ID,
            enabled_key: "searchEngine.musicbrainz.enabled",
            base_url_key: "searchEngine.musicbrainz.baseUrl",
            path: "artist",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "discogs",
            display_name: "Discogs Search Engine",
            wire_id: DISCOGS_PROVIDER_ID,
            enabled_key: "searchEngine.discogs.enabled",
            base_url_key: "searchEngine.discogs.baseUrl",
            path: "database/search",
            credential_keys: &["searchEngine.discogs.userToken"],
        },
        ProviderDescriptor {
            provider: "spotify",
            display_name: "Spotify Service",
            wire_id: SPOTIFY_PROVIDER_ID,
            enabled_key: "searchEngine.spotify.enabled",
            base_url_key: "searchEngine.spotify.baseUrl",
            path: "search",
            credential_keys: &["searchEngine.spotify.accessToken"],
        },
        ProviderDescriptor {
            provider: "itunes",
            display_name: "ITunes Search Engine",
            wire_id: ITUNES_PROVIDER_ID,
            enabled_key: "searchEngine.itunes.enabled",
            base_url_key: "searchEngine.itunes.baseUrl",
            path: "search",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "deezer",
            display_name: "Deezer Search Engine",
            wire_id: DEEZER_PROVIDER_ID,
            enabled_key: "searchEngine.deezer.enabled",
            base_url_key: "searchEngine.deezer.baseUrl",
            path: "search/artist",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "wikidata",
            display_name: "WikiData Search Engine",
            wire_id: WIKIDATA_PROVIDER_ID,
            enabled_key: "searchEngine.wikidata.enabled",
            base_url_key: "searchEngine.wikidata.baseUrl",
            path: "",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "brave",
            display_name: "Brave Search",
            wire_id: BRAVE_PROVIDER_ID,
            enabled_key: "searchEngine.brave.enabled",
            base_url_key: "searchEngine.brave.baseUrl",
            path: "res/v1/images/search",
            credential_keys: &["searchEngine.brave.apiKey"],
        },
        ProviderDescriptor {
            provider: "metalapi",
            display_name: "Metal-API",
            wire_id: METAL_API_PROVIDER_ID,
            enabled_key: "searchEngine.metalApi.enabled",
            base_url_key: "searchEngine.metalApi.baseUrl",
            path: "artists",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "lastfm",
            display_name: "Last FM Search Engine",
            wire_id: LAST_FM_PROVIDER_ID,
            enabled_key: "searchEngine.lastFm.Enabled",
            base_url_key: "searchEngine.lastFm.baseUrl",
            path: "",
            credential_keys: &["scrobbling.lastFm.apiKey"],
        },
    ]
}

async fn setting_text(
    resolver: &SettingsResolver<'_>,
    key: &str,
) -> Result<Option<String>, ProviderError> {
    Ok(resolver
        .resolve(key)
        .await
        .map_err(settings_error)?
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned)))
}

async fn setting_bool(
    resolver: &SettingsResolver<'_>,
    key: &str,
    default: bool,
) -> Result<bool, ProviderError> {
    Ok(resolver
        .resolve(key)
        .await
        .map_err(settings_error)?
        .value
        .and_then(|value| value.as_bool())
        .unwrap_or(default))
}

async fn setting_u64(
    resolver: &SettingsResolver<'_>,
    key: &str,
    default: u64,
) -> Result<u64, ProviderError> {
    Ok(resolver
        .resolve(key)
        .await
        .map_err(settings_error)?
        .value
        .and_then(|value| value.as_u64())
        .unwrap_or(default))
}

fn settings_error(error: melodee_persistence::SettingsError) -> ProviderError {
    ProviderError {
        category: "configuration",
        message: error.to_string(),
    }
}

fn endpoint(base: &str, path: &str) -> String {
    if path.is_empty() {
        base.to_owned()
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn extract_json_image_url(value: &serde_json::Value) -> Option<String> {
    fn walk(value: &serde_json::Value, depth: usize) -> Option<String> {
        if depth > 6 {
            return None;
        }
        match value {
            serde_json::Value::Object(object) => {
                for key in [
                    "image_url",
                    "imageUrl",
                    "artworkUrl100",
                    "artworkUrl600",
                    "coverArtArchiveUrl",
                    "cover",
                    "cover_xl",
                    "cover_big",
                    "thumbnail",
                    "url",
                ] {
                    if let Some(url) = object
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
                    {
                        return Some(url.to_owned());
                    }
                }
                object.values().find_map(|child| walk(child, depth + 1))
            }
            serde_json::Value::Array(values) => {
                values.iter().find_map(|child| walk(child, depth + 1))
            }
            _ => None,
        }
    }
    walk(value, 0)
}

async fn local_musicbrainz_payload(
    storage_path: Option<&str>,
    query: &str,
) -> Result<Option<serde_json::Value>, ProviderError> {
    let Some(storage_path) = storage_path.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let configured = Path::new(storage_path);
    let candidates = if configured.extension().is_some() {
        vec![configured.to_path_buf()]
    } else {
        vec![
            configured.join("musicbrainz.ddb"),
            configured.join("MusicBrainz.ddb"),
            configured.join("musicbrainz.db"),
            configured.join("artists.json"),
            configured.join("musicbrainz-artists.json"),
        ]
    };
    for path in candidates {
        let is_json = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        let value = if is_json {
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(ProviderError {
                        category: "local_replica",
                        message: format!(
                            "MusicBrainz replica read failed at {}: {error}",
                            path.display()
                        ),
                    });
                }
            };
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| ProviderError {
                category: "local_replica",
                message: format!("MusicBrainz replica JSON is malformed: {error}"),
            })?
        } else {
            if tokio::fs::metadata(&path).await.is_err() {
                continue;
            }
            local_musicbrainz_decentdb_payload(path.clone(), query.to_owned()).await?
        };
        let artists = value
            .get("artists")
            .and_then(serde_json::Value::as_array)
            .or_else(|| value.as_array())
            .cloned()
            .unwrap_or_default();
        let needle = query.to_lowercase();
        let artists = artists
            .into_iter()
            .filter(|artist| {
                !is_json
                    || artist
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| name.to_lowercase().contains(&needle))
            })
            .take(100)
            .collect::<Vec<_>>();
        return Ok(Some(serde_json::json!({ "artists": artists })));
    }
    Ok(None)
}

/// Reads the legacy MusicBrainz materialized `Artist` table without importing
/// the C# provider boundary. The query is read-only and runs on a blocking
/// task because DecentDB's embedded API is synchronous; the bounded provider
/// semaphore still limits concurrent calls at the async boundary.
async fn local_musicbrainz_decentdb_payload(
    path: std::path::PathBuf,
    query: String,
) -> Result<serde_json::Value, ProviderError> {
    tokio::task::spawn_blocking(move || {
        let db = Db::open(&path, DbConfig::default()).map_err(|error| ProviderError {
            category: "local_replica",
            message: format!(
                "MusicBrainz DecentDB open failed at {}: {error}",
                path.display()
            ),
        })?;
        let escaped_query = query.trim().to_lowercase().replace('\'', "''");
        let sql = format!(
            "SELECT a.MusicBrainzIdRaw, a.Name, a.SortName, a.NameNormalized \
             FROM Artist a \
             WHERE lower(a.NameNormalized) LIKE '%{escaped_query}%' \
                OR EXISTS (SELECT 1 FROM ArtistAlias alias \
                           WHERE alias.MusicBrainzArtistId = a.MusicBrainzArtistId \
                             AND lower(alias.NameNormalized) LIKE '%{escaped_query}%') \
             ORDER BY a.Id LIMIT 100"
        );
        let result = db.execute(&sql).map_err(|error| ProviderError {
            category: "local_replica",
            message: format!(
                "MusicBrainz DecentDB query failed at {}: {error}",
                path.display()
            ),
        })?;
        let artists = result
            .rows()
            .iter()
            .filter_map(|row| {
                let values = row.values();
                let external_id = value_text(values.first()?)?;
                let name = value_text(values.get(1)?)?;
                let sort_name = value_text(values.get(2)?)?;
                let normalized_name = value_text(values.get(3)?)?;
                Some(serde_json::json!({
                    "id": external_id,
                    "name": name,
                    "sort-name": sort_name,
                    "name-normalized": normalized_name,
                }))
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "artists": artists }))
    })
    .await
    .map_err(|error| ProviderError {
        category: "local_replica",
        message: format!("MusicBrainz DecentDB task failed: {error}"),
    })?
}

fn value_text(value: &Value) -> Option<String> {
    value.as_text().map(ToOwned::to_owned)
}

fn request_cache_key(provider: &str, purpose: &str, query: &str, limit: usize) -> String {
    blake3::hash(format!("{provider}\0{purpose}\0{}\0{limit}", query.to_lowercase()).as_bytes())
        .to_hex()
        .to_string()
}

fn parse_cached_candidates(
    payload: &serde_json::Value,
    provider: &str,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    serde_json::from_value::<Vec<ProviderCandidate>>(
        payload
            .get("candidates")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map(|mut candidates| {
        for candidate in &mut candidates {
            candidate.provider = provider.to_owned();
        }
        candidates
    })
    .map_err(|error| ProviderError {
        category: "malformed_cache",
        message: error.to_string(),
    })
}

fn parse_cached_release_candidates(
    provider: &str,
    payload: &serde_json::Value,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    if let Some(candidates) = payload.get("candidates") {
        return serde_json::from_value(candidates.clone()).map_err(|error| ProviderError {
            category: "malformed_cache",
            message: error.to_string(),
        });
    }
    let body = payload.get("payload").unwrap_or(payload);
    parse_provider_release_candidates(provider, body)
}

fn parse_provider_release_candidates(
    provider: &str,
    payload: &serde_json::Value,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    let rows = match provider {
        "musicbrainz" => payload
            .get("releases")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("id").and_then(json_string),
                    row.get("title").and_then(json_string),
                    row.pointer("/artist-credit/0/name").and_then(json_string),
                    extract_json_image_url(row),
                    None,
                )
            })
            .collect(),
        "discogs" => payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("id").and_then(json_string),
                    row.get("title").and_then(json_string),
                    None,
                    row.get("cover_image").and_then(json_string),
                    row.get("thumb").and_then(json_string),
                )
            })
            .collect(),
        "spotify" => payload
            .pointer("/albums/items")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("id").and_then(json_string),
                    row.get("name").and_then(json_string),
                    row.pointer("/artists/0/name").and_then(json_string),
                    row.pointer("/images/0/url").and_then(json_string),
                    None,
                )
            })
            .collect(),
        "itunes" => payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("collectionId").and_then(json_string),
                    row.get("collectionName").and_then(json_string),
                    row.get("artistName").and_then(json_string),
                    row.get("artworkUrl600")
                        .or_else(|| row.get("artworkUrl100"))
                        .and_then(json_string),
                    None,
                )
            })
            .collect(),
        "deezer" => payload
            .get("data")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("id").and_then(json_string),
                    row.get("title").and_then(json_string),
                    row.pointer("/artist/name").and_then(json_string),
                    row.get("cover_xl")
                        .or_else(|| row.get("cover_big"))
                        .and_then(json_string),
                    row.get("cover_medium").and_then(json_string),
                )
            })
            .collect(),
        "lastfm" => payload
            .pointer("/results/albummatches/album")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("mbid")
                        .and_then(json_string)
                        .or_else(|| row.get("url").and_then(json_string)),
                    row.get("name").and_then(json_string),
                    row.get("artist").and_then(json_string),
                    row.get("image")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|images| images.last())
                        .and_then(|image| image.get("#text"))
                        .and_then(json_string),
                    None,
                )
            })
            .collect(),
        "wikidata" => payload
            .pointer("/results/bindings")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.pointer("/item/value").and_then(json_string),
                    row.pointer("/itemLabel/value").and_then(json_string),
                    None,
                    None,
                    None,
                )
            })
            .collect(),
        "brave" => payload
            .get("results")
            .or_else(|| payload.get("data"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("url").and_then(json_string),
                    row.get("title").and_then(json_string),
                    None,
                    row.get("image_url").and_then(json_string),
                    row.get("thumbnail").and_then(json_string),
                )
            })
            .collect(),
        "metalapi" => payload
            .get("releases")
            .or_else(|| payload.get("data"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                release_candidate(
                    provider,
                    row.get("id").and_then(json_string),
                    row.get("name")
                        .or_else(|| row.get("title"))
                        .and_then(json_string),
                    row.pointer("/artist/name").and_then(json_string),
                    row.get("image").and_then(json_string),
                    None,
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(rows)
}

fn json_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

fn release_candidate(
    provider: &str,
    external_id: Option<String>,
    name: Option<String>,
    artist: Option<String>,
    image_url: Option<String>,
    thumbnail_url: Option<String>,
) -> Option<ProviderCandidate> {
    let external_id = external_id.filter(|value| !value.trim().is_empty())?;
    let name = name.filter(|value| !value.trim().is_empty())?;
    Some(ProviderCandidate {
        provider: provider.to_owned(),
        local: false,
        external_id,
        name: artist.map_or(name.clone(), |artist| format!("{artist} — {name}")),
        sort_name: None,
        image_url,
        thumbnail_url,
    })
}

fn parse_provider_candidates(
    provider: &str,
    payload: &serde_json::Value,
) -> Result<Vec<ProviderCandidate>, ProviderError> {
    let rows = match provider {
        "musicbrainz" => payload
            .get("artists")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("id")?.as_str()?.to_owned(),
                    name: row.get("name")?.as_str()?.to_owned(),
                    sort_name: row
                        .get("sort-name")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    image_url: None,
                    thumbnail_url: None,
                })
            })
            .collect(),
        "discogs" => payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("id")?.to_string(),
                    name: row.get("title")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row.get("uri")?.as_str().map(ToOwned::to_owned),
                    thumbnail_url: row
                        .get("thumb")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect(),
        "spotify" => payload
            .pointer("/artists/items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("id")?.as_str()?.to_owned(),
                    name: row.get("name")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("images")?
                        .as_array()?
                        .first()?
                        .get("url")?
                        .as_str()
                        .map(ToOwned::to_owned),
                    thumbnail_url: None,
                })
            })
            .collect(),
        "itunes" => payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("artistId")?.to_string(),
                    name: row.get("artistName")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("artworkUrl100")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    thumbnail_url: None,
                })
            })
            .collect(),
        "deezer" => payload
            .get("data")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("id")?.to_string(),
                    name: row.get("name")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("picture_xl")
                        .or_else(|| row.get("picture_big"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    thumbnail_url: row
                        .get("picture_medium")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect(),
        "lastfm" => payload
            .pointer("/results/artistmatches/artist")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row
                        .get("mbid")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty())
                        .or_else(|| row.get("url")?.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    name: row.get("name")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("image")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|images| images.last())
                        .and_then(|image| image.get("#text"))
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    thumbnail_url: None,
                })
            })
            .collect(),
        "wikidata" => payload
            .pointer("/results/bindings")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.pointer("/item/value")?.as_str()?.to_owned(),
                    name: row.pointer("/itemLabel/value")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: None,
                    thumbnail_url: None,
                })
            })
            .collect(),
        "brave" => payload
            .get("results")
            .or_else(|| payload.get("data"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("url")?.as_str()?.to_owned(),
                    name: row.get("title")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("image_url")
                        .or_else(|| row.get("url"))?
                        .as_str()
                        .map(ToOwned::to_owned),
                    thumbnail_url: row
                        .get("thumbnail")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect(),
        "metalapi" => payload
            .get("artists")
            .or_else(|| payload.get("data"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(ProviderCandidate {
                    provider: provider.to_owned(),
                    local: false,
                    external_id: row.get("id")?.to_string(),
                    name: row.get("name")?.as_str()?.to_owned(),
                    sort_name: None,
                    image_url: row
                        .get("image")
                        .or_else(|| row.get("image_url"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    thumbnail_url: None,
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(rows)
}

fn provider_by_wire_id(id: &str) -> Option<&'static str> {
    match id.to_ascii_uppercase().as_str() {
        MELODEE_PROVIDER_ID => Some("melodee"),
        SPOTIFY_PROVIDER_ID => Some("spotify"),
        ITUNES_PROVIDER_ID => Some("itunes"),
        LAST_FM_PROVIDER_ID => Some("lastfm"),
        DISCOGS_PROVIDER_ID => Some("discogs"),
        WIKIDATA_PROVIDER_ID => Some("wikidata"),
        BRAVE_PROVIDER_ID => Some("brave"),
        BRAVE_ALBUM_PROVIDER_ID => Some("brave"),
        METAL_API_PROVIDER_ID => Some("metalapi"),
        METAL_API_ALBUM_PROVIDER_ID => Some("metalapi"),
        DEEZER_PROVIDER_ID => Some("deezer"),
        _ => None,
    }
}

pub fn selected_provider_names(ids: Option<Vec<String>>) -> Option<BTreeSet<String>> {
    ids.map(|ids| {
        ids.into_iter()
            .filter_map(|id| provider_by_wire_id(&id).map(ToOwned::to_owned))
            .collect()
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn provider_parsers_keep_external_ids_and_artwork() {
        let candidates = parse_provider_candidates("itunes", &serde_json::json!({"results":[{"artistId":42,"artistName":"Fixture","artworkUrl100":"https://fixture/image.jpg"}]})).expect("parse fixture");
        assert_eq!(candidates[0].external_id, "42");
        assert_eq!(
            candidates[0].image_url.as_deref(),
            Some("https://fixture/image.jpg")
        );
    }

    #[test]
    fn cache_keys_are_bounded_and_query_specific() {
        let first = request_cache_key("itunes", "artist_metadata", "fixture", 5);
        let second = request_cache_key("itunes", "artist_metadata", "other", 5);
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }

    #[test]
    fn every_accepted_provider_has_a_deterministic_artist_parser() {
        let fixtures = [
            (
                "musicbrainz",
                serde_json::json!({"artists":[{"id":"mbid","name":"Fixture"}]}),
            ),
            (
                "discogs",
                serde_json::json!({"results":[{"id":7,"title":"Fixture","uri":"https://fixture"}]}),
            ),
            (
                "spotify",
                serde_json::json!({"artists":{"items":[{"id":"spotify-id","name":"Fixture","images":[{"url":"https://fixture/image"}]}]}}),
            ),
            (
                "itunes",
                serde_json::json!({"results":[{"artistId":7,"artistName":"Fixture"}]}),
            ),
            (
                "deezer",
                serde_json::json!({"data":[{"id":7,"name":"Fixture"}]}),
            ),
            (
                "lastfm",
                serde_json::json!({"results":{"artistmatches":{"artist":[{"mbid":"last-id","name":"Fixture"}]}}}),
            ),
            (
                "wikidata",
                serde_json::json!({"results":{"bindings":[{"item":{"value":"Q7"},"itemLabel":{"value":"Fixture"}}]}}),
            ),
            (
                "brave",
                serde_json::json!({"results":[{"url":"https://fixture","title":"Fixture"}]}),
            ),
            (
                "metalapi",
                serde_json::json!({"artists":[{"id":7,"name":"Fixture"}]}),
            ),
        ];
        for (provider, payload) in fixtures {
            let parsed = parse_provider_candidates(provider, &payload).expect("provider parser");
            assert_eq!(parsed.len(), 1, "{provider} parser produced no candidate");
            assert_eq!(parsed[0].name, "Fixture", "{provider} parser name");
            assert_eq!(parsed[0].provider, provider);
        }
    }

    #[tokio::test]
    async fn configured_decentdb_replica_is_read_as_musicbrainz_evidence() {
        let directory = tempdir().expect("create local replica fixture directory");
        let path = directory.path().join("musicbrainz.ddb");
        let db = Db::create(&path, DbConfig::default()).expect("create DecentDB fixture");
        db.execute(
            "CREATE TABLE Artist (Id INT64 PRIMARY KEY, MusicBrainzIdRaw TEXT, Name TEXT,
             SortName TEXT, NameNormalized TEXT, MusicBrainzArtistId INT64)",
        )
        .expect("create Artist table");
        db.execute("CREATE TABLE ArtistAlias (MusicBrainzArtistId INT64, NameNormalized TEXT)")
            .expect("create ArtistAlias table");
        db.execute(
            "INSERT INTO Artist VALUES (1, 'mb-fixture', 'Fixture Band', 'Fixture Band',
             'fixture band', 1)",
        )
        .expect("insert Artist fixture");
        drop(db);

        let payload = local_musicbrainz_payload(
            Some(path.to_str().expect("fixture path is UTF-8")),
            "Fixture",
        )
        .await
        .expect("read DecentDB replica")
        .expect("replica should be found");
        let candidates = parse_provider_candidates("musicbrainz", &payload).expect("parse replica");
        assert_eq!(candidates[0].external_id, "mb-fixture");
        assert_eq!(candidates[0].name, "Fixture Band");
    }

    #[test]
    fn every_accepted_provider_has_a_deterministic_release_parser() {
        let fixtures = [
            (
                "musicbrainz",
                serde_json::json!({"releases":[{"id":"mb-release","title":"Fixture"}]}),
            ),
            (
                "discogs",
                serde_json::json!({"results":[{"id":7,"title":"Fixture","cover_image":"https://fixture/image"}]}),
            ),
            (
                "spotify",
                serde_json::json!({"albums":{"items":[{"id":"spotify-release","name":"Fixture"}]}}),
            ),
            (
                "itunes",
                serde_json::json!({"results":[{"collectionId":7,"collectionName":"Fixture","artistName":"Artist"}]}),
            ),
            (
                "deezer",
                serde_json::json!({"data":[{"id":7,"title":"Fixture","artist":{"name":"Artist"}}]}),
            ),
            (
                "lastfm",
                serde_json::json!({"results":{"albummatches":{"album":[{"mbid":"last-release","name":"Fixture","artist":"Artist"}]}}}),
            ),
            (
                "wikidata",
                serde_json::json!({"results":{"bindings":[{"item":{"value":"Q7"},"itemLabel":{"value":"Fixture"}}]}}),
            ),
            (
                "brave",
                serde_json::json!({"results":[{"url":"https://fixture","title":"Fixture"}]}),
            ),
            (
                "metalapi",
                serde_json::json!({"releases":[{"id":7,"name":"Fixture"}]}),
            ),
        ];
        for (provider, payload) in fixtures {
            let parsed =
                parse_provider_release_candidates(provider, &payload).expect("release parser");
            assert_eq!(parsed.len(), 1, "{provider} parser produced no Release");
            assert!(
                parsed[0].name.contains("Fixture"),
                "{provider} parser title"
            );
        }
    }
}
