//! Explicit work-engine configuration.
//!
//! Only these keys are read from the environment. The legacy server's
//! indiscriminate absorption of every process environment variable is
//! deliberately NOT reproduced.

use crate::runtime_settings::var as config_var;
use melodee_domain::RetryPolicy;
use thiserror::Error;
use time::Duration;

/// Environment keys (the complete set this crate honors).
pub const ENV_DATABASE_URL: &str = "MELODEE__DATABASE__URL";
pub const ENV_DATABASE_MAX_CONNECTIONS: &str = "MELODEE__DATABASE__MAX_CONNECTIONS";
pub const ENV_DATABASE_MIN_CONNECTIONS: &str = "MELODEE__DATABASE__MIN_CONNECTIONS";
pub const ENV_DATABASE_ACQUIRE_TIMEOUT_SECONDS: &str = "MELODEE__DATABASE__ACQUIRE_TIMEOUT_SECONDS";
pub const ENV_DATABASE_IDLE_TIMEOUT_SECONDS: &str = "MELODEE__DATABASE__IDLE_TIMEOUT_SECONDS";
pub const ENV_LEASE_SECONDS: &str = "MELODEE__WORK__LEASE_SECONDS";
pub const ENV_RETRY_BASE_SECONDS: &str = "MELODEE__WORK__RETRY_BASE_SECONDS";
pub const ENV_RETRY_MAX_SECONDS: &str = "MELODEE__WORK__RETRY_MAX_SECONDS";
pub const ENV_MAX_ATTEMPTS: &str = "MELODEE__WORK__MAX_ATTEMPTS";
pub const ENV_CLAIM_BATCH_SIZE: &str = "MELODEE__WORK__CLAIM_BATCH_SIZE";

pub const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 32;
pub const DEFAULT_DATABASE_MIN_CONNECTIONS: u32 = 2;
pub const DEFAULT_DATABASE_ACQUIRE_TIMEOUT_SECONDS: u64 = 30;
pub const DEFAULT_DATABASE_IDLE_TIMEOUT_SECONDS: u64 = 600;
pub const MAX_DATABASE_CONNECTIONS_CEILING: u32 = 1024;

/// PostgreSQL connection pool configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabasePoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
}

impl Default for DatabasePoolConfig {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_DATABASE_MAX_CONNECTIONS,
            min_connections: DEFAULT_DATABASE_MIN_CONNECTIONS,
            acquire_timeout_seconds: DEFAULT_DATABASE_ACQUIRE_TIMEOUT_SECONDS,
            idle_timeout_seconds: DEFAULT_DATABASE_IDLE_TIMEOUT_SECONDS,
        }
    }
}

impl DatabasePoolConfig {
    /// Builds database pool configuration from environment keys.
    pub fn from_env() -> Result<Self, WorkConfigError> {
        let defaults = Self::default();
        let max_conns = parse_optional(ENV_DATABASE_MAX_CONNECTIONS)?;
        let min_conns = parse_optional(ENV_DATABASE_MIN_CONNECTIONS)?;
        let acquire_timeout = parse_optional(ENV_DATABASE_ACQUIRE_TIMEOUT_SECONDS)?;
        let idle_timeout = parse_optional(ENV_DATABASE_IDLE_TIMEOUT_SECONDS)?;

        let config = Self {
            max_connections: max_conns.unwrap_or(defaults.max_connections),
            min_connections: min_conns.unwrap_or(defaults.min_connections),
            acquire_timeout_seconds: acquire_timeout.unwrap_or(defaults.acquire_timeout_seconds),
            idle_timeout_seconds: idle_timeout.unwrap_or(defaults.idle_timeout_seconds),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), WorkConfigError> {
        if self.max_connections == 0 {
            return Err(WorkConfigError::NotPositive(ENV_DATABASE_MAX_CONNECTIONS));
        }
        if self.max_connections > MAX_DATABASE_CONNECTIONS_CEILING {
            return Err(WorkConfigError::ExceedsCeiling {
                key: ENV_DATABASE_MAX_CONNECTIONS,
                ceiling: MAX_DATABASE_CONNECTIONS_CEILING,
            });
        }
        if self.min_connections > self.max_connections {
            return Err(WorkConfigError::MinGreaterThanMax {
                min_key: ENV_DATABASE_MIN_CONNECTIONS,
                max_key: ENV_DATABASE_MAX_CONNECTIONS,
            });
        }
        if self.acquire_timeout_seconds == 0 {
            return Err(WorkConfigError::NotPositive(
                ENV_DATABASE_ACQUIRE_TIMEOUT_SECONDS,
            ));
        }
        if self.idle_timeout_seconds == 0 {
            return Err(WorkConfigError::NotPositive(
                ENV_DATABASE_IDLE_TIMEOUT_SECONDS,
            ));
        }
        Ok(())
    }

    /// Converts into SQLx [`sqlx::postgres::PgPoolOptions`].
    #[must_use]
    pub fn pool_options(&self) -> sqlx::postgres::PgPoolOptions {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(std::time::Duration::from_secs(self.acquire_timeout_seconds))
            .idle_timeout(std::time::Duration::from_secs(self.idle_timeout_seconds))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkEngineConfig {
    /// Lease duration for claimed work. Configuration-driven; no literal
    /// durations in engine code.
    pub lease_duration: Duration,
    pub retry_policy: RetryPolicy,
    /// Bounded claim batch (never an unbounded queue fetch).
    pub claim_batch_size: u32,
}

impl Default for WorkEngineConfig {
    fn default() -> Self {
        Self {
            lease_duration: Duration::seconds(300),
            retry_policy: RetryPolicy::default(),
            claim_batch_size: 4,
        }
    }
}

impl WorkEngineConfig {
    /// Builds a config from the explicit environment keys only.
    pub fn from_env() -> Result<Self, WorkConfigError> {
        let defaults = Self::default();
        let lease_seconds = parse_optional(ENV_LEASE_SECONDS)?;
        let retry_base = parse_optional(ENV_RETRY_BASE_SECONDS)?;
        let retry_max = parse_optional(ENV_RETRY_MAX_SECONDS)?;
        let max_attempts = parse_optional(ENV_MAX_ATTEMPTS)?;
        let batch = parse_optional(ENV_CLAIM_BATCH_SIZE)?;
        let config = Self {
            lease_duration: lease_seconds.map_or(defaults.lease_duration, Duration::seconds),
            retry_policy: RetryPolicy {
                base_delay: retry_base.map_or(defaults.retry_policy.base_delay, Duration::seconds),
                max_delay: retry_max.map_or(defaults.retry_policy.max_delay, Duration::seconds),
                max_attempts: max_attempts.unwrap_or(defaults.retry_policy.max_attempts),
            },
            claim_batch_size: batch.unwrap_or(defaults.claim_batch_size),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), WorkConfigError> {
        if !self.lease_duration.is_positive() {
            return Err(WorkConfigError::NotPositive(ENV_LEASE_SECONDS));
        }
        if !self.retry_policy.base_delay.is_positive() {
            return Err(WorkConfigError::NotPositive(ENV_RETRY_BASE_SECONDS));
        }
        if !self.retry_policy.max_delay.is_positive() {
            return Err(WorkConfigError::NotPositive(ENV_RETRY_MAX_SECONDS));
        }
        if self.retry_policy.max_delay < self.retry_policy.base_delay {
            return Err(WorkConfigError::RetryRange);
        }
        if self.retry_policy.max_attempts == 0 {
            return Err(WorkConfigError::NotPositive(ENV_MAX_ATTEMPTS));
        }
        if self.claim_batch_size == 0 {
            return Err(WorkConfigError::NotPositive(ENV_CLAIM_BATCH_SIZE));
        }
        Ok(())
    }
}

fn parse_optional<T>(key: &'static str) -> Result<Option<T>, WorkConfigError>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    config_var(key)
        .ok()
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|source| WorkConfigError::InvalidInteger {
                    key,
                    source: Box::new(source),
                })
        })
        .transpose()
}

#[derive(Debug, Error)]
pub enum WorkConfigError {
    #[error("{key} is not a valid integer")]
    InvalidInteger {
        key: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{0} must be greater than zero")]
    NotPositive(&'static str),
    #[error("{ENV_RETRY_MAX_SECONDS} must be greater than or equal to {ENV_RETRY_BASE_SECONDS}")]
    RetryRange,
    #[error("{min_key} must be less than or equal to {max_key}")]
    MinGreaterThanMax {
        min_key: &'static str,
        max_key: &'static str,
    },
    #[error("{key} exceeds maximum allowed ceiling of {ceiling}")]
    ExceedsCeiling { key: &'static str, ceiling: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_positive_lease_and_batch() {
        let mut config = WorkEngineConfig {
            lease_duration: Duration::ZERO,
            ..WorkEngineConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(WorkConfigError::NotPositive(ENV_LEASE_SECONDS))
        ));

        config = WorkEngineConfig::default();
        config.claim_batch_size = 0;
        assert!(matches!(
            config.validate(),
            Err(WorkConfigError::NotPositive(ENV_CLAIM_BATCH_SIZE))
        ));
    }

    #[test]
    fn database_pool_config_validation() {
        let mut config = DatabasePoolConfig::default();
        assert!(config.validate().is_ok());

        config.max_connections = 0;
        assert!(matches!(
            config.validate(),
            Err(WorkConfigError::NotPositive(ENV_DATABASE_MAX_CONNECTIONS))
        ));

        config = DatabasePoolConfig::default();
        config.max_connections = MAX_DATABASE_CONNECTIONS_CEILING + 1;
        assert!(matches!(
            config.validate(),
            Err(WorkConfigError::ExceedsCeiling { .. })
        ));

        config = DatabasePoolConfig::default();
        config.min_connections = 50;
        config.max_connections = 20;
        assert!(matches!(
            config.validate(),
            Err(WorkConfigError::MinGreaterThanMax { .. })
        ));
    }
}
