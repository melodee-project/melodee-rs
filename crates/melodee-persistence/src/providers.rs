//! external provider cache and Last.fm account-link
//! persistence (ADR-0134 §7 as amended 2026-08-14, PRD-EXT-001..004,
//! PRD-AUTH-002) over `external_provider_cache` and `native_lastfm_links`
//! (migration 0028).
//!
//! # Provider cache
//!
//! [`ProviderCacheStore`] is the provenance/cache home for external metadata
//! providers (PRD-EXT-003/004). Payloads are cache/evidence only — they
//! never become catalog identity (PRD-ARTIST-012, PRD-EXT-002). The provider
//! vocabulary is the enumerated legacy inventory (evidence:
//! legacy C# behavior audit §9). `request_key`
//! is a caller-hashed lookup key: raw requests can embed artist names or
//! tokens and are never persisted.
//!
//! # Last.fm links
//!
//! [`LastfmLinkStore`] persists per-user Last.fm session links (legacy
//! ADR-0004 lifecycle). The raw session key is **never at rest in
//! plaintext**: callers seal it with [`melodee_auth::SecretCipher`] and
//! digest it with [`melodee_auth::lastfm_session_key_digest`] before calling
//! [`LastfmLinkStore::link`], and decrypt
//! [`LastfmLinkRecord::session_key_ciphertext`] only at the signing
//! boundary. Neither the raw key nor the ciphertext is ever logged or
//! exposed by an API; the digest is the only audit/lookup representation.

use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Bounded provider identifier length, mirroring the migration CHECK.
pub const MAX_PROVIDER_CHARS: usize = 50;
/// Bounded hashed lookup-key length, mirroring the migration CHECK.
pub const MAX_REQUEST_KEY_CHARS: usize = 128;
/// Serialized-size bound for cached payloads, mirroring the migration CHECK.
pub const MAX_PROVIDER_PAYLOAD_BYTES: usize = 256 * 1024;
/// Bounded Last.fm username length, mirroring the migration CHECK.
pub const MAX_LASTFM_USERNAME_CHARS: usize = 64;
/// SHA-256 digest length of a Last.fm session key.
pub const SESSION_KEY_DIGEST_BYTES: usize = 32;
/// Bounded sealed session-key length, mirroring the migration CHECK
/// (12-byte nonce + 16-byte tag + the short session key).
pub const MIN_SESSION_KEY_CIPHERTEXT_BYTES: usize = 29;
pub const MAX_SESSION_KEY_CIPHERTEXT_BYTES: usize = 1024;

/// The enumerated legacy external-provider inventory (evidence doc §9),
/// sorted; the migration CHECK is the fail-closed backstop. "AMG" (named in
/// PRD-EXT-001 prose) has no legacy implementation and is deliberately
/// absent.
#[rustfmt::skip]
pub const KNOWN_PROVIDERS: &[&str] = &[
    "brave", "deezer", "discogs", "itunes", "lastfm",
    "metalapi", "musicbrainz", "spotify", "wikidata",
];

/// Whether the provider is in the enumerated inventory.
#[must_use]
pub fn is_known_provider(provider: &str) -> bool {
    KNOWN_PROVIDERS.contains(&provider)
}

type CacheRow = (
    String,
    String,
    serde_json::Value,
    OffsetDateTime,
    OffsetDateTime,
);

/// One `external_provider_cache` row.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderCacheEntry {
    pub provider: String,
    pub request_key: String,
    pub payload: serde_json::Value,
    pub fetched_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

impl From<CacheRow> for ProviderCacheEntry {
    fn from(row: CacheRow) -> Self {
        let (provider, request_key, payload, fetched_at, expires_at) = row;
        Self {
            provider,
            request_key,
            payload,
            fetched_at,
            expires_at,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid provider cache entry: {0}")]
    InvalidEntry(&'static str),
    #[error("invalid Last.fm link: {0}")]
    InvalidLink(&'static str),
}

fn validate_provider(provider: &str) -> Result<(), ProviderStoreError> {
    if provider.chars().count() > MAX_PROVIDER_CHARS {
        return Err(ProviderStoreError::InvalidEntry(
            "provider exceeds 50 characters",
        ));
    }
    if !is_known_provider(provider) {
        return Err(ProviderStoreError::InvalidEntry(
            "provider is not in the enumerated inventory",
        ));
    }
    Ok(())
}

fn validate_request_key(request_key: &str) -> Result<(), ProviderStoreError> {
    if request_key.trim().is_empty() {
        return Err(ProviderStoreError::InvalidEntry(
            "request_key must be non-blank",
        ));
    }
    if request_key.chars().count() > MAX_REQUEST_KEY_CHARS {
        return Err(ProviderStoreError::InvalidEntry(
            "request_key exceeds 128 characters",
        ));
    }
    Ok(())
}

/// Stateless entry point for the external provider cache.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderCacheStore;

impl ProviderCacheStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The cached payload for one (provider, request_key) when present and
    /// not expired at `now`. Expired rows are treated as absent (callers
    /// re-fetch); [`ProviderCacheStore::purge_expired`] reclaims them.
    pub async fn get_if_fresh(
        &self,
        pool: &PgPool,
        provider: &str,
        request_key: &str,
        now: OffsetDateTime,
    ) -> Result<Option<ProviderCacheEntry>, ProviderStoreError> {
        validate_provider(provider)?;
        validate_request_key(request_key)?;
        let row = sqlx::query_as::<_, CacheRow>(
            "SELECT provider, request_key, payload, fetched_at, expires_at \
             FROM external_provider_cache \
             WHERE provider=$1 AND request_key=$2 AND expires_at > $3",
        )
        .bind(provider)
        .bind(request_key)
        .bind(now)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(ProviderCacheEntry::from))
    }

    /// Creates or replaces one cache entry. The expiry must be strictly
    /// after the fetch time (the migration CHECK is the backstop).
    pub async fn upsert(
        &self,
        pool: &PgPool,
        entry: &ProviderCacheEntry,
    ) -> Result<(), ProviderStoreError> {
        validate_provider(&entry.provider)?;
        validate_request_key(&entry.request_key)?;
        let serialized = entry.payload.to_string();
        if serialized.len() > MAX_PROVIDER_PAYLOAD_BYTES {
            return Err(ProviderStoreError::InvalidEntry(
                "payload exceeds 262144 serialized bytes",
            ));
        }
        if entry.expires_at <= entry.fetched_at {
            return Err(ProviderStoreError::InvalidEntry(
                "expires_at must be after fetched_at",
            ));
        }
        sqlx::query(
            "INSERT INTO external_provider_cache \
               (provider, request_key, payload, fetched_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (provider, request_key) DO UPDATE SET \
               payload = EXCLUDED.payload, fetched_at = EXCLUDED.fetched_at, \
               expires_at = EXCLUDED.expires_at",
        )
        .bind(&entry.provider)
        .bind(&entry.request_key)
        .bind(&entry.payload)
        .bind(entry.fetched_at)
        .bind(entry.expires_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Removes every entry expired at `now`, returning the number removed.
    /// This is the single sanctioned deletion path; the cache is a
    /// rebuildable projection of provider responses, never authoritative.
    pub async fn purge_expired(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
    ) -> Result<u64, ProviderStoreError> {
        let result = sqlx::query("DELETE FROM external_provider_cache WHERE expires_at <= $1")
            .bind(now)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}

type LinkRow = (Uuid, Option<String>, Vec<u8>, Vec<u8>, OffsetDateTime);

/// One `native_lastfm_links` row. `session_key_digest` is the only
/// representation permitted in audit surfaces; `session_key_ciphertext`
/// leaves the store only toward the AEAD unsealing boundary in the Last.fm
/// client. Neither is ever logged, traced, serialized, or API-exposed, and
/// the raw session key is never persisted.
#[derive(Clone, PartialEq)]
pub struct LastfmLinkRecord {
    pub user_id: Uuid,
    pub lastfm_username: Option<String>,
    pub session_key_digest: [u8; SESSION_KEY_DIGEST_BYTES],
    pub session_key_ciphertext: Vec<u8>,
    pub linked_at: OffsetDateTime,
}

impl std::fmt::Debug for LastfmLinkRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LastfmLinkRecord")
            .field("user_id", &self.user_id)
            .field("lastfm_username", &self.lastfm_username)
            .field("session_key_digest", &"<redacted>")
            .field("session_key_ciphertext", &"<redacted>")
            .field("linked_at", &self.linked_at)
            .finish()
    }
}

impl TryFrom<LinkRow> for LastfmLinkRecord {
    type Error = ProviderStoreError;

    fn try_from(row: LinkRow) -> Result<Self, Self::Error> {
        let (user_id, lastfm_username, digest, ciphertext, linked_at) = row;
        let digest: [u8; SESSION_KEY_DIGEST_BYTES] = digest
            .try_into()
            .map_err(|_| ProviderStoreError::InvalidLink("stored digest is not 32 bytes"))?;
        Ok(Self {
            user_id,
            lastfm_username,
            session_key_digest: digest,
            session_key_ciphertext: ciphertext,
            linked_at,
        })
    }
}

/// Stateless entry point for per-user Last.fm links.
#[derive(Clone, Copy, Debug, Default)]
pub struct LastfmLinkStore;

impl LastfmLinkStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Creates or replaces one user's Last.fm link (legacy ADR-0004:
    /// re-authentication is the only recovery flow, so a re-link simply
    /// replaces the row). `digest` and `ciphertext` are produced by
    /// `melodee_auth::lastfm_session_key_digest` and
    /// `melodee_auth::SecretCipher::seal`; the raw session key never reaches
    /// this store.
    pub async fn link(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        lastfm_username: Option<&str>,
        digest: &[u8; SESSION_KEY_DIGEST_BYTES],
        ciphertext: &[u8],
        now: OffsetDateTime,
    ) -> Result<(), ProviderStoreError> {
        if let Some(username) = lastfm_username
            && (username.trim().is_empty() || username.chars().count() > MAX_LASTFM_USERNAME_CHARS)
        {
            return Err(ProviderStoreError::InvalidLink(
                "lastfm_username must be non-blank and at most 64 characters",
            ));
        }
        if ciphertext.len() < MIN_SESSION_KEY_CIPHERTEXT_BYTES
            || ciphertext.len() > MAX_SESSION_KEY_CIPHERTEXT_BYTES
        {
            return Err(ProviderStoreError::InvalidLink(
                "session_key_ciphertext must be within 29..=1024 bytes",
            ));
        }
        sqlx::query(
            "INSERT INTO native_lastfm_links \
               (user_id, lastfm_username, session_key_digest, session_key_ciphertext, linked_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id) DO UPDATE SET \
               lastfm_username = EXCLUDED.lastfm_username, \
               session_key_digest = EXCLUDED.session_key_digest, \
               session_key_ciphertext = EXCLUDED.session_key_ciphertext, \
               linked_at = EXCLUDED.linked_at",
        )
        .bind(user_id)
        .bind(lastfm_username)
        .bind(digest.as_slice())
        .bind(ciphertext)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// The user's link row, if linked.
    pub async fn get(
        &self,
        pool: &PgPool,
        user_id: Uuid,
    ) -> Result<Option<LastfmLinkRecord>, ProviderStoreError> {
        let row = sqlx::query_as::<_, LinkRow>(
            "SELECT user_id, lastfm_username, session_key_digest, session_key_ciphertext, linked_at \
             FROM native_lastfm_links WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
        row.map(LastfmLinkRecord::try_from).transpose()
    }

    /// Removes the user's link. Idempotent (legacy disconnect succeeds when
    /// nothing is linked); returns whether a row existed. Also the
    /// invalid-session clearing path (legacy ADR-0004: Last.fm auth errors
    /// 4/9/10 clear the stored key; re-linking is the only recovery).
    pub async fn unlink(&self, pool: &PgPool, user_id: Uuid) -> Result<bool, ProviderStoreError> {
        let result = sqlx::query("DELETE FROM native_lastfm_links WHERE user_id=$1")
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Whether the user has a link (admin/diagnostic existence check; never
    /// exposes key material).
    pub async fn exists(&self, pool: &PgPool, user_id: Uuid) -> Result<bool, ProviderStoreError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM native_lastfm_links WHERE user_id=$1)",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        Ok(exists)
    }

    /// Total linked accounts (admin/diagnostic count; never exposes key
    /// material).
    pub async fn count(&self, pool: &PgPool) -> Result<i64, ProviderStoreError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_lastfm_links")
            .fetch_one(pool)
            .await?;
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Effective provider eligibility (ADR-0212)
// ---------------------------------------------------------------------------

/// One external provider's settings contract. The enabled flag and the
/// credential inventory live here, next to the provider vocabulary, so the
/// capability endpoint, the catalog importer, the backfill worker and the
/// enrichment worker share one definition of "eligible" instead of each
/// re-deriving it from settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub provider: &'static str,
    pub display_name: &'static str,
    pub wire_id: &'static str,
    pub enabled_key: &'static str,
    pub base_url_key: &'static str,
    pub path: &'static str,
    pub credential_keys: &'static [&'static str],
}

pub const MELODEE_PROVIDER_WIRE_ID: &str = "018A798D-7B68-4F3E-80CD-1BAF03998C0B";

/// The enumerated external-provider registry, ordered by provider name.
/// MusicBrainz is eligible without credentials (the documented local option).
#[rustfmt::skip]
#[must_use]
pub fn provider_descriptors() -> Vec<ProviderDescriptor> {
    vec![
        ProviderDescriptor {
            provider: "musicbrainz",
            display_name: "Music Brainz Database",
            wire_id: MELODEE_PROVIDER_WIRE_ID,
            enabled_key: "searchEngine.musicbrainz.enabled",
            base_url_key: "searchEngine.musicbrainz.baseUrl",
            path: "artist",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "discogs",
            display_name: "Discogs Search Engine",
            wire_id: "5A8D2E5B-3C9D-4E7F-9B7A-A1B2C3D4E5F6",
            enabled_key: "searchEngine.discogs.enabled",
            base_url_key: "searchEngine.discogs.baseUrl",
            path: "database/search",
            credential_keys: &["searchEngine.discogs.userToken"],
        },
        ProviderDescriptor {
            provider: "spotify",
            display_name: "Spotify Service",
            wire_id: "BBAC49B7-0EDF-4D31-8A54-C9126509C2CE",
            enabled_key: "searchEngine.spotify.enabled",
            base_url_key: "searchEngine.spotify.baseUrl",
            path: "search",
            credential_keys: &["searchEngine.spotify.accessToken"],
        },
        ProviderDescriptor {
            provider: "itunes",
            display_name: "ITunes Search Engine",
            wire_id: "36E4DB7A-18BB-4A7D-86F7-A3C653A21611",
            enabled_key: "searchEngine.itunes.enabled",
            base_url_key: "searchEngine.itunes.baseUrl",
            path: "search",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "deezer",
            display_name: "Deezer Search Engine",
            wire_id: "A1528971-44B3-42A7-B658-DB96ABCEF10D",
            enabled_key: "searchEngine.deezer.enabled",
            base_url_key: "searchEngine.deezer.baseUrl",
            path: "search/artist",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "wikidata",
            display_name: "WikiData Search Engine",
            wire_id: "7B9C8E1F-2A3D-4E5B-8C7D-9E0F1A2B3C4D",
            enabled_key: "searchEngine.wikidata.enabled",
            base_url_key: "searchEngine.wikidata.baseUrl",
            path: "",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "brave",
            display_name: "Brave Search",
            wire_id: "F8B2C3D1-4E5A-6B7C-8D9E-0F1A2B3C4D5E",
            enabled_key: "searchEngine.brave.enabled",
            base_url_key: "searchEngine.brave.baseUrl",
            path: "res/v1/images/search",
            credential_keys: &["searchEngine.brave.apiKey"],
        },
        ProviderDescriptor {
            provider: "metalapi",
            display_name: "Metal-API",
            wire_id: "2B7F8D4E-5C9A-4E1B-8F3D-A7C2E6B9D4F1",
            enabled_key: "searchEngine.metalApi.enabled",
            base_url_key: "searchEngine.metalApi.baseUrl",
            path: "artists",
            credential_keys: &[],
        },
        ProviderDescriptor {
            provider: "lastfm",
            display_name: "Last FM Search Engine",
            wire_id: "3E43D998-2E9C-45B0-8128-EE6F0A41419E",
            enabled_key: "searchEngine.lastFm.Enabled",
            base_url_key: "searchEngine.lastFm.baseUrl",
            path: "",
            credential_keys: &["scrobbling.lastFm.apiKey"],
        },
    ]
}

/// The effective eligibility of one provider as derived from settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedProviderEligibility {
    pub provider: &'static str,
    pub metadata_eligible: bool,
    pub artwork_eligible: bool,
}

/// Resolves effective eligibility for every external provider.
///
/// A provider is eligible when its enabled flag resolves true through the
/// ordinary settings precedence and every required credential resolves to a
/// non-blank value. A blank or whitespace-only secret is "not configured";
/// the local MusicBrainz option requires no credential. Provider credentials
/// are never returned to callers or stored in the eligibility record.
pub async fn resolve_provider_eligibility(
    resolver: &crate::settings::SettingsResolver<'_>,
) -> Result<Vec<ResolvedProviderEligibility>, crate::settings::SettingsError> {
    let mut resolved = Vec::with_capacity(KNOWN_PROVIDERS.len());
    for descriptor in provider_descriptors() {
        let enabled = resolver
            .resolve(descriptor.enabled_key)
            .await?
            .value
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let mut configured = enabled;
        for key in descriptor.credential_keys {
            let blank = resolver
                .resolve(key)
                .await?
                .value
                .is_none_or(|value| value.as_str().is_none_or(|text| text.trim().is_empty()));
            if blank {
                configured = false;
            }
        }
        resolved.push(ResolvedProviderEligibility {
            provider: descriptor.provider,
            metadata_eligible: configured,
            artwork_eligible: configured,
        });
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn provider_descriptors_cover_the_vocabulary_once() {
        let descriptors = provider_descriptors();
        assert_eq!(descriptors.len(), KNOWN_PROVIDERS.len());
        let mut names: Vec<_> = descriptors.iter().map(|d| d.provider).collect();
        names.sort_unstable();
        assert_eq!(names, KNOWN_PROVIDERS.to_vec());
        for descriptor in &descriptors {
            assert!(is_known_provider(descriptor.provider));
            assert!(!descriptor.enabled_key.is_empty());
        }
        let musicbrainz = descriptors
            .iter()
            .find(|d| d.provider == "musicbrainz")
            .expect("musicbrainz descriptor");
        assert!(musicbrainz.credential_keys.is_empty());
    }

    #[test]
    fn provider_inventory_matches_the_migration_check() {
        assert_eq!(KNOWN_PROVIDERS.len(), 9);
        for pair in KNOWN_PROVIDERS.windows(2) {
            assert!(pair[0] < pair[1], "inventory must be sorted");
        }
        for provider in KNOWN_PROVIDERS {
            assert!(provider.chars().count() <= MAX_PROVIDER_CHARS);
        }
        // PRD-EXT-001 names "AMG", which has no legacy implementation
        // (evidence doc §9) and must not enter the vocabulary.
        assert!(!is_known_provider("amg"));
    }
}
