//! Configuration-driven request security policy (legacy `BlacklistService`
//! parity, `security.blacklistedEmails` / `security.blacklistedIPs`).
//!
//! The C# server consults these two settings on every Melodee API request and
//! on the authentication endpoints, returning the frozen `403 BLACKLISTED`
//! envelope. The Rust replacement previously only honored the per-user
//! `blacklisted` column, so an operator who denied a source IP or an email in
//! configuration saw no effect after cutover.
//!
//! The policy is resolved through the same layered settings used everywhere
//! else (`environment` > `config-file` > `database` > `registered default`),
//! cached for [`CACHE_TTL`] so the check can run on the authenticated request
//! path without a database round trip per request. A failed refresh keeps the
//! last successfully loaded policy: a transient database error must not
//! silently disable an operator's deny list. The very first load is empty and
//! is retried on the next check (with a warning), which matches the legacy
//! "no configuration means no exclusions" behavior.
//!
//! Values are comparison data, never identities: nothing here participates in
//! catalog identity, and the raw lists are never logged or returned by the
//! diagnostics API (counts only).

use std::{
    sync::RwLock,
    time::{Duration, Instant},
};

use melodee_persistence::{ResolvedSetting, SettingsResolver};
use serde::Serialize;

pub const SETTING_BLACKLISTED_EMAILS: &str = "security.blacklistedEmails";
pub const SETTING_BLACKLISTED_IPS: &str = "security.blacklistedIPs";

/// How long a resolved deny list is reused before the database/file layers
/// are consulted again. Short enough that an operator's change takes effect
/// without a restart, long enough that per-request checks stay free.
pub const CACHE_TTL: Duration = Duration::from_secs(10);

/// Parsed deny lists. Emails are lower-cased and trimmed; IPs are trimmed.
/// Hostnames/ports are not accepted for IP entries (the value must match the
/// trusted-proxy-resolved client address exactly, as in the legacy filter).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlacklistPolicy {
    pub emails: Vec<String>,
    pub ips: Vec<String>,
}

impl BlacklistPolicy {
    #[must_use]
    pub fn from_values(emails: Option<&ResolvedSetting>, ips: Option<&ResolvedSetting>) -> Self {
        Self {
            emails: emails.map_or_else(Vec::new, parse_list),
            ips: ips.map_or_else(Vec::new, parse_list),
        }
    }

    #[must_use]
    pub fn is_email_blacklisted(&self, email: &str) -> bool {
        let normalized = email.trim().to_lowercase();
        !normalized.is_empty() && self.emails.iter().any(|entry| entry == &normalized)
    }

    #[must_use]
    pub fn is_ip_blacklisted(&self, ip: &str) -> bool {
        let normalized = ip.trim();
        !normalized.is_empty() && self.ips.iter().any(|entry| entry == normalized)
    }
}

/// One deny-list entry array. Legacy stores a JSON array of strings
/// (`["a@b.test"]`); a plain comma/newline separated value is also accepted so
/// a hand-edited environment variable cannot silently produce an empty list.
/// Individual entries are normalized the same way for both shapes.
fn parse_list(setting: &ResolvedSetting) -> Vec<String> {
    let Some(value) = setting.value.as_ref() else {
        return Vec::new();
    };
    let raw_entries: Vec<String> = match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        serde_json::Value::String(text) => {
            if text.trim_start().starts_with('[') {
                match serde_json::from_str::<Vec<String>>(text.as_str()) {
                    Ok(items) => items,
                    Err(error) => {
                        tracing::warn!(
                            key = setting.key,
                            %error,
                            "security list value is not a JSON string array; treating it as empty"
                        );
                        Vec::new()
                    }
                }
            } else {
                text.split([',', '\n'])
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect()
            }
        }
        other => {
            tracing::warn!(
                key = setting.key,
                kind = ?other,
                "security list value has an unsupported type; treating it as empty"
            );
            Vec::new()
        }
    };
    // Email entries are case-insensitive (legacy normalizes to lower case);
    // IP entries are byte-exact after trimming. Applying `to_lowercase` to IPs
    // is harmless for both IPv4 and IPv6 hex text and keeps one code path.
    let mut normalized: Vec<String> = raw_entries
        .into_iter()
        .map(|entry| entry.trim().to_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlacklistReason {
    Email,
    Ip,
}

impl BlacklistReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Ip => "ip",
        }
    }
}

#[derive(Debug, Default)]
struct CachedPolicy {
    policy: BlacklistPolicy,
    loaded_at: Option<Instant>,
}

/// Shared deny-list cache. Cheap to clone (`Arc` inside the runtime state).
#[derive(Debug)]
pub struct BlacklistCache {
    pool: sqlx::PgPool,
    ttl: Duration,
    state: RwLock<CachedPolicy>,
}

/// Diagnostics view of the cache. Counts and freshness only: the entries
/// themselves are operator data and are never exposed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlacklistStatus {
    pub blacklisted_email_count: usize,
    pub blacklisted_ip_count: usize,
    pub age_seconds: Option<u64>,
}

impl BlacklistCache {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            ttl: CACHE_TTL,
            state: RwLock::new(CachedPolicy::default()),
        }
    }

    /// Cache with an explicit TTL (tests).
    #[must_use]
    pub fn with_ttl(pool: sqlx::PgPool, ttl: Duration) -> Self {
        Self {
            pool,
            ttl,
            state: RwLock::new(CachedPolicy::default()),
        }
    }

    async fn current(&self) -> BlacklistPolicy {
        let needs_refresh = {
            let guard = self.state.read().unwrap_or_else(|poisoned| {
                // A poisoned lock only means another thread panicked while
                // holding it; the cached policy is still a plain value.
                poisoned.into_inner()
            });
            guard
                .loaded_at
                .is_none_or(|loaded_at| loaded_at.elapsed() >= self.ttl)
        };
        if needs_refresh {
            let resolver = SettingsResolver::new(&self.pool);
            let loaded = async {
                let emails = resolver.resolve(SETTING_BLACKLISTED_EMAILS).await?;
                let ips = resolver.resolve(SETTING_BLACKLISTED_IPS).await?;
                Ok::<BlacklistPolicy, melodee_persistence::SettingsError>(
                    BlacklistPolicy::from_values(Some(&emails), Some(&ips)),
                )
            }
            .await;
            let mut guard = self
                .state
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match loaded {
                Ok(policy) => {
                    guard.policy = policy;
                    guard.loaded_at = Some(Instant::now());
                }
                Err(error) => {
                    // Keep the previous policy: failing closed to "no list"
                    // would unfilter traffic because of a transient outage.
                    tracing::warn!(
                        %error,
                        "unable to refresh security blacklist settings; retaining the previous policy"
                    );
                    // Avoid a hot retry loop on a persistent failure.
                    guard.loaded_at = Some(Instant::now());
                }
            }
        }
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .policy
            .clone()
    }

    pub async fn is_email_blacklisted(&self, email: &str) -> bool {
        self.current().await.is_email_blacklisted(email)
    }

    /// Which deny-list rule matched, if any. Used for bounded audit logging
    /// (the rule name only — never the entry).
    pub async fn evaluate(&self, email: &str, ip: &str) -> Option<BlacklistReason> {
        let policy = self.current().await;
        if policy.is_email_blacklisted(email) {
            Some(BlacklistReason::Email)
        } else if policy.is_ip_blacklisted(ip) {
            Some(BlacklistReason::Ip)
        } else {
            None
        }
    }

    pub async fn is_blacklisted(&self, email: &str, ip: &str) -> bool {
        let policy = self.current().await;
        policy.is_email_blacklisted(email) || policy.is_ip_blacklisted(ip)
    }

    pub async fn status(&self) -> BlacklistStatus {
        let policy = self.current().await;
        let age_seconds = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .loaded_at
            .map(|loaded_at| loaded_at.elapsed().as_secs());
        BlacklistStatus {
            blacklisted_email_count: policy.emails.len(),
            blacklisted_ip_count: policy.ips.len(),
            age_seconds,
        }
    }
}

/// The frozen native `403 BLACKLISTED` envelope (legacy `ApiBlacklisted()`).
#[must_use]
pub fn forbidden_blacklisted() -> axum::response::Response {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "code": "BLACKLISTED",
            "message": "User is blacklisted",
            "correlationId": crate::correlation::envelope_id(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use melodee_persistence::SettingSource;

    fn setting(key: &str, value: serde_json::Value) -> ResolvedSetting {
        ResolvedSetting {
            key: key.to_string(),
            value: Some(value),
            source: SettingSource::Database,
            is_secret: false,
        }
    }

    #[test]
    fn json_array_values_are_normalized_and_deduplicated() {
        let emails = setting(
            SETTING_BLACKLISTED_EMAILS,
            serde_json::json!([" Blocked@Example.test ", "blocked@example.test", ""]),
        );
        let ips = setting(SETTING_BLACKLISTED_IPS, serde_json::json!(["203.0.113.7"]));
        let policy = BlacklistPolicy::from_values(Some(&emails), Some(&ips));
        assert_eq!(policy.emails, vec!["blocked@example.test".to_string()]);
        assert_eq!(policy.ips, vec!["203.0.113.7".to_string()]);
        assert!(policy.is_email_blacklisted("BLOCKED@example.TEST"));
        assert!(policy.is_ip_blacklisted("203.0.113.7"));
        assert!(!policy.is_email_blacklisted("other@example.test"));
        assert!(!policy.is_ip_blacklisted("203.0.113.8"));
        assert!(!policy.is_ip_blacklisted(""));
    }

    #[test]
    fn json_encoded_text_and_plain_lists_are_accepted() {
        let encoded = setting(
            SETTING_BLACKLISTED_EMAILS,
            serde_json::Value::String(r#"["a@example.test"]"#.to_string()),
        );
        let plain = setting(
            SETTING_BLACKLISTED_IPS,
            serde_json::Value::String("198.51.100.1, 198.51.100.2\n".to_string()),
        );
        let policy = BlacklistPolicy::from_values(Some(&encoded), Some(&plain));
        assert_eq!(policy.emails, vec!["a@example.test".to_string()]);
        assert_eq!(
            policy.ips,
            vec!["198.51.100.1".to_string(), "198.51.100.2".to_string()]
        );
    }

    #[test]
    fn malformed_json_text_yields_an_empty_list_instead_of_a_wildcard() {
        let malformed = setting(
            SETTING_BLACKLISTED_EMAILS,
            serde_json::Value::String("[not-json".to_string()),
        );
        let policy = BlacklistPolicy::from_values(Some(&malformed), None);
        assert!(policy.emails.is_empty());
        assert!(policy.ips.is_empty());
    }

    #[test]
    fn absent_values_yield_an_empty_policy() {
        let policy = BlacklistPolicy::from_values(None, None);
        assert!(policy.emails.is_empty());
        assert!(policy.ips.is_empty());
        assert!(!policy.is_email_blacklisted("a@example.test"));
    }
}
