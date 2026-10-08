//! Last.fm integration: account linking/session lifecycle and
//! scrobble/now-playing submission (PRD-EXT-001/003/004, PRD-AUTH-002;
//! legacy ADR-0003/0004; ADR-0134 §7 as amended 2026-08-14).
//!
//! Behavioral evidence: legacy C# behavior audit.
//! Preserved legacy semantics:
//!
//! - Web authentication flow (`auth.getSession`) only — the user's Last.fm
//!   password is never collected (legacy ADR-0004).
//! - `api_sig` is the Last.fm Web Services Authentication MD5 scheme:
//!   parameter name/value pairs concatenated without separators in sorted
//!   order, shared secret appended last, MD5 lowercase hex. MD5 is
//!   external-protocol mandated here (same compatibility category as the
//!   OpenSubsonic salted-password MD5, ADR-0130), not a security primitive.
//! - Scrobble timestamps are stamped at **processing time**; client-submitted
//!   timestamps are ignored (preserved quirk, evidence doc §4/§10.4).
//! - `trackNumber ?? 0`, `chosenByUser = !randomized` (no Rust surface carries
//!   a randomized flag, so scrobbles are always user-chosen).
//! - Single attempt, no batching, no retry, no offline queue: failures drop
//!   the scrobble (evidence doc §5). Last.fm API errors 4/9/10 clear the
//!   user's link; re-linking is the only recovery (legacy ADR-0004).
//! - Everything fails degraded: Last.fm outcomes never propagate into the
//!   playback/scrobble write path.
//!
//! Secrets: the raw per-user session key is never at rest in plaintext, never
//! logged, never traced, never serialized, and never exposed by any API. The
//! database holds a SHA-256 digest (lookup/audit) plus the AEAD-sealed key
//! (ChaCha20-Poly1305 under a key derived from the layered
//! `security.secretKey` setting); see ADR-0134 §7.
//!
//! Known divergence/gaps: the Rust catalog does not store MusicBrainz
//! recording IDs yet, so the optional `mbid` scrobble parameter is never sent
//! (it was nullable in legacy). `duration` is sent when the catalog knows the
//! track duration. Configured-state response shapes (`{url}`,
//! `{"message":"Last.fm linked"}`) are source-evidenced only — the
//! configured-state golden capture gap is recorded in the evidence doc.

use std::sync::Arc;
use std::time::Duration;

use crate::resource_bounds::ResourceGauge;

use md5::{Digest as _, Md5};
use melodee_auth::{
    LASTFM_SEALING_KEY_DOMAIN, SecretCipher, SecretString, derive_sealing_key,
    lastfm_session_key_digest,
};
use melodee_persistence::{LastfmLinkStore, ProviderStoreError, SettingsError, SettingsResolver};
use serde::Deserialize;
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::Instrument as _;
use uuid::Uuid;

// Setting keys use the exact legacy spellings, including the capital-E
// `scrobbling.lastFm.Enabled` quirk (evidence doc §6).
pub const SETTING_LASTFM_ENABLED: &str = "scrobbling.lastFm.Enabled";
pub const SETTING_LASTFM_API_KEY: &str = "scrobbling.lastFm.apiKey";
pub const SETTING_LASTFM_SHARED_SECRET: &str = "scrobbling.lastFm.sharedSecret";
pub const SETTING_SERVER_SECRET: &str = "security.secretKey";

/// Production Last.fm Web Services endpoint (legacy named HttpClient
/// "LastFm", evidence doc §3).
pub const LASTFM_API_BASE_URL: &str = "https://ws.audioscrobbler.com";
/// The last.fm page the client visits to authorize Melodee (evidence doc §3).
pub const LASTFM_AUTH_PAGE_URL: &str = "https://www.last.fm/api/auth/";
/// Legacy named-client timeout (evidence doc §3: 10 s).
const LASTFM_TIMEOUT: Duration = Duration::from_secs(10);
/// Response bodies are bounded; Last.fm payloads are a few KiB.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// Minimum server-secret length accepted for sealing-key derivation,
/// matching the JWT signing-key floor.
const MIN_SERVER_SECRET_BYTES: usize = 32;

/// Resolves the endpoint for route wiring: `Some` selects an explicit base
/// URL (per-test mock servers, carried on the adapter state); `None` selects
/// the production endpoint. There is deliberately no process-global override:
/// parallel tests must never share Last.fm mock state.
fn resolve_api_base_url(api_base_url: Option<&str>) -> String {
    api_base_url
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| LASTFM_API_BASE_URL.to_string())
}

/// Last.fm client/transport failures. Diagnostics never carry the session
/// key, tokens, or secrets.
#[derive(Debug, Error)]
pub enum LastfmError {
    #[error("Last.fm request failed")]
    Transport,
    #[error("Last.fm API error {code}")]
    Api { code: i64 },
    #[error("Last.fm response was malformed")]
    MalformedResponse,
}

impl LastfmError {
    /// Legacy ADR-0004 clear-session triggers: 4 (authentication failed),
    /// 9 (invalid session key), 10 (invalid API key).
    #[must_use]
    pub fn is_invalid_session(&self) -> bool {
        matches!(self, Self::Api { code } if matches!(code, 4 | 9 | 10))
    }
}

/// The Last.fm `api_sig` construction (evidence doc §3/§10.1): parameter
/// name/value pairs concatenated without separators in sorted order, the
/// shared secret appended last, MD5 lowercase hex. `format` and `api_sig`
/// itself are never signed.
fn build_api_signature(params: &[(String, String)], shared_secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort();
    let mut hasher = Md5::new();
    for (name, value) in sorted {
        hasher.update(name.as_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(shared_secret.as_bytes());
    crate::opensubsonic::params::hex_encode(&hasher.finalize())
}

/// RFC 3986 unreserved-character percent encoding (legacy
/// `Uri.EscapeDataString` behavior for the auth callback).
fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[(byte >> 4) as usize]));
            out.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    out
}

/// Builds the configured-state auth URL: the client visits last.fm and posts
/// the returned token back to Melodee (the server never calls
/// `auth.getToken`; evidence doc §3).
#[must_use]
pub fn build_auth_url(api_key: &str, callback: &str) -> String {
    format!(
        "{LASTFM_AUTH_PAGE_URL}?api_key={api_key}&cb={}",
        percent_encode(callback)
    )
}

/// The resolved Last.fm scrobbling configuration through the layered
/// settings (env → config file → database → legacy defaults). Blank
/// credentials mean "not configured" (legacy ADR-0003 graceful disable).
pub struct LastfmSettings {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub shared_secret: Option<SecretString>,
}

impl LastfmSettings {
    /// A client is constructible only with both credentials present.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some() && self.shared_secret.is_some()
    }
}

fn non_blank(value: Option<serde_json::Value>) -> Option<String> {
    value
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
}

/// Resolves the Last.fm settings through the layered resolver. Secret values
/// are the real values (internal use only — never log or serialize them).
pub async fn resolve_lastfm_settings(pool: &PgPool) -> Result<LastfmSettings, SettingsError> {
    let resolver = SettingsResolver::new(pool);
    let enabled = resolver
        .resolve(SETTING_LASTFM_ENABLED)
        .await?
        .value
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let api_key = non_blank(resolver.resolve(SETTING_LASTFM_API_KEY).await?.value);
    let shared_secret = non_blank(resolver.resolve(SETTING_LASTFM_SHARED_SECRET).await?.value)
        .map(SecretString::new);
    Ok(LastfmSettings {
        enabled,
        api_key,
        shared_secret,
    })
}

/// Builds the AEAD cipher for session-key sealing from the layered
/// `security.secretKey` setting (ADR-0134 §7 amended). `None` means the
/// deployment has no server secret configured and Last.fm linking fails
/// closed.
pub async fn sealing_cipher(pool: &PgPool) -> Result<Option<SecretCipher>, SettingsError> {
    let resolver = SettingsResolver::new(pool);
    let secret = non_blank(resolver.resolve(SETTING_SERVER_SECRET).await?.value);
    let Some(secret) = secret.filter(|secret| secret.len() >= MIN_SERVER_SECRET_BYTES) else {
        return Ok(None);
    };
    let key = derive_sealing_key(LASTFM_SEALING_KEY_DOMAIN, &SecretString::new(secret));
    Ok(SecretCipher::new(&key).ok())
}

/// Parsed `auth.getSession` response. The username is optional: the legacy
/// flow discards it (evidence doc §3) and it is stored only as display data.
pub struct LastfmSession {
    pub key: SecretString,
    pub username: Option<String>,
}

/// `error` codes are extracted by `parse_response` before this envelope is
/// decoded, so only the session payload is modeled here.
#[derive(Deserialize)]
struct SessionEnvelope {
    session: Option<SessionBody>,
}

#[derive(Deserialize)]
struct SessionBody {
    key: Option<String>,
    name: Option<String>,
}

/// One scrobble submission mapped to Last.fm parameters (evidence doc §4).
/// There is deliberately no `mbid` field: the Rust catalog does not store
/// MusicBrainz recording IDs yet, and the parameter was nullable in legacy.
pub struct ScrobbleSubmission {
    pub track: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// Legacy `SongNumber ?? 0` quirk: always sent, zero when unknown.
    pub track_number: i32,
    pub duration_seconds: Option<i32>,
    /// Processing-time stamp; client-submitted timestamps are ignored
    /// (preserved quirk, evidence doc §4/§10.4).
    pub timestamp_unix: i64,
    /// Legacy `!IsRandomizedScrobble`; no Rust surface carries a randomized
    /// flag, so submissions are always user-chosen.
    pub chosen_by_user: bool,
}

/// One now-playing submission (evidence doc §4: no duration or mbid).
pub struct NowPlayingSubmission {
    pub track: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_number: i32,
}

/// FOSS Last.fm Web Services client over reqwest (no new HTTP dependency).
/// One attempt per call: no retry, no batching, no queue (evidence doc §5).
#[derive(Clone)]
pub struct LastfmClient {
    http: reqwest::Client,
    api_base_url: String,
    api_key: String,
    shared_secret: SecretString,
}

impl LastfmClient {
    /// Client for the endpoint route wiring carries on the adapter state
    /// (`None` selects the production endpoint) with the legacy 10 s timeout.
    pub fn new(
        api_key: String,
        shared_secret: SecretString,
        api_base_url: Option<&str>,
    ) -> Result<Self, LastfmError> {
        Self::with_api_base_url(api_key, shared_secret, &resolve_api_base_url(api_base_url))
    }

    /// Client against an explicit base URL (the per-test seam for local mock
    /// servers; production wiring uses [`LastfmClient::new`] with `None`).
    pub fn with_api_base_url(
        api_key: String,
        shared_secret: SecretString,
        api_base_url: &str,
    ) -> Result<Self, LastfmError> {
        let http = reqwest::Client::builder()
            .connect_timeout(LASTFM_TIMEOUT)
            .timeout(LASTFM_TIMEOUT)
            .build()
            .map_err(|_| LastfmError::Transport)?;
        Ok(Self {
            http,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
            api_key,
            shared_secret,
        })
    }

    /// Reads a bounded response body and extracts the Last.fm `error` code
    /// when present (Last.fm returns error JSON on both 2xx and 4xx). The
    /// cap is enforced while streaming so a hostile or proxied endpoint
    /// cannot force an unbounded allocation before the limit is checked.
    async fn parse_response(response: reqwest::Response) -> Result<serde_json::Value, LastfmError> {
        use futures_util::StreamExt;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(LastfmError::MalformedResponse);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| LastfmError::Transport)?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(LastfmError::MalformedResponse);
            }
            body.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| LastfmError::MalformedResponse)?;
        if let Some(code) = value.get("error").and_then(serde_json::Value::as_i64) {
            return Err(LastfmError::Api { code });
        }
        Ok(value)
    }

    /// `auth.getSession` (GET, signed). Success returns the session key; the
    /// key is never logged or returned to any Melodee API caller.
    pub async fn get_session(&self, token: &str) -> Result<LastfmSession, LastfmError> {
        let params = vec![
            ("api_key".to_string(), self.api_key.clone()),
            ("method".to_string(), "auth.getSession".to_string()),
            ("token".to_string(), token.to_string()),
        ];
        let signature = build_api_signature(&params, self.shared_secret.expose_secret());
        let response = self
            .http
            .get(format!("{}/2.0/", self.api_base_url))
            .query(&[
                ("method", "auth.getSession"),
                ("api_key", self.api_key.as_str()),
                ("token", token),
                ("api_sig", signature.as_str()),
                ("format", "json"),
            ])
            .send()
            .await
            .map_err(|_| LastfmError::Transport)?;
        let value = Self::parse_response(response).await?;
        let envelope: SessionEnvelope =
            serde_json::from_value(value).map_err(|_| LastfmError::MalformedResponse)?;
        let Some(session) = envelope.session else {
            return Err(LastfmError::MalformedResponse);
        };
        let Some(key) = session.key.filter(|key| !key.is_empty()) else {
            return Err(LastfmError::MalformedResponse);
        };
        let username = session.name.filter(|name| !name.trim().is_empty());
        Ok(LastfmSession {
            key: SecretString::new(key),
            username,
        })
    }

    /// Signed POST helper. `format=json` is sent but never signed. The form
    /// body is encoded locally (reqwest's `form` feature is deliberately not
    /// enabled in this workspace).
    async fn signed_post(
        &self,
        params: Vec<(String, String)>,
    ) -> Result<serde_json::Value, LastfmError> {
        let signature = build_api_signature(&params, self.shared_secret.expose_secret());
        let mut form = params;
        form.push(("api_sig".to_string(), signature));
        form.push(("format".to_string(), "json".to_string()));
        let body = form
            .iter()
            .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let response = self
            .http
            .post(format!("{}/2.0/", self.api_base_url))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|_| LastfmError::Transport)?;
        Self::parse_response(response).await
    }

    /// `track.scrobble` — one track per call (legacy sends a single-element
    /// batch; evidence doc §4). Returns `true` iff Last.fm accepted the
    /// scrobble (`accepted > 0`).
    pub async fn scrobble(
        &self,
        session_key: &SecretString,
        submission: &ScrobbleSubmission,
    ) -> Result<bool, LastfmError> {
        let mut params = vec![
            ("album".to_string(), submission.album.clone()),
            ("albumArtist".to_string(), submission.album_artist.clone()),
            ("api_key".to_string(), self.api_key.clone()),
            (
                "chosenByUser".to_string(),
                if submission.chosen_by_user {
                    "1".to_string()
                } else {
                    "0".to_string()
                },
            ),
            ("method".to_string(), "track.scrobble".to_string()),
            ("sk".to_string(), session_key.expose_secret().to_string()),
            (
                "timestamp".to_string(),
                submission.timestamp_unix.to_string(),
            ),
            ("track".to_string(), submission.track.clone()),
            ("artist".to_string(), submission.artist.clone()),
            (
                "trackNumber".to_string(),
                submission.track_number.to_string(),
            ),
        ];
        if let Some(duration) = submission.duration_seconds {
            params.push(("duration".to_string(), duration.to_string()));
        }
        let value = self.signed_post(params).await?;
        let accepted = value
            .pointer("/scrobbles/@attr/accepted")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        Ok(accepted > 0)
    }

    /// `track.updateNowPlaying` (evidence doc §4: no timestamp, duration, or
    /// mbid).
    pub async fn update_now_playing(
        &self,
        session_key: &SecretString,
        submission: &NowPlayingSubmission,
    ) -> Result<(), LastfmError> {
        let params = vec![
            ("album".to_string(), submission.album.clone()),
            ("albumArtist".to_string(), submission.album_artist.clone()),
            ("api_key".to_string(), self.api_key.clone()),
            ("method".to_string(), "track.updateNowPlaying".to_string()),
            ("sk".to_string(), session_key.expose_secret().to_string()),
            ("track".to_string(), submission.track.clone()),
            ("artist".to_string(), submission.artist.clone()),
            (
                "trackNumber".to_string(),
                submission.track_number.to_string(),
            ),
        ];
        self.signed_post(params).await?;
        Ok(())
    }
}

/// The catalog facts needed to map a scrobble onto Last.fm parameters.
struct ScrobbleTrackFacts {
    title: String,
    track_artist: String,
    release_title: String,
    release_artist: String,
    track_number: Option<i32>,
    duration_ms: Option<i64>,
}

/// Loads the primary-billing artist names and release facts for one track.
/// Mirrors the adapters' catalog join shape (primary billing by presentation
/// order).
async fn load_track_facts(
    pool: &PgPool,
    track_public_id: Uuid,
) -> Result<Option<ScrobbleTrackFacts>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, String, String, Option<i32>, Option<i64>)>(
        "SELECT track.title, track_artist.canonical_name, release.title, \
                release_artist.canonical_name, track.track_number, \
                COALESCE(track.duration_ms, media.duration_ms) \
         FROM tracks track \
         JOIN releases release ON release.id = track.release_id AND release.status = 'active' \
         JOIN media_assets media ON media.id = track.playable_media_asset_id \
         JOIN LATERAL ( \
           SELECT artist.* FROM track_artists billing \
           JOIN artists artist ON artist.id = billing.artist_id \
           WHERE billing.track_id = track.id AND billing.role = 'primary' \
           ORDER BY billing.presentation_order, billing.id LIMIT 1 \
         ) track_artist ON true \
         JOIN LATERAL ( \
           SELECT artist.* FROM release_artists billing \
           JOIN artists artist ON artist.id = billing.artist_id \
           WHERE billing.release_id = release.id AND billing.role = 'primary' \
           ORDER BY billing.presentation_order, billing.id LIMIT 1 \
         ) release_artist ON true \
         WHERE track.public_api_id = $1",
    )
    .bind(track_public_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(title, track_artist, release_title, release_artist, track_number, duration_ms)| {
            ScrobbleTrackFacts {
                title,
                track_artist,
                release_title,
                release_artist,
                track_number,
                duration_ms,
            }
        },
    ))
}

/// Outcome of one dispatch, for tests and tracing. Every outcome is a
/// success from the caller's perspective: Last.fm failures degrade, never
/// propagate (evidence doc §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Last.fm accepted the submission.
    Submitted,
    /// Last.fm was not enabled/configured — the legacy silent no-op.
    NotConfigured,
    /// The user has no link — the legacy ADR-0004 silent no-op.
    NotLinked,
    /// The submission failed and was dropped (no retry, no queue). If the
    /// failure was an invalid-session error the link was cleared.
    Dropped,
}

/// Which submission kind a dispatch performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmissionKind {
    Scrobble,
    NowPlaying,
}

/// Fire-and-record Last.fm dispatch for the shared scrobble write path
/// (native `/api/v1/scrobble`, OpenSubsonic `scrobble`, Jellyfin
/// `Sessions/Playing*`). Constructed per call site from the pool; the
/// expensive HTTP client is built only after the enabled/configured/linked
/// gates pass, so the default (Last.fm disabled) path costs one settings
/// resolution and no HTTP setup.
#[derive(Clone)]
pub struct LastfmDispatch {
    pool: PgPool,
    api_base_url: String,
}

/// Process-global bound on concurrent outbound Last.fm calls (
/// PRD-INGEST-009). Dispatches are spawned from every scrobble/now-playing
/// write path across all three adapters and each call site builds its own
/// `LastfmDispatch`, so the bound cannot live on the struct: it must be
/// process state shared by every construction. Saturation queues the spawned
/// task on the semaphore instead of rejecting — the authoritative scrobble
/// is already durable in PostgreSQL, so the provider forward waits rather
/// than being dropped.
const LASTFM_DISPATCH_CONCURRENCY: usize = 4;

static DISPATCH_PERMITS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();
static DISPATCH_GAUGE: std::sync::OnceLock<Option<Arc<crate::resource_bounds::ResourceGauge>>> =
    std::sync::OnceLock::new();

fn dispatch_permits() -> Arc<tokio::sync::Semaphore> {
    Arc::clone(
        DISPATCH_PERMITS
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(LASTFM_DISPATCH_CONCURRENCY))),
    )
}

fn dispatch_gauge() -> Option<Arc<crate::resource_bounds::ResourceGauge>> {
    DISPATCH_GAUGE
        .get_or_init(|| {
            crate::resource_bounds::ResourceBounds::global().map(|registry| {
                registry.register("provider.lastfm.dispatch", LASTFM_DISPATCH_CONCURRENCY)
            })
        })
        .clone()
}

impl LastfmDispatch {
    /// Dispatch against the endpoint route wiring carries on the adapter
    /// state (`None` selects the production Last.fm endpoint).
    pub fn new(pool: PgPool, api_base_url: Option<&str>) -> Self {
        Self {
            pool,
            api_base_url: resolve_api_base_url(api_base_url),
        }
    }

    /// Dispatch against an explicit base URL (the per-test seam for local
    /// mock servers; production wiring uses [`LastfmDispatch::new`] with
    /// `None`).
    pub fn with_api_base_url(pool: PgPool, api_base_url: &str) -> Self {
        Self {
            pool,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Spawns a scrobble dispatch; the playback/scrobble write path never
    /// waits on Last.fm. The in-flight request's correlation id is
    /// captured here — task-locals do not cross `tokio::spawn` — and carried
    /// on the dispatch span.
    pub fn spawn_scrobble(&self, user_id: Uuid, track_public_id: Uuid) {
        let dispatch = self.clone();
        let correlation_id = crate::correlation::current();
        let span = tracing::info_span!(
            "lastfm_scrobble",
            %user_id,
            %track_public_id,
            correlation_id = tracing::field::Empty,
        );
        if let Some(id) = correlation_id {
            span.record("correlation_id", tracing::field::display(id));
        }
        tokio::spawn(
            async move {
                let _admission = dispatch.acquire_dispatch_permit().await;
                let outcome = dispatch.submit_scrobble(user_id, track_public_id).await;
                if outcome != DispatchOutcome::Submitted {
                    tracing::debug!(?outcome, "last.fm scrobble dispatch did not submit");
                }
            }
            .instrument(span),
        );
    }

    /// Spawns a now-playing dispatch; the write path never waits on Last.fm.
    pub fn spawn_now_playing(&self, user_id: Uuid, track_public_id: Uuid) {
        let dispatch = self.clone();
        let correlation_id = crate::correlation::current();
        let span = tracing::info_span!(
            "lastfm_now_playing",
            %user_id,
            %track_public_id,
            correlation_id = tracing::field::Empty,
        );
        if let Some(id) = correlation_id {
            span.record("correlation_id", tracing::field::display(id));
        }
        tokio::spawn(
            async move {
                let _admission = dispatch.acquire_dispatch_permit().await;
                let outcome = dispatch.submit_now_playing(user_id, track_public_id).await;
                if outcome != DispatchOutcome::Submitted {
                    tracing::debug!(?outcome, "last.fm now-playing dispatch did not submit");
                }
            }
            .instrument(span),
        );
    }

    /// Waits for a process-global dispatch slot. Dropping the returned future
    /// (task cancellation/shutdown) never admits, so cancelled waiters cannot
    /// consume a permit; a closed semaphore is unrecoverable process state and
    /// simply skips the provider forward, which is best-effort by contract.
    async fn acquire_dispatch_permit(
        &self,
    ) -> Option<(
        tokio::sync::OwnedSemaphorePermit,
        Option<crate::resource_bounds::GaugePermit>,
    )> {
        let permits = dispatch_permits();
        let gauge = dispatch_gauge();
        if let Some(gauge) = &gauge {
            gauge.begin_wait();
        }
        match permits.acquire_owned().await {
            Ok(permit) => Some((permit, gauge.as_ref().map(ResourceGauge::admit))),
            Err(error) => {
                if let Some(gauge) = &gauge {
                    gauge.wait_failed();
                }
                let _ = error;
                None
            }
        }
    }

    /// One scrobble submission, gated on `scrobbling.lastFm.Enabled` and a
    /// per-user link. Never fails outward.
    pub async fn submit_scrobble(&self, user_id: Uuid, track_public_id: Uuid) -> DispatchOutcome {
        self.submit(user_id, track_public_id, SubmissionKind::Scrobble)
            .await
    }

    /// One now-playing submission with the same gating. Never fails outward.
    pub async fn submit_now_playing(
        &self,
        user_id: Uuid,
        track_public_id: Uuid,
    ) -> DispatchOutcome {
        self.submit(user_id, track_public_id, SubmissionKind::NowPlaying)
            .await
    }

    async fn submit(
        &self,
        user_id: Uuid,
        track_public_id: Uuid,
        kind: SubmissionKind,
    ) -> DispatchOutcome {
        let settings = match resolve_lastfm_settings(&self.pool).await {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(%error, "last.fm settings resolution failed; dispatch skipped");
                return DispatchOutcome::NotConfigured;
            }
        };
        if !settings.enabled {
            return DispatchOutcome::NotConfigured;
        }
        let (Some(api_key), Some(shared_secret)) = (settings.api_key, settings.shared_secret)
        else {
            // Legacy guard: blank credentials → success/no-op ("Last.fm
            // scrobbling not configured.").
            return DispatchOutcome::NotConfigured;
        };
        let store = LastfmLinkStore::new();
        let link = match store.get(&self.pool, user_id).await {
            Ok(link) => link,
            Err(error) => {
                tracing::warn!(%error, "last.fm link lookup failed; dispatch skipped");
                return DispatchOutcome::Dropped;
            }
        };
        let Some(link) = link else {
            // Legacy ADR-0004: global scrobbling enabled but the user has no
            // session key → success/no-op.
            return DispatchOutcome::NotLinked;
        };
        let cipher = match sealing_cipher(&self.pool).await {
            Ok(Some(cipher)) => cipher,
            Ok(None) => {
                tracing::warn!("security.secretKey is not configured; last.fm dispatch skipped");
                return DispatchOutcome::Dropped;
            }
            Err(error) => {
                tracing::warn!(%error, "last.fm sealing key resolution failed; dispatch skipped");
                return DispatchOutcome::Dropped;
            }
        };
        let session_key = match cipher.open(&link.session_key_ciphertext) {
            Ok(plaintext) => match String::from_utf8(plaintext) {
                Ok(key) => SecretString::new(key),
                Err(_) => {
                    tracing::warn!(
                        "last.fm session key payload is not valid utf-8; dispatch skipped"
                    );
                    return DispatchOutcome::Dropped;
                }
            },
            Err(error) => {
                tracing::warn!(%error, "last.fm session key unsealing failed; dispatch skipped");
                return DispatchOutcome::Dropped;
            }
        };
        let facts = match load_track_facts(&self.pool, track_public_id).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(%error, "last.fm track fact lookup failed; dispatch skipped");
                return DispatchOutcome::Dropped;
            }
        };
        let Some(facts) = facts else {
            return DispatchOutcome::Dropped;
        };
        let client =
            match LastfmClient::with_api_base_url(api_key, shared_secret, &self.api_base_url) {
                Ok(client) => client,
                Err(error) => {
                    tracing::warn!(%error, "last.fm client construction failed; dispatch skipped");
                    return DispatchOutcome::Dropped;
                }
            };
        let result = match kind {
            SubmissionKind::Scrobble => {
                client
                    .scrobble(
                        &session_key,
                        &ScrobbleSubmission {
                            track: facts.title,
                            artist: facts.track_artist,
                            album: facts.release_title,
                            album_artist: facts.release_artist,
                            track_number: facts.track_number.unwrap_or(0),
                            duration_seconds: facts
                                .duration_ms
                                .map(|ms| i32::try_from(ms / 1000).unwrap_or(i32::MAX)),
                            // Preserved quirk: processing time, never the
                            // client-submitted timestamp.
                            timestamp_unix: OffsetDateTime::now_utc().unix_timestamp(),
                            chosen_by_user: true,
                        },
                    )
                    .await
            }
            SubmissionKind::NowPlaying => client
                .update_now_playing(
                    &session_key,
                    &NowPlayingSubmission {
                        track: facts.title,
                        artist: facts.track_artist,
                        album: facts.release_title,
                        album_artist: facts.release_artist,
                        track_number: facts.track_number.unwrap_or(0),
                    },
                )
                .await
                .map(|()| true),
        };
        match result {
            Ok(accepted) => {
                if accepted {
                    DispatchOutcome::Submitted
                } else {
                    // Last.fm ignored the scrobble (accepted = 0); legacy
                    // reports a false result and drops it.
                    DispatchOutcome::Dropped
                }
            }
            Err(error) => {
                if error.is_invalid_session() {
                    // Legacy ADR-0004 clear-on-invalid-session: re-linking is
                    // the only recovery (there is no refresh flow).
                    match store.unlink(&self.pool, user_id).await {
                        Ok(_) => {
                            tracing::warn!(%error, "last.fm session invalidated; link cleared")
                        }
                        Err(unlink_error) => {
                            tracing::warn!(%unlink_error, "last.fm link clearing failed")
                        }
                    }
                } else {
                    tracing::warn!(%error, "last.fm submission failed; scrobble dropped");
                }
                DispatchOutcome::Dropped
            }
        }
    }
}

/// Internal session-link failures; callers preserve the captured HTTP error.
#[derive(Debug, Error)]
pub enum LastfmLinkError {
    #[error("Last.fm sealing key resolution failed")]
    Settings(#[source] SettingsError),
    #[error("Last.fm sealing key is not configured")]
    MissingKey,
    #[error("Last.fm session sealing failed")]
    Seal(#[source] melodee_auth::CipherError),
    #[error("Last.fm link persistence failed")]
    Store(#[source] ProviderStoreError),
}

/// Persists a completed session exchange as the user's Last.fm link. The raw
/// session key is sealed and digested here and never reaches the store.
///
/// # Errors
/// Returns a typed failure with a warning logged;
/// callers map every failure to the captured 400 shapes.
pub async fn store_linked_session(
    pool: &PgPool,
    user_id: Uuid,
    session: &LastfmSession,
) -> Result<(), LastfmLinkError> {
    let Some(cipher) = sealing_cipher(pool).await.map_err(|error| {
        tracing::warn!(%error, "last.fm sealing key resolution failed");
        LastfmLinkError::Settings(error)
    })?
    else {
        tracing::warn!("security.secretKey is not configured; cannot store last.fm link");
        return Err(LastfmLinkError::MissingKey);
    };
    let ciphertext = cipher
        .seal(session.key.expose_secret().as_bytes())
        .map_err(|error| {
            tracing::warn!(%error, "last.fm session key sealing failed");
            LastfmLinkError::Seal(error)
        })?;
    let digest = lastfm_session_key_digest(session.key.expose_secret().as_bytes());
    LastfmLinkStore::new()
        .link(
            pool,
            user_id,
            session.username.as_deref(),
            &digest,
            &ciphertext,
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|error: ProviderStoreError| {
            tracing::warn!(%error, "last.fm link persistence failed");
            LastfmLinkError::Store(error)
        })
}

/// Removes the user's link; disconnect is idempotent (legacy: succeeds when
/// nothing is linked and when Last.fm is unconfigured).
pub async fn unlink_user(pool: &PgPool, user_id: Uuid) {
    match LastfmLinkStore::new().unlink(pool, user_id).await {
        Ok(unlinked) => tracing::debug!(unlinked, "last.fm disconnect processed"),
        Err(error) => tracing::warn!(%error, "last.fm unlink failed"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn api_signature_matches_a_hand_computed_lastfm_vector() {
        // From the Last.fm Web Services Authentication scheme: sorted
        // name+value pairs concatenated without separators, secret appended,
        // MD5 lowercase hex. Expected value computed independently from the
        // spec string
        // "api_keytest-api-keymethodauth.getSessiontokentest-tokentest-secret"
        // (independently computed: md5sum of that exact byte string).
        let params = vec![
            ("method".to_string(), "auth.getSession".to_string()),
            ("token".to_string(), "test-token".to_string()),
            ("api_key".to_string(), "test-api-key".to_string()),
        ];
        assert_eq!(
            build_api_signature(&params, "test-secret"),
            "c3b3e61eb13c87a69f8628488321f560"
        );
    }

    #[test]
    fn api_signature_sorts_parameters_before_hashing() {
        let forward = vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ];
        let reversed = vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ];
        assert_eq!(
            build_api_signature(&forward, "secret"),
            build_api_signature(&reversed, "secret")
        );
    }

    #[test]
    fn auth_url_percent_encodes_the_callback() {
        assert_eq!(
            build_auth_url("key", "https://client.example/callback?x=1&y=2"),
            "https://www.last.fm/api/auth/?api_key=key&cb=https%3A%2F%2Fclient.example%2Fcallback%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn invalid_session_codes_are_exactly_4_9_10() {
        for code in [4, 9, 10] {
            assert!(LastfmError::Api { code }.is_invalid_session());
        }
        for code in [2, 3, 6, 8, 11, 26, 29] {
            assert!(!LastfmError::Api { code }.is_invalid_session());
        }
        assert!(!LastfmError::Transport.is_invalid_session());
    }
}
