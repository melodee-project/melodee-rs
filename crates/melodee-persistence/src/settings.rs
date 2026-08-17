//! Phase 9 WP-08 DB-backed server settings and onboarding state (ADR-0134
//! §3, PRD-SETUP-001/002, PRD-CONFIG-001..003, PRD-AUTH-007) over
//! `server_settings` and `onboarding_state` (migration 0025).
//!
//! # Layered resolution (PRD-CONFIG-003)
//!
//! Effective setting values resolve through four layers, highest precedence
//! first (matching the legacy `MelodeeConfigurationFactory` direction: the
//! environment overlays the database, never the reverse):
//!
//! 1. **Environment** — `MELODEE__*` variables. Keys with an established
//!    server convention map explicitly (the `email.*` group uses the
//!    existing `MELODEE__EMAIL__*` constants; `jellyfin.token.pepper` uses
//!    `MELODEE__JELLYFIN__TOKEN_PEPPER`). Every other key maps mechanically:
//!    `MELODEE__` + the key with `.` replaced by `__`, uppercased (e.g.
//!    `processing.doDeleteOriginal` → `MELODEE__PROCESSING__DODELETEORIGINAL`).
//!    The legacy all-environment-variables overlay with `_`→`.` mapping is
//!    deliberately NOT reproduced (see `configuration-inventory.md` §"Rust-side
//!    mapping").
//! 2. **Config file** — an optional JSON object `{"key": value, ...}` at
//!    `MELODEE__CONFIG_FILE` for container deployments.
//! 3. **Database** — `server_settings` rows (operator-managed at runtime).
//! 4. **Built-in defaults** — the seeded legacy defaults from the 190-key
//!    inventory (`design/evidence/phase-9/configuration-inventory.md`),
//!    encoded in [`KNOWN_SETTINGS`].
//!
//! # Secret handling (PRD-AUTH-007)
//!
//! Keys classified secret in the inventory (12 hard secrets + 2
//! credential-adjacent keys) are write-only over every API/list/export
//! representation: reads return [`SettingValueView::Redacted`] and exports
//! substitute [`SECRET_REDACTION_MARKER`]. The marker is never accepted as a
//! storable value (fail-closed), so a redacted export can never overwrite a
//! real secret on import. Raw values are reachable only through
//! [`SettingsStore::read_raw_value`]/[`SettingsStore::list_raw`], whose
//! callers (the resolver and export/import machinery) must never log or
//! serialize them.
//!
//! Defaults for `encryption.privateKey` and `jellyfin.token.pepper` are
//! deliberately NOT carried from the legacy seed: both shipped publicly
//! known values and must be regenerated, never imported (inventory quirk 4).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Mirrored from the migration CHECK.
pub const MAX_SETTING_KEY_CHARS: usize = 255;
/// Mirrored from the migration CHECK (serialized jsonb bound).
pub const MAX_SETTING_VALUE_BYTES: usize = 64 * 1024;
/// Upper bound for settings listings; the API/CLI clamp to this.
pub const MAX_SETTINGS_LIST: u32 = 1000;
/// Bounded import document size (CLI read bound and server body limit).
pub const MAX_IMPORT_DOCUMENT_BYTES: usize = 1024 * 1024;
/// Bounded number of keys in one import document.
pub const MAX_IMPORT_KEYS: usize = 1000;
/// Bounded configuration-import audit trail kept in onboarding evidence.
pub const MAX_IMPORT_AUDIT_ENTRIES: usize = 20;
/// Fixed marker substituted for every secret value in list/get/export
/// representations. It is never accepted as a storable value.
pub const SECRET_REDACTION_MARKER: &str = "__MELODEE_SECRET_REDACTED__";
/// Format tag of the configuration export/import document.
pub const CONFIG_EXPORT_FORMAT: &str = "melodee-config-export/v1";
/// Environment key pointing at the optional JSON config-file layer.
pub const ENV_CONFIG_FILE: &str = "MELODEE__CONFIG_FILE";
/// Header warning carried by every export document (PRD-AUTH-007).
pub const CONFIG_EXPORT_WARNING: &str = "This file contains server configuration. Secret values \
are REDACTED and must be re-entered after import; redacted markers are never accepted as values. \
Handle this file as sensitive: non-secret values can still reveal infrastructure details.";

/// Value shape and validation rule for a known registry key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingKind {
    /// JSON boolean.
    Bool,
    /// JSON integer within the documented range (`None` = unbounded above).
    Int { min: i64, max: Option<i64> },
    /// JSON number within the documented inclusive range.
    Float { min: f64, max: f64 },
    /// JSON string restricted to the documented set.
    Enum(&'static [&'static str]),
    /// JSON string (legacy stored every value as a string; cron expressions,
    /// paths, and legacy JSON-in-string values stay strings).
    Text,
}

impl SettingKind {
    /// Parses a plain-text representation (environment value, config-file
    /// string, or built-in default) into the typed JSON value for this kind.
    /// Returns `None` when the text does not satisfy the kind.
    fn parse_text(&self, raw: &str) -> Option<serde_json::Value> {
        match self {
            Self::Bool => match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(serde_json::Value::Bool(true)),
                "false" | "0" | "no" => Some(serde_json::Value::Bool(false)),
                _ => None,
            },
            Self::Int { min, max } => raw
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|value| value >= min && max.is_none_or(|max| *value <= max))
                .map(serde_json::Value::from),
            Self::Float { min, max } => raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value >= min && value <= max)
                .map(serde_json::Value::from),
            Self::Enum(options) => options
                .contains(&raw)
                .then(|| serde_json::Value::String(raw.to_string())),
            Self::Text => Some(serde_json::Value::String(raw.to_string())),
        }
    }

    /// Validates a typed JSON value against this kind. The error text names
    /// the expected shape only — never the rejected value.
    pub fn validate(&self, value: &serde_json::Value) -> Result<(), String> {
        match (self, value) {
            (Self::Bool, serde_json::Value::Bool(_)) => Ok(()),
            (Self::Bool, _) => Err("expected a JSON boolean".to_string()),
            (Self::Int { min, max }, _) => {
                let number = value
                    .as_i64()
                    .ok_or_else(|| "expected a JSON integer".to_string())?;
                if number < *min || max.is_some_and(|max| number > max) {
                    return Err(match max {
                        Some(max) => format!("expected an integer between {min} and {max}"),
                        None => format!("expected an integer of at least {min}"),
                    });
                }
                Ok(())
            }
            (Self::Float { min, max }, _) => {
                let number = value
                    .as_f64()
                    .ok_or_else(|| "expected a JSON number".to_string())?;
                if number < *min || number > *max {
                    return Err(format!("expected a number between {min} and {max}"));
                }
                Ok(())
            }
            (Self::Enum(options), serde_json::Value::String(candidate))
                if options.contains(&candidate.as_str()) =>
            {
                Ok(())
            }
            (Self::Enum(options), _) => Err(format!(
                "expected one of: {}",
                options
                    .iter()
                    .map(|option| format!("'{option}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            (Self::Text, serde_json::Value::String(_)) => Ok(()),
            (Self::Text, _) => Err("expected a JSON string".to_string()),
        }
    }

    /// Typed built-in default, if the inventory records one.
    fn default_value(&self, default: Option<&'static str>) -> Option<serde_json::Value> {
        default.and_then(|raw| self.parse_text(raw))
    }
}

/// One known legacy-registry setting: classification, secret flag, built-in
/// default, and explicit environment-variable override.
pub struct KnownSetting {
    pub key: &'static str,
    pub kind: SettingKind,
    pub secret: bool,
    pub default: Option<&'static str>,
    pub env: Option<&'static str>,
}

impl KnownSetting {
    /// The typed built-in default value for this key, if any.
    #[must_use]
    pub fn default_value(&self) -> Option<serde_json::Value> {
        self.kind.default_value(self.default)
    }
}

/// The 190-key legacy configuration registry classification (evidence:
/// `design/evidence/phase-9/configuration-inventory.md`). Sorted by key;
/// a unit test pins the count and the sort order.
///
/// Casing quirks are verbatim (`defaults.pagesize`, `scrobbling.lastFm.Enabled`,
/// `searchEngine.lastFm.Enabled`, the `openSubsonicServer.*` variants, and the
/// `processing.dontDeleteExisitingMelodeeDataFiles` typo) because migration
/// parity keys off the exact strings (inventory "Parity quirks").
// Generated from the inventory by .tmp-wp08/gen_known_settings.py equivalent
// classification; keep sorted by `key`.
#[rustfmt::skip]
pub static KNOWN_SETTINGS: &[KnownSetting] = &[
    KnownSetting { key: "artist.biographyPlaceHolderText", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "conversion.bitrate", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("384"), env: None },
    KnownSetting { key: "conversion.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "conversion.samplingRate", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("48000"), env: None },
    KnownSetting { key: "conversion.vbrLevel", kind: SettingKind::Int { min: 0, max: Some(9) }, secret: false, default: Some("4"), env: None },
    KnownSetting { key: "defaults.batchSize", kind: SettingKind::Int { min: 250, max: Some(1000) }, secret: false, default: Some("250"), env: None },
    KnownSetting { key: "defaults.cacheDurationInMinutes", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("240"), env: None },
    KnownSetting { key: "defaults.dashboard.latestPageSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("50"), env: None },
    KnownSetting { key: "defaults.pagesize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("100"), env: None },
    KnownSetting { key: "email.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: Some("MELODEE__EMAIL__ENABLED") },
    KnownSetting { key: "email.fromEmail", kind: SettingKind::Text, secret: false, default: Some(""), env: Some("MELODEE__EMAIL__FROM_EMAIL") },
    KnownSetting { key: "email.fromName", kind: SettingKind::Text, secret: false, default: Some("Melodee"), env: Some("MELODEE__EMAIL__FROM_NAME") },
    KnownSetting { key: "email.resetPassword.htmlBodyTemplate", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "email.resetPassword.subject", kind: SettingKind::Text, secret: false, default: Some("Reset your Melodee password"), env: None },
    KnownSetting { key: "email.resetPassword.textBodyTemplate", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "email.smtpHost", kind: SettingKind::Text, secret: false, default: Some(""), env: Some("MELODEE__EMAIL__SMTP_HOST") },
    KnownSetting { key: "email.smtpPassword", kind: SettingKind::Text, secret: true, default: Some(""), env: Some("MELODEE__EMAIL__SMTP_PASSWORD") },
    KnownSetting { key: "email.smtpPort", kind: SettingKind::Int { min: 1, max: Some(65535) }, secret: false, default: Some("587"), env: Some("MELODEE__EMAIL__SMTP_PORT") },
    KnownSetting { key: "email.smtpUseSsl", kind: SettingKind::Bool, secret: false, default: Some("false"), env: Some("MELODEE__EMAIL__SMTP_USE_SSL") },
    KnownSetting { key: "email.smtpUseStartTls", kind: SettingKind::Bool, secret: false, default: Some("true"), env: Some("MELODEE__EMAIL__SMTP_USE_START_TLS") },
    KnownSetting { key: "email.smtpUsername", kind: SettingKind::Text, secret: true, default: Some(""), env: Some("MELODEE__EMAIL__SMTP_USERNAME") },
    KnownSetting { key: "encryption.privateKey", kind: SettingKind::Text, secret: true, default: None, env: None },
    KnownSetting { key: "filtering.lessThanDuration", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("720000"), env: None },
    KnownSetting { key: "filtering.lessThanSongCount", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("3"), env: None },
    KnownSetting { key: "formatting.dateTimeDisplayActivityFormat", kind: SettingKind::Text, secret: false, default: Some("hh\\:mm\\:ss\\.ffff"), env: None },
    KnownSetting { key: "formatting.dateTimeDisplayFormatShort", kind: SettingKind::Text, secret: false, default: Some("yyyyMMdd HH\\:mm"), env: None },
    KnownSetting { key: "imaging.doLoadEmbeddedImages", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "imaging.duplicateThreshold", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("80"), env: None },
    KnownSetting { key: "imaging.largeSize", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("1600"), env: None },
    KnownSetting { key: "imaging.maximumNumberOfAlbumImages", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("25"), env: None },
    KnownSetting { key: "imaging.maximumNumberOfArtistImages", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("25"), env: None },
    KnownSetting { key: "imaging.mediumSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("600"), env: None },
    KnownSetting { key: "imaging.minimumImageSize", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("300"), env: None },
    KnownSetting { key: "imaging.smallSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("300"), env: None },
    KnownSetting { key: "imaging.thumbnailSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("50"), env: None },
    KnownSetting { key: "jellyfin.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "jellyfin.rateLimit.apiPeriodSeconds", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("60"), env: None },
    KnownSetting { key: "jellyfin.rateLimit.apiRequestsPerPeriod", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("200"), env: None },
    KnownSetting { key: "jellyfin.rateLimit.streamConcurrentPerUser", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("2"), env: None },
    KnownSetting { key: "jellyfin.routePrefix", kind: SettingKind::Text, secret: false, default: Some("/api/jf"), env: None },
    KnownSetting { key: "jellyfin.token.allowLegacyHeaders", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "jellyfin.token.expiresAfterHours", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("168"), env: None },
    KnownSetting { key: "jellyfin.token.maxActivePerUser", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("10"), env: None },
    KnownSetting { key: "jellyfin.token.pepper", kind: SettingKind::Text, secret: true, default: None, env: Some("MELODEE__JELLYFIN__TOKEN_PEPPER") },
    KnownSetting { key: "jobs.artistHousekeeping.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 0/1 1/1 * ? *"), env: None },
    KnownSetting { key: "jobs.artistSearchEngineHousekeeping.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 0 * * ?"), env: None },
    KnownSetting { key: "jobs.chartUpdate.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 2 * * ?"), env: None },
    KnownSetting { key: "jobs.libraryInsert.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 0 * * ?"), env: None },
    KnownSetting { key: "jobs.libraryProcess.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 */10 * ? * *"), env: None },
    KnownSetting { key: "jobs.musicbrainzUpdateDatabase.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 12 1 * ?"), env: None },
    KnownSetting { key: "jobs.nowPlayingCleanup.cronExpression", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "jobs.podcastCleanup.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 0 2 * * ?"), env: None },
    KnownSetting { key: "jobs.podcastDownload.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 */5 * ? * *"), env: None },
    KnownSetting { key: "jobs.podcastRecovery.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 */30 * ? * *"), env: None },
    KnownSetting { key: "jobs.podcastRefresh.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 */15 * ? * *"), env: None },
    KnownSetting { key: "jobs.stagingAlbumRevalidation.cronExpression", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "jobs.stagingAutoMove.cronExpression", kind: SettingKind::Text, secret: false, default: Some("0 */15 * * * ?"), env: None },
    KnownSetting { key: "jukebox.backendType", kind: SettingKind::Enum(&["", "mpv", "mpd"]), secret: false, default: Some(""), env: None },
    KnownSetting { key: "jukebox.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "lyrics.filesEnabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "magic.doRemoveFeaturingArtistFromSongArtist", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doRemoveFeaturingArtistFromSongTitle", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doRemoveUnwantedTextFromAlbumTitle", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doRemoveUnwantedTextFromSongTitles", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doRenumberSongs", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doReplaceSongsArtistSeparators", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "magic.doSetYearToCurrentIfInvalid", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "magic.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "mpd.enableDebugOutput", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "mpd.host", kind: SettingKind::Text, secret: false, default: Some("localhost"), env: None },
    KnownSetting { key: "mpd.initialVolume", kind: SettingKind::Float { min: 0.0, max: 1.0 }, secret: false, default: Some("0.8"), env: None },
    KnownSetting { key: "mpd.instanceName", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "mpd.password", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "mpd.port", kind: SettingKind::Int { min: 1, max: Some(65535) }, secret: false, default: Some("6600"), env: None },
    KnownSetting { key: "mpd.timeoutMs", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("10000"), env: None },
    KnownSetting { key: "mpv.audioDevice", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "mpv.enableDebugOutput", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "mpv.extraArgs", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "mpv.initialVolume", kind: SettingKind::Float { min: 0.0, max: 1.0 }, secret: false, default: Some("0.8"), env: None },
    KnownSetting { key: "mpv.path", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "mpv.socketPath", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "openSubsonicServer.openSubsonic.serverSupportedVersion", kind: SettingKind::Text, secret: false, default: Some("1.16.1"), env: None },
    KnownSetting { key: "openSubsonicServer.openSubsonicServer.index.artistLimit", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("1000"), env: None },
    KnownSetting { key: "openSubsonicServer.openSubsonicServer.type", kind: SettingKind::Text, secret: false, default: Some("Melodee"), env: None },
    KnownSetting { key: "openSubsonicServer.openSubsonicServerLicenseEmail", kind: SettingKind::Text, secret: false, default: Some("noreply@localhost.lan"), env: None },
    KnownSetting { key: "partyMode.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "playlist.dynamicPlaylist.disabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "playlist.maximumAllowedPageSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("1000"), env: None },
    KnownSetting { key: "plugin.blackbeard.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "plugin.cueSheet.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "plugin.m3u.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "plugin.nfo.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "plugin.simpleFileVerification.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "podcast.download.maxConcurrent.global", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("2"), env: None },
    KnownSetting { key: "podcast.download.maxConcurrent.perUser", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("1"), env: None },
    KnownSetting { key: "podcast.download.maxEnclosureBytes", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("2147483648"), env: None },
    KnownSetting { key: "podcast.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "podcast.http.allowHttp", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "podcast.http.maxFeedBytes", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("10485760"), env: None },
    KnownSetting { key: "podcast.http.maxRedirects", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("10"), env: None },
    KnownSetting { key: "podcast.http.timeoutSeconds", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("30"), env: None },
    KnownSetting { key: "podcast.quota.maxBytesPerUser", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("5368709120"), env: None },
    KnownSetting { key: "podcast.recovery.orphanedUsageThresholdHours", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("12"), env: None },
    KnownSetting { key: "podcast.recovery.stuckDownloadThresholdMinutes", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("60"), env: None },
    KnownSetting { key: "podcast.refresh.maxItemsPerChannel", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("500"), env: None },
    KnownSetting { key: "podcast.retention.downloadedEpisodesInDays", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "podcast.retention.keepLastNEpisodes", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "podcast.retention.keepUnplayedOnly", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "processing.albumTitleRemovals", kind: SettingKind::Text, secret: false, default: Some("['^', '~', '#']"), env: None },
    KnownSetting { key: "processing.artistNameReplacements", kind: SettingKind::Text, secret: false, default: Some("{'AC/DC': ['AC; DC', 'AC;DC', 'AC/ DC', 'AC DC'] , 'Love/Hate': ['Love; Hate', 'Love;Hate', 'Love/ Hate', 'Love Hate'] }"), env: None },
    KnownSetting { key: "processing.convertedExtension", kind: SettingKind::Text, secret: false, default: Some("_converted"), env: None },
    KnownSetting { key: "processing.deleteSourceResidueAfterIngest", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "processing.doContinueOnDirectoryProcessingErrors", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "processing.doDeleteComments", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "processing.doDeleteOriginal", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "processing.doOverrideExistingMelodeeDataFiles", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "processing.doUseCurrentYearAsDefaultOrigAlbumYearValue", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "processing.dontDeleteExisitingMelodeeDataFiles", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "processing.duplicateAlbumPrefix", kind: SettingKind::Text, secret: false, default: Some("_duplicate_ "), env: None },
    KnownSetting { key: "processing.fileExtensionsToDelete", kind: SettingKind::Text, secret: false, default: Some("['log', 'lnk', 'lrc', 'doc']"), env: None },
    KnownSetting { key: "processing.ignoredArticles", kind: SettingKind::Text, secret: false, default: Some("THE\\|EL\\|LA\\|LOS\\|LAS\\|LE\\|LES\\|OS\\|AS\\|O\\|A"), env: None },
    KnownSetting { key: "processing.ignoredPerformers", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "processing.ignoredProduction", kind: SettingKind::Text, secret: false, default: Some("['www.t.me;pmedia_music']"), env: None },
    KnownSetting { key: "processing.ignoredPublishers", kind: SettingKind::Text, secret: false, default: Some("['P.M.E.D.I.A','PMEDIA','PMEDIA GROUP']"), env: None },
    KnownSetting { key: "processing.maximumAlbumDirectoryNameLength", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("255"), env: None },
    KnownSetting { key: "processing.maximumArtistDirectoryNameLength", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("255"), env: None },
    KnownSetting { key: "processing.maximumProcessingCount", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "processing.processedExtension", kind: SettingKind::Text, secret: false, default: Some("_processed"), env: None },
    KnownSetting { key: "processing.songTitleRemovals", kind: SettingKind::Text, secret: false, default: Some("[';', '(Remaster)', 'Remaster']"), env: None },
    KnownSetting { key: "register.disabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "register.privateCode", kind: SettingKind::Text, secret: true, default: None, env: None },
    KnownSetting { key: "scripting.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "scripting.postDiscoveryScript", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "scripting.preDiscoveryScript", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "scrobbling.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "scrobbling.lastFm.Enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "scrobbling.lastFm.apiKey", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "scrobbling.lastFm.sharedSecret", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.artistSearchDatabaseRefreshInDays", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("14"), env: None },
    KnownSetting { key: "searchEngine.brave.apiKey", kind: SettingKind::Text, secret: true, default: None, env: None },
    KnownSetting { key: "searchEngine.brave.baseUrl", kind: SettingKind::Text, secret: false, default: Some("https://api.search.brave.com"), env: None },
    KnownSetting { key: "searchEngine.brave.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "searchEngine.brave.imageSearchPath", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "searchEngine.deezer.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "searchEngine.defaultPageSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("20"), env: None },
    KnownSetting { key: "searchEngine.discogs.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "searchEngine.discogs.userToken", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.itunes.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "searchEngine.lastFm.Enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "searchEngine.maximumAllowedPageSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("1000"), env: None },
    KnownSetting { key: "searchEngine.metalApi.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "searchEngine.musicbrainz.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "searchEngine.musicbrainz.importBatchSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("50000"), env: None },
    KnownSetting { key: "searchEngine.musicbrainz.importLastImportTimestamp", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.musicbrainz.importMaximumToProcess", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "searchEngine.musicbrainz.storagePath", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "searchEngine.spotify.accessToken", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.spotify.apiKey", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.spotify.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "searchEngine.spotify.sharedSecret", kind: SettingKind::Text, secret: true, default: Some(""), env: None },
    KnownSetting { key: "searchEngine.userAgent", kind: SettingKind::Text, secret: false, default: Some("Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"), env: None },
    KnownSetting { key: "searchEngine.wikidata.enabled", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "searchResults.defaultPageSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("50"), env: None },
    KnownSetting { key: "security.blacklistedEmails", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "security.blacklistedIPs", kind: SettingKind::Text, secret: false, default: Some(""), env: None },
    KnownSetting { key: "security.passwordResetTokenExpiryMinutes", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("60"), env: None },
    KnownSetting { key: "security.secretKey", kind: SettingKind::Text, secret: true, default: None, env: None },
    KnownSetting { key: "streaming.maxConcurrentStreams.global", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "streaming.maxConcurrentStreams.perUser", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("0"), env: None },
    KnownSetting { key: "streaming.useBufferedResponses", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "system.baseUrl", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "system.defaultTheme", kind: SettingKind::Text, secret: false, default: Some("melodee-dark"), env: None },
    KnownSetting { key: "system.isDownloadingEnabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "system.maxUploadSize", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("5242880"), env: None },
    KnownSetting { key: "system.onboardingCompletedAt", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "system.siteName", kind: SettingKind::Text, secret: false, default: Some("Melodee"), env: None },
    KnownSetting { key: "theme.enforceContrastValidation", kind: SettingKind::Bool, secret: false, default: Some("false"), env: None },
    KnownSetting { key: "theme.libraryPath", kind: SettingKind::Text, secret: false, default: None, env: None },
    KnownSetting { key: "theme.maxUploadSizeMb", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("50"), env: None },
    // ADR-0147: the legacy seed literals were malformed (unbalanced quotes,
    // single-quoted "JSON") and unconsumed by any legacy code path
    // (ADR-0108 Phase 0 ratification). The corrected defaults below are
    // parseable JSON template objects consumed by the streaming transcoding
    // service; placeholders %s (source path), %t (start offset seconds), and
    // %bk (target bitrate kbps) are substituted at launch.
    KnownSetting { key: "transcoding.command.aac", kind: SettingKind::Text, secret: false, default: Some("{\"format\":\"Aac\",\"bitrate\":256,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a aac -f adts -\"}"), env: None },
    KnownSetting { key: "transcoding.command.mp3", kind: SettingKind::Text, secret: false, default: Some("{\"format\":\"Mp3\",\"bitrate\":192,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -f mp3 -\"}"), env: None },
    KnownSetting { key: "transcoding.command.opus", kind: SettingKind::Text, secret: false, default: Some("{\"format\":\"Opus\",\"bitrate\":128,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -c:a libopus -f opus -\"}"), env: None },
    KnownSetting { key: "transcoding.default", kind: SettingKind::Enum(&["raw", "aac", "mp3", "opus"]), secret: false, default: Some("raw"), env: None },
    // ADR-0147 decision 5: transcoding capacity is bounded independently of
    // direct-play streaming.maxConcurrentStreams.*; 0 disables transcoding
    // (transcode requests fail closed; direct play is unaffected).
    KnownSetting { key: "transcoding.maxConcurrent", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("4"), env: None },
    KnownSetting { key: "userDeviceProfile.enabled", kind: SettingKind::Bool, secret: false, default: Some("true"), env: None },
    KnownSetting { key: "userinterface.toastAutoCloseTime", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("2000"), env: None },
    KnownSetting { key: "validation.maximumAlbumYear", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("2150"), env: None },
    KnownSetting { key: "validation.maximumSongNumber", kind: SettingKind::Int { min: 1, max: None }, secret: false, default: Some("9999"), env: None },
    KnownSetting { key: "validation.minimumAlbumDuration", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("10"), env: None },
    KnownSetting { key: "validation.minimumAlbumYear", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("1860"), env: None },
    KnownSetting { key: "validation.minimumSongCount", kind: SettingKind::Int { min: 0, max: None }, secret: false, default: Some("3"), env: None },];

/// Looks up a key in [`KNOWN_SETTINGS`] (binary search; the table is sorted).
#[must_use]
pub fn known_setting(key: &str) -> Option<&'static KnownSetting> {
    KNOWN_SETTINGS
        .binary_search_by(|entry| entry.key.cmp(key))
        .ok()
        .map(|index| &KNOWN_SETTINGS[index])
}

/// Whether the key is classified secret by the inventory.
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    known_setting(key).is_some_and(|known| known.secret)
}

/// Environment variable name for a setting key: the explicit established
/// convention when one exists, else the mechanical
/// `MELODEE__<KEY with . → __, uppercased>` mapping (see module docs).
#[must_use]
pub fn env_var_name(key: &str) -> String {
    match known_setting(key).and_then(|known| known.env) {
        Some(name) => name.to_string(),
        None => format!("MELODEE__{}", key.replace('.', "__").to_uppercase()),
    }
}

/// The value of a setting as exposed to API/list/export surfaces: secret
/// values are always redacted, never serialized.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValueView {
    /// Secret value withheld; the marker is emitted instead.
    Redacted,
    /// Non-secret value.
    Value(serde_json::Value),
}

impl SettingValueView {
    /// The serializable representation: the fixed marker for redacted
    /// secrets, the value otherwise.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Redacted => serde_json::Value::String(SECRET_REDACTION_MARKER.to_string()),
            Self::Value(value) => value.clone(),
        }
    }
}

type SettingRow = (String, serde_json::Value, bool, OffsetDateTime);

/// One `server_settings` row as exposed over APIs (secrets redacted).
#[derive(Clone, Debug, PartialEq)]
pub struct ServerSetting {
    pub key: String,
    pub value: SettingValueView,
    pub is_secret: bool,
    pub updated_at: OffsetDateTime,
}

impl From<SettingRow> for ServerSetting {
    fn from(row: SettingRow) -> Self {
        let (key, value, is_secret, updated_at) = row;
        Self {
            key,
            value: if is_secret {
                SettingValueView::Redacted
            } else {
                SettingValueView::Value(value)
            },
            is_secret,
            updated_at,
        }
    }
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid setting key: {0}")]
    InvalidKey(&'static str),
    #[error("invalid value for setting {key}: {reason}")]
    InvalidValue { key: String, reason: String },
    #[error(
        "setting {0} is a known registry key and cannot be deleted; set a new value to override it"
    )]
    KnownKeyNotDeletable(String),
    #[error("the redacted marker is never accepted as a value for setting {0}")]
    RedactedMarker(String),
    #[error("invalid configuration document: {0}")]
    InvalidDocument(&'static str),
    #[error("configuration file error: {0}")]
    ConfigFile(String),
    #[error("invalid onboarding completion: {0}")]
    InvalidOnboarding(&'static str),
}

fn validate_key(key: &str) -> Result<(), SettingsError> {
    if key.trim().is_empty() {
        return Err(SettingsError::InvalidKey("key must be non-blank"));
    }
    if key.chars().count() > MAX_SETTING_KEY_CHARS {
        return Err(SettingsError::InvalidKey("key exceeds 255 characters"));
    }
    Ok(())
}

fn validate_value(key: &str, value: &serde_json::Value) -> Result<(), SettingsError> {
    if let serde_json::Value::String(candidate) = value
        && candidate == SECRET_REDACTION_MARKER
    {
        // Fail closed: a redacted export must never be able to overwrite a
        // stored value (PRD-AUTH-007).
        return Err(SettingsError::RedactedMarker(key.to_string()));
    }
    let serialized = value.to_string();
    if serialized.len() > MAX_SETTING_VALUE_BYTES {
        return Err(SettingsError::InvalidValue {
            key: key.to_string(),
            reason: format!("value exceeds {MAX_SETTING_VALUE_BYTES} serialized bytes"),
        });
    }
    if let Some(known) = known_setting(key) {
        known
            .kind
            .validate(value)
            .map_err(|reason| SettingsError::InvalidValue {
                key: key.to_string(),
                reason,
            })?;
    }
    Ok(())
}

/// Stateless entry point for `server_settings` administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsStore;

impl SettingsStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Fetches one setting with secret values redacted.
    pub async fn get<'e, E>(
        &self,
        executor: E,
        key: &str,
    ) -> Result<Option<ServerSetting>, SettingsError>
    where
        E: PgExecutor<'e>,
    {
        let row = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value, is_secret, updated_at FROM server_settings WHERE key = $1",
        )
        .bind(key)
        .fetch_optional(executor)
        .await?;
        Ok(row.map(ServerSetting::from))
    }

    /// Bounded, deterministic listing (by key) with secret values redacted.
    pub async fn list(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ServerSetting>, SettingsError> {
        let rows = sqlx::query_as::<_, SettingRow>(
            "SELECT key, value, is_secret, updated_at FROM server_settings \
             ORDER BY key LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit.min(MAX_SETTINGS_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(ServerSetting::from).collect())
    }

    /// Reads the raw stored value and secret flag. INTERNAL USE ONLY: the
    /// resolver and import machinery call this; the returned value must
    /// never be logged or serialized when `is_secret` is true.
    pub async fn read_raw_value<'e, E>(
        &self,
        executor: E,
        key: &str,
    ) -> Result<Option<(serde_json::Value, bool)>, SettingsError>
    where
        E: PgExecutor<'e>,
    {
        let row = sqlx::query_as::<_, (serde_json::Value, bool)>(
            "SELECT value, is_secret FROM server_settings WHERE key = $1",
        )
        .bind(key)
        .fetch_optional(executor)
        .await?;
        Ok(row)
    }

    /// All stored rows keyed by setting key. INTERNAL USE ONLY (same
    /// secrecy constraints as [`SettingsStore::read_raw_value`]).
    pub async fn list_raw(
        &self,
        pool: &PgPool,
    ) -> Result<BTreeMap<String, (serde_json::Value, bool)>, SettingsError> {
        let rows = sqlx::query_as::<_, (String, serde_json::Value, bool)>(
            "SELECT key, value, is_secret FROM server_settings ORDER BY key LIMIT $1",
        )
        .bind(i64::from(MAX_SETTINGS_LIST))
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(key, value, is_secret)| (key, (value, is_secret)))
            .collect())
    }

    /// Creates or replaces one setting. Known keys validate against their
    /// inventory kind; known secret keys are always stored with
    /// `is_secret = true` regardless of the caller's flag. `is_secret` of
    /// `None` means "not requested" (known classification wins; custom keys
    /// default to non-secret).
    pub async fn set<'e, E>(
        &self,
        executor: E,
        key: &str,
        value: &serde_json::Value,
        is_secret: Option<bool>,
        now: OffsetDateTime,
    ) -> Result<ServerSetting, SettingsError>
    where
        E: PgExecutor<'e>,
    {
        validate_key(key)?;
        validate_value(key, value)?;
        let effective_secret = is_secret_key(key) || is_secret.unwrap_or(false);
        let row = sqlx::query_as::<_, SettingRow>(
            "INSERT INTO server_settings (key, value, is_secret, updated_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (key) DO UPDATE SET \
               value = EXCLUDED.value, is_secret = EXCLUDED.is_secret, \
               updated_at = EXCLUDED.updated_at \
             RETURNING key, value, is_secret, updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(effective_secret)
        .bind(now)
        .fetch_one(executor)
        .await?;
        Ok(ServerSetting::from(row))
    }

    /// Deletes one custom (non-registry) setting. Known registry keys fail
    /// closed: removing them would silently revert reads to the built-in
    /// default, so operators override with a value instead.
    pub async fn delete(&self, pool: &PgPool, key: &str) -> Result<(), SettingsError> {
        if known_setting(key).is_some() {
            return Err(SettingsError::KnownKeyNotDeletable(key.to_string()));
        }
        let result = sqlx::query("DELETE FROM server_settings WHERE key = $1")
            .bind(key)
            .execute(pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(SettingsError::InvalidKey("setting not found"));
        }
        Ok(())
    }
}

/// The singleton `onboarding_state` row. An absent row means "not completed,
/// no evidence yet"; a row with `completed_at = NULL` means setup evidence
/// (configuration imports) is accumulating before completion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnboardingState {
    pub completed_at: Option<OffsetDateTime>,
    pub completed_by: Option<String>,
    pub evidence: Option<serde_json::Value>,
}

impl OnboardingState {
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }
}

type OnboardingRow = (
    Option<OffsetDateTime>,
    Option<String>,
    Option<serde_json::Value>,
);

impl From<OnboardingRow> for OnboardingState {
    fn from(row: OnboardingRow) -> Self {
        let (completed_at, completed_by, evidence) = row;
        Self {
            completed_at,
            completed_by,
            evidence,
        }
    }
}

/// Stateless entry point for the singleton onboarding state.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnboardingStore;

impl OnboardingStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The current onboarding state; the absent-row case maps to the
    /// default (not completed, no evidence).
    pub async fn get_state(&self, pool: &PgPool) -> Result<OnboardingState, SettingsError> {
        let row = sqlx::query_as::<_, OnboardingRow>(
            "SELECT completed_at, completed_by, evidence FROM onboarding_state WHERE id = true",
        )
        .fetch_optional(pool)
        .await?;
        Ok(row.map(OnboardingState::from).unwrap_or_default())
    }

    pub async fn is_completed(&self, pool: &PgPool) -> Result<bool, SettingsError> {
        Ok(self.get_state(pool).await?.is_completed())
    }

    /// Marks onboarding complete (idempotent re-completion replaces the
    /// marker and evidence). `evidence` must be a bounded JSON object —
    /// preflight check results and never secret material.
    pub async fn complete(
        &self,
        pool: &PgPool,
        completed_by: &str,
        evidence: serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<OnboardingState, SettingsError> {
        if completed_by.trim().is_empty() || completed_by.chars().count() > MAX_SETTING_KEY_CHARS {
            return Err(SettingsError::InvalidOnboarding(
                "completed_by must be non-blank and at most 255 characters",
            ));
        }
        if !evidence.is_object() || evidence.to_string().len() > MAX_SETTING_VALUE_BYTES {
            return Err(SettingsError::InvalidOnboarding(
                "evidence must be a JSON object of at most 65536 serialized bytes",
            ));
        }
        let row = sqlx::query_as::<_, OnboardingRow>(
            "INSERT INTO onboarding_state (id, completed_at, completed_by, evidence) \
             VALUES (true, $1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET \
               completed_at = EXCLUDED.completed_at, \
               completed_by = EXCLUDED.completed_by, \
               evidence = EXCLUDED.evidence \
             RETURNING completed_at, completed_by, evidence",
        )
        .bind(now)
        .bind(completed_by)
        .bind(&evidence)
        .fetch_one(pool)
        .await?;
        Ok(OnboardingState::from(row))
    }

    /// Appends one bounded configuration-import audit entry to the
    /// singleton's `evidence.configImports` array (oldest entries are
    /// trimmed past [`MAX_IMPORT_AUDIT_ENTRIES`]). The entry must contain
    /// counts and metadata only — never setting values.
    pub async fn record_config_import(
        &self,
        pool: &PgPool,
        entry: serde_json::Value,
    ) -> Result<(), SettingsError> {
        let mut transaction = pool.begin().await?;
        self.record_config_import_tx(&mut transaction, entry)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Transactional variant so configuration imports commit their rows and
    /// their audit entry atomically.
    async fn record_config_import_tx(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entry: serde_json::Value,
    ) -> Result<(), SettingsError> {
        let row = sqlx::query_as::<_, (Option<serde_json::Value>,)>(
            "SELECT evidence FROM onboarding_state WHERE id = true FOR UPDATE",
        )
        .fetch_optional(&mut **transaction)
        .await?;
        let mut evidence = row
            .and_then(|(evidence,)| evidence)
            .unwrap_or_else(|| serde_json::json!({}));
        if !evidence.is_object() {
            return Err(SettingsError::InvalidOnboarding(
                "stored onboarding evidence is not a JSON object",
            ));
        }
        let mut entries = evidence
            .get("configImports")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        entries.push(entry);
        if entries.len() > MAX_IMPORT_AUDIT_ENTRIES {
            let overflow = entries.len() - MAX_IMPORT_AUDIT_ENTRIES;
            entries.drain(..overflow);
        }
        evidence["configImports"] = serde_json::Value::Array(entries);
        if evidence.to_string().len() > MAX_SETTING_VALUE_BYTES {
            return Err(SettingsError::InvalidOnboarding(
                "onboarding evidence exceeds the 65536-byte bound",
            ));
        }
        sqlx::query(
            "INSERT INTO onboarding_state (id, evidence) VALUES (true, $1) \
             ON CONFLICT (id) DO UPDATE SET evidence = EXCLUDED.evidence",
        )
        .bind(&evidence)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }
}

/// Which layer supplied an effective setting value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingSource {
    Environment,
    ConfigFile,
    Database,
    Default,
}

impl SettingSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::ConfigFile => "config-file",
            Self::Database => "database",
            Self::Default => "default",
        }
    }
}

/// One effective setting value with its origin layer. The `value` of a
/// secret setting is the REAL value (internal use); presentation surfaces
/// must redact via [`ResolvedSetting::to_redacted_json`].
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSetting {
    pub key: String,
    pub value: Option<serde_json::Value>,
    pub source: SettingSource,
    pub is_secret: bool,
}

impl ResolvedSetting {
    /// Presentation-safe JSON: the marker for secrets with a stored/effective
    /// value, the value otherwise.
    #[must_use]
    pub fn to_redacted_json(&self) -> serde_json::Value {
        match (&self.value, self.is_secret) {
            (Some(_), true) => serde_json::Value::String(SECRET_REDACTION_MARKER.to_string()),
            (Some(value), false) => value.clone(),
            (None, _) => serde_json::Value::Null,
        }
    }
}

/// Layered settings resolution (see module docs for precedence). Borrows
/// the pool; the optional config-file layer is loaded at construction. The
/// environment layer is read through `env_reader` — the process environment
/// by default, injectable for tests and embedded runners (environment
/// mutation is `unsafe` under edition 2024, so tests inject instead).
pub struct SettingsResolver<'a> {
    pool: &'a PgPool,
    file_values: BTreeMap<String, serde_json::Value>,
    env_reader: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
}

/// Default environment layer: non-empty process environment values.
fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

impl<'a> SettingsResolver<'a> {
    /// Resolver without a config-file layer, reading the process
    /// environment.
    #[must_use]
    pub const fn new(pool: &'a PgPool) -> Self {
        Self {
            pool,
            file_values: BTreeMap::new(),
            env_reader: &process_env,
        }
    }

    /// Resolver with an explicit environment source (test/embedding seam).
    #[must_use]
    pub const fn with_env_reader(
        pool: &'a PgPool,
        env_reader: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    ) -> Self {
        Self {
            pool,
            file_values: BTreeMap::new(),
            env_reader,
        }
    }

    /// Resolver honoring `MELODEE__CONFIG_FILE` when set: a bounded (1 MiB)
    /// JSON object of `{"key": value}` overrides. A set-but-unreadable or
    /// malformed file fails closed.
    pub fn from_env(pool: &'a PgPool) -> Result<Self, SettingsError> {
        let Some(path) = std::env::var_os(ENV_CONFIG_FILE) else {
            return Ok(Self::new(pool));
        };
        Self::with_config_file(pool, std::path::Path::new(&path), &process_env)
    }

    /// Resolver with an explicit config-file layer.
    pub fn with_config_file(
        pool: &'a PgPool,
        path: &std::path::Path,
        env_reader: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    ) -> Result<Self, SettingsError> {
        let raw = std::fs::read(path)
            .map_err(|error| SettingsError::ConfigFile(format!("{}: {error}", path.display())))?;
        if raw.len() > MAX_IMPORT_DOCUMENT_BYTES {
            return Err(SettingsError::ConfigFile(format!(
                "{}: exceeds {MAX_IMPORT_DOCUMENT_BYTES} bytes",
                path.display()
            )));
        }
        let document: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| {
            SettingsError::ConfigFile(format!("{}: not valid JSON", path.display()))
        })?;
        let object = document.as_object().ok_or_else(|| {
            SettingsError::ConfigFile(format!("{}: root must be a JSON object", path.display()))
        })?;
        if object.len() > MAX_IMPORT_KEYS {
            return Err(SettingsError::ConfigFile(format!(
                "{}: more than {MAX_IMPORT_KEYS} keys",
                path.display()
            )));
        }
        let mut file_values = BTreeMap::new();
        for (key, value) in object {
            validate_key(key)?;
            validate_value(key, value)?;
            file_values.insert(key.clone(), value.clone());
        }
        Ok(Self {
            pool,
            file_values,
            env_reader,
        })
    }

    /// Resolves one key through env → file → DB → default. Environment and
    /// file values that fail the key's kind validation are hard errors
    /// (fail closed; a misconfigured deployment must not silently fall
    /// through to a lower layer).
    pub async fn resolve(&self, key: &str) -> Result<ResolvedSetting, SettingsError> {
        let known = known_setting(key);
        let kind = known.map_or(SettingKind::Text, |known| known.kind);
        let env_name = env_var_name(key);
        if let Some(raw) = (self.env_reader)(&env_name) {
            let value = kind
                .parse_text(&raw)
                .ok_or_else(|| SettingsError::InvalidValue {
                    key: key.to_string(),
                    reason: format!(
                        "environment variable {env_name} does not match the setting type"
                    ),
                })?;
            return Ok(ResolvedSetting {
                key: key.to_string(),
                value: Some(value),
                source: SettingSource::Environment,
                is_secret: is_secret_key(key),
            });
        }
        if let Some(value) = self.file_values.get(key) {
            return Ok(ResolvedSetting {
                key: key.to_string(),
                value: Some(value.clone()),
                source: SettingSource::ConfigFile,
                is_secret: is_secret_key(key),
            });
        }
        if let Some((value, is_secret)) =
            SettingsStore::new().read_raw_value(self.pool, key).await?
        {
            return Ok(ResolvedSetting {
                key: key.to_string(),
                value: Some(value),
                source: SettingSource::Database,
                is_secret,
            });
        }
        Ok(ResolvedSetting {
            key: key.to_string(),
            value: known.and_then(KnownSetting::default_value),
            source: SettingSource::Default,
            is_secret: is_secret_key(key),
        })
    }

    /// Resolves every known key plus every custom DB row, sorted by key.
    pub async fn resolve_all(&self) -> Result<Vec<ResolvedSetting>, SettingsError> {
        let mut keys: Vec<String> = KNOWN_SETTINGS
            .iter()
            .map(|known| known.key.to_string())
            .collect();
        for key in SettingsStore::new().list_raw(self.pool).await?.keys() {
            if known_setting(key).is_none() {
                keys.push(key.clone());
            }
        }
        let mut resolved = Vec::with_capacity(keys.len());
        for key in keys {
            resolved.push(self.resolve(&key).await?);
        }
        resolved.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(resolved)
    }
}

/// The export/import document (PRD-CONFIG-002). Secret values are always
/// the fixed redaction marker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigExportDocument {
    pub format: String,
    pub warning: String,
    pub exported_at: String,
    pub settings: BTreeMap<String, serde_json::Value>,
}

/// Builds the export document from the **database + defaults layer**: every
/// known key with its stored value (or built-in default when unset) plus
/// every custom stored key. Environment/config-file overlays are deployment
/// concerns and are deliberately not captured — importing this document
/// writes DB rows, and env/file values keep winning over them at runtime
/// (see module docs). Secret values are always the redaction marker; secret
/// keys appear only when a row was actually stored (an operator set one).
pub async fn export_config(
    pool: &PgPool,
    now: OffsetDateTime,
) -> Result<ConfigExportDocument, SettingsError> {
    let stored = SettingsStore::new().list_raw(pool).await?;
    let mut settings = BTreeMap::new();
    for known in KNOWN_SETTINGS {
        match (stored.get(known.key), known.secret) {
            (Some(_), true) => {
                settings.insert(
                    known.key.to_string(),
                    serde_json::Value::String(SECRET_REDACTION_MARKER.to_string()),
                );
            }
            (Some((value, false)), false) => {
                settings.insert(known.key.to_string(), value.clone());
            }
            (None, false) => {
                if let Some(value) = known.default_value() {
                    settings.insert(known.key.to_string(), value);
                }
            }
            // A secret key with no stored row exports nothing (its default
            // is empty or deliberately absent).
            _ => {}
        }
    }
    for (key, (value, is_secret)) in &stored {
        if known_setting(key).is_none() {
            settings.insert(
                key.clone(),
                if *is_secret {
                    serde_json::Value::String(SECRET_REDACTION_MARKER.to_string())
                } else {
                    value.clone()
                },
            );
        }
    }
    Ok(ConfigExportDocument {
        format: CONFIG_EXPORT_FORMAT.to_string(),
        warning: CONFIG_EXPORT_WARNING.to_string(),
        exported_at: now
            .format(&Rfc3339)
            .map_err(|_| SettingsError::InvalidDocument("timestamp formatting failed"))?,
        settings,
    })
}

/// The import diff. Secret keys appear BY NAME ONLY (PRD-AUTH-007): values
/// never leave the document/DB boundary in either direction.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigImportPlan {
    /// Known keys with no stored row whose value differs from the default.
    pub added: Vec<String>,
    /// Known keys whose stored value would change.
    pub changed: Vec<String>,
    /// Known keys whose effective DB-layer value already matches.
    pub unchanged: Vec<String>,
    /// Secret keys with a real (non-marker) value to store.
    pub secrets_reentered: Vec<String>,
    /// Secret keys present as the redaction marker: skipped on apply and
    /// must be re-entered by the operator afterwards.
    pub secrets_skipped: Vec<String>,
    /// Keys outside the 190-key registry.
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

/// Parses and validates an import document, producing the preview plan.
/// Fail-closed: the redaction marker for any non-secret key, a kind
/// violation, or a malformed document is an error, not a warning.
pub fn plan_config_import(
    document: &serde_json::Value,
    stored: &BTreeMap<String, (serde_json::Value, bool)>,
) -> Result<ConfigImportPlan, SettingsError> {
    let format = document
        .get("format")
        .and_then(serde_json::Value::as_str)
        .ok_or(SettingsError::InvalidDocument("missing 'format'"))?;
    if format != CONFIG_EXPORT_FORMAT {
        return Err(SettingsError::InvalidDocument("unsupported 'format'"));
    }
    let settings = document
        .get("settings")
        .and_then(serde_json::Value::as_object)
        .ok_or(SettingsError::InvalidDocument("missing 'settings' object"))?;
    if settings.len() > MAX_IMPORT_KEYS {
        return Err(SettingsError::InvalidDocument("too many settings"));
    }
    let mut plan = ConfigImportPlan::default();
    for (key, value) in settings {
        validate_key(key)?;
        let Some(known) = known_setting(key) else {
            plan.unknown.push(key.clone());
            continue;
        };
        if let serde_json::Value::String(candidate) = value
            && candidate == SECRET_REDACTION_MARKER
        {
            if known.secret {
                plan.secrets_skipped.push(key.clone());
            } else {
                return Err(SettingsError::RedactedMarker(key.clone()));
            }
            continue;
        }
        known
            .kind
            .validate(value)
            .map_err(|reason| SettingsError::InvalidValue {
                key: key.clone(),
                reason,
            })?;
        if known.secret {
            plan.secrets_reentered.push(key.clone());
            // Re-entered secrets are stored; classify them against the
            // current stored row like any other write below.
        }
        let current = stored
            .get(key)
            .map(|(value, _)| Some(value.clone()))
            .unwrap_or_else(|| known.default_value());
        match (stored.contains_key(key), current.as_ref() == Some(value)) {
            (_, true) => plan.unchanged.push(key.clone()),
            (false, false) => plan.added.push(key.clone()),
            (true, false) => plan.changed.push(key.clone()),
        }
    }
    for list in [
        &mut plan.added,
        &mut plan.changed,
        &mut plan.unchanged,
        &mut plan.secrets_reentered,
        &mut plan.secrets_skipped,
        &mut plan.unknown,
    ] {
        list.sort();
        list.dedup();
    }
    if !plan.secrets_skipped.is_empty() {
        plan.warnings.push(format!(
            "secret settings were redacted in this export and are NOT imported: {}; \
             re-enter them explicitly afterwards",
            plan.secrets_skipped.join(", ")
        ));
    }
    if !plan.unknown.is_empty() {
        plan.warnings.push(format!(
            "unknown settings are rejected on apply unless explicitly allowed: {}",
            plan.unknown.join(", ")
        ));
    }
    Ok(plan)
}

/// Applies a validated import document in one transaction and records the
/// import in the onboarding evidence (counts and metadata only, never
/// values). Unknown keys are rejected unless `allow_unknown`; admitted
/// unknown keys are stored as custom rows (bounded and marker-checked like
/// any other write).
pub async fn apply_config_import(
    pool: &PgPool,
    document: &serde_json::Value,
    plan: &ConfigImportPlan,
    allow_unknown: bool,
    actor: &str,
    now: OffsetDateTime,
) -> Result<(), SettingsError> {
    if !plan.unknown.is_empty() && !allow_unknown {
        return Err(SettingsError::InvalidDocument(
            "unknown settings rejected; pass allow-unknown to admit them",
        ));
    }
    let settings = document
        .get("settings")
        .and_then(serde_json::Value::as_object)
        .ok_or(SettingsError::InvalidDocument("missing 'settings' object"))?;
    let store = SettingsStore::new();
    let mut transaction = pool.begin().await?;
    let mut writable: Vec<&String> = plan
        .added
        .iter()
        .chain(plan.changed.iter())
        .chain(plan.secrets_reentered.iter())
        .collect();
    if allow_unknown {
        writable.extend(plan.unknown.iter());
    }
    writable.sort();
    writable.dedup();
    for key in writable {
        let value = settings.get(key).ok_or(SettingsError::InvalidDocument(
            "planned key missing from document",
        ))?;
        // Re-validate at write time: the store is the fail-closed backstop
        // even if plan and document drifted apart.
        store.set(&mut *transaction, key, value, None, now).await?;
    }
    OnboardingStore::new()
        .record_config_import_tx(
            &mut transaction,
            serde_json::json!({
                "importedAt": now
                    .format(&Rfc3339)
                    .map_err(|_| SettingsError::InvalidDocument("timestamp formatting failed"))?,
                "actor": actor,
                "added": plan.added.len(),
                "changed": plan.changed.len(),
                "unchanged": plan.unchanged.len(),
                "secretsReentered": plan.secrets_reentered.len(),
                "secretsSkipped": plan.secrets_skipped.len(),
                "unknown": plan.unknown.len(),
                "unknownAdmitted": allow_unknown && !plan.unknown.is_empty(),
            }),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn known_settings_table_is_sorted_complete_and_consistent() {
        // 190-key legacy inventory + `transcoding.maxConcurrent`, the first
        // non-legacy operator setting, added under ADR-0147 decision 5.
        assert_eq!(
            KNOWN_SETTINGS.len(),
            191,
            "190-key legacy inventory + ADR-0147"
        );
        for pair in KNOWN_SETTINGS.windows(2) {
            assert!(
                pair[0].key < pair[1].key,
                "table must be sorted: {:?} then {:?}",
                pair[0].key,
                pair[1].key
            );
        }
        for known in KNOWN_SETTINGS {
            assert!(!known.key.trim().is_empty());
            assert!(known.key.chars().count() <= MAX_SETTING_KEY_CHARS);
            // Every recorded default must satisfy its own kind.
            if let Some(default) = known.default {
                assert!(
                    known.kind.parse_text(default).is_some(),
                    "default for {} does not parse as its kind",
                    known.key
                );
            }
        }
        // The inventory's 12 hard secrets + 2 credential-adjacent keys.
        let secrets: Vec<&str> = KNOWN_SETTINGS
            .iter()
            .filter(|known| known.secret)
            .map(|known| known.key)
            .collect();
        assert_eq!(secrets.len(), 14, "{secrets:?}");
        for expected in [
            "encryption.privateKey",
            "security.secretKey",
            "jellyfin.token.pepper",
            "email.smtpPassword",
            "email.smtpUsername",
            "register.privateCode",
            "mpd.password",
            "scrobbling.lastFm.apiKey",
            "scrobbling.lastFm.sharedSecret",
            "searchEngine.spotify.apiKey",
            "searchEngine.spotify.sharedSecret",
            "searchEngine.spotify.accessToken",
            "searchEngine.discogs.userToken",
            "searchEngine.brave.apiKey",
        ] {
            assert!(secrets.contains(&expected), "{expected}");
        }
    }

    #[test]
    fn env_mapping_uses_explicit_conventions_then_mechanical_names() {
        assert_eq!(
            env_var_name("email.smtpPassword"),
            "MELODEE__EMAIL__SMTP_PASSWORD"
        );
        assert_eq!(
            env_var_name("jellyfin.token.pepper"),
            "MELODEE__JELLYFIN__TOKEN_PEPPER"
        );
        assert_eq!(
            env_var_name("processing.doDeleteOriginal"),
            "MELODEE__PROCESSING__DODELETEORIGINAL"
        );
        assert_eq!(
            env_var_name("custom.unknownKey"),
            "MELODEE__CUSTOM__UNKNOWNKEY"
        );
    }

    #[test]
    fn kind_parsing_and_validation_are_fail_closed() {
        let int = SettingKind::Int {
            min: 250,
            max: Some(1000),
        };
        assert_eq!(int.parse_text("500"), Some(serde_json::json!(500)));
        assert_eq!(int.parse_text("249"), None);
        assert_eq!(int.parse_text("1001"), None);
        assert!(int.validate(&serde_json::json!(250)).is_ok());
        assert!(int.validate(&serde_json::json!(1001)).is_err());
        assert!(int.validate(&serde_json::json!("500")).is_err());

        assert_eq!(
            SettingKind::Bool.parse_text("yes"),
            Some(serde_json::json!(true))
        );
        assert!(
            SettingKind::Bool
                .validate(&serde_json::json!("true"))
                .is_err()
        );

        let float = SettingKind::Float { min: 0.0, max: 1.0 };
        assert!(float.validate(&serde_json::json!(0.8)).is_ok());
        assert!(float.validate(&serde_json::json!(1.1)).is_err());

        let enum_kind = SettingKind::Enum(&["", "mpv", "mpd"]);
        assert!(enum_kind.validate(&serde_json::json!("mpv")).is_ok());
        assert!(enum_kind.validate(&serde_json::json!("vlc")).is_err());

        assert!(SettingKind::Text.validate(&serde_json::json!("x")).is_ok());
        assert!(SettingKind::Text.validate(&serde_json::json!(1)).is_err());
    }

    #[test]
    fn defaults_that_shipped_compromised_or_placeholder_values_are_not_carried() {
        for key in [
            "encryption.privateKey",
            "jellyfin.token.pepper",
            "searchEngine.musicbrainz.storagePath",
            "system.baseUrl",
        ] {
            let known = known_setting(key).expect("known key");
            assert!(known.default.is_none(), "{key} must not carry a default");
        }
    }

    #[test]
    fn import_plan_diffs_and_never_accepts_the_marker_for_non_secrets() {
        let stored = BTreeMap::from([
            (
                "magic.enabled".to_string(),
                (serde_json::json!(false), false),
            ),
            (
                "email.smtpPassword".to_string(),
                (serde_json::json!("stored-secret"), true),
            ),
        ]);
        let document = serde_json::json!({
            "format": CONFIG_EXPORT_FORMAT,
            "warning": CONFIG_EXPORT_WARNING,
            "exportedAt": "2026-08-14T00:00:00Z",
            "settings": {
                "magic.enabled": true,
                "conversion.enabled": true,
                "conversion.bitrate": 500,
                "email.smtpPassword": SECRET_REDACTION_MARKER,
                "scrobbling.lastFm.apiKey": "reentered-secret",
                "totally.unknown": "x"
            }
        });
        let plan = plan_config_import(&document, &stored).expect("plan");
        assert_eq!(plan.changed, vec!["magic.enabled"]);
        assert_eq!(
            plan.added,
            vec!["conversion.bitrate", "scrobbling.lastFm.apiKey"]
        );
        // Equal to the built-in default with no stored row: unchanged.
        assert_eq!(plan.unchanged, vec!["conversion.enabled"]);
        assert_eq!(plan.secrets_skipped, vec!["email.smtpPassword"]);
        assert_eq!(plan.secrets_reentered, vec!["scrobbling.lastFm.apiKey"]);
        assert_eq!(plan.unknown, vec!["totally.unknown"]);
        assert_eq!(plan.warnings.len(), 2);

        // The marker for a non-secret key is a hard error (fail closed).
        let bad = serde_json::json!({
            "format": CONFIG_EXPORT_FORMAT,
            "settings": { "magic.enabled": SECRET_REDACTION_MARKER }
        });
        assert!(matches!(
            plan_config_import(&bad, &stored),
            Err(SettingsError::RedactedMarker(key)) if key == "magic.enabled"
        ));

        // Kind violations are hard errors naming the key, not the value.
        let bad_type = serde_json::json!({
            "format": CONFIG_EXPORT_FORMAT,
            "settings": { "defaults.batchSize": 100 }
        });
        assert!(matches!(
            plan_config_import(&bad_type, &stored),
            Err(SettingsError::InvalidValue { key, .. }) if key == "defaults.batchSize"
        ));

        // Unsupported documents fail closed.
        assert!(matches!(
            plan_config_import(&serde_json::json!({"settings": {}}), &stored),
            Err(SettingsError::InvalidDocument(_))
        ));
    }
}
