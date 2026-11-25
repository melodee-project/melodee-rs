//! Bounded catalog-import configuration.

use melodee_persistence::runtime_settings::var as config_var;

use thiserror::Error;
use time::Duration;

pub const ENV_CATALOG_ENABLED: &str = "MELODEE__CATALOG__ENABLED";
pub const ENV_CATALOG_CONCURRENCY: &str = "MELODEE__CATALOG__CONCURRENCY";
pub const ENV_CATALOG_BATCH_SIZE: &str = "MELODEE__CATALOG__BATCH_SIZE";
pub const ENV_CATALOG_POLL_INTERVAL_SECONDS: &str = "MELODEE__CATALOG__POLL_INTERVAL_SECONDS";

const DEFAULT_CATALOG_CONCURRENCY: u32 = 2;
const DEFAULT_CATALOG_BATCH_SIZE: u32 = 16;
const DEFAULT_CATALOG_POLL_INTERVAL_SECONDS: i64 = 5;

/// Runtime controls for the manifest-v3-to-catalog handoff.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CatalogConfig {
    pub enabled: bool,
    pub concurrency: u32,
    pub batch_size: u32,
    pub poll_interval: Duration,
}

impl CatalogConfig {
    /// Maximum work claimed per pass after applying the concurrency fence.
    #[must_use]
    pub const fn claim_limit(&self) -> u32 {
        if self.batch_size < self.concurrency {
            self.batch_size
        } else {
            self.concurrency
        }
    }

    /// Read catalog settings from their documented environment variables.
    pub fn from_env() -> Result<Self, CatalogConfigError> {
        let enabled = config_var(ENV_CATALOG_ENABLED)
            .ok()
            .map(|value| parse_bool(ENV_CATALOG_ENABLED, &value))
            .transpose()?
            .unwrap_or(true);
        let concurrency =
            parse_positive_u32_env(ENV_CATALOG_CONCURRENCY, DEFAULT_CATALOG_CONCURRENCY)?;
        let batch_size =
            parse_positive_u32_env(ENV_CATALOG_BATCH_SIZE, DEFAULT_CATALOG_BATCH_SIZE)?;
        let poll_interval_seconds = parse_positive_i64_env(
            ENV_CATALOG_POLL_INTERVAL_SECONDS,
            DEFAULT_CATALOG_POLL_INTERVAL_SECONDS,
        )?;

        Ok(Self {
            enabled,
            concurrency,
            batch_size,
            poll_interval: Duration::seconds(poll_interval_seconds),
        })
    }
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            concurrency: DEFAULT_CATALOG_CONCURRENCY,
            batch_size: DEFAULT_CATALOG_BATCH_SIZE,
            poll_interval: Duration::seconds(DEFAULT_CATALOG_POLL_INTERVAL_SECONDS),
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CatalogConfigError {
    #[error("{key} must be true or false, got '{value}'")]
    InvalidBoolean { key: &'static str, value: String },
    #[error("{key} must be a positive integer, got '{value}'")]
    InvalidPositiveInteger { key: &'static str, value: String },
}

fn parse_bool(key: &'static str, value: &str) -> Result<bool, CatalogConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(CatalogConfigError::InvalidBoolean {
            key,
            value: value.to_owned(),
        }),
    }
}

fn parse_positive_u32_env(key: &'static str, default: u32) -> Result<u32, CatalogConfigError> {
    config_var(key)
        .ok()
        .map(|value| parse_positive_u32(key, &value))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_positive_u32(key: &'static str, value: &str) -> Result<u32, CatalogConfigError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| CatalogConfigError::InvalidPositiveInteger {
            key,
            value: value.to_owned(),
        })
}

fn parse_positive_i64_env(key: &'static str, default: i64) -> Result<i64, CatalogConfigError> {
    config_var(key)
        .ok()
        .map(|value| parse_positive_i64(key, &value))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_positive_i64(key: &'static str, value: &str) -> Result<i64, CatalogConfigError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| CatalogConfigError::InvalidPositiveInteger {
            key,
            value: value.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_positive_and_bounded() {
        let config = CatalogConfig::default();

        assert!(config.enabled);
        assert_eq!(config.concurrency, 2);
        assert_eq!(config.batch_size, 16);
        assert_eq!(config.claim_limit(), 2);
        assert_eq!(config.poll_interval, Duration::seconds(5));
    }

    #[test]
    fn boolean_parser_accepts_operator_friendly_values() {
        assert_eq!(parse_bool(ENV_CATALOG_ENABLED, "yes"), Ok(true));
        assert_eq!(parse_bool(ENV_CATALOG_ENABLED, "OFF"), Ok(false));
        assert!(parse_bool(ENV_CATALOG_ENABLED, "maybe").is_err());
    }

    #[test]
    fn numeric_parser_rejects_zero_and_negative_values() {
        assert!(parse_positive_u32(ENV_CATALOG_CONCURRENCY, "0").is_err());
        assert!(parse_positive_i64(ENV_CATALOG_POLL_INTERVAL_SECONDS, "-1").is_err());
    }
}
