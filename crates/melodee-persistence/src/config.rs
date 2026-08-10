//! Explicit work-engine configuration.
//!
//! Only these keys are read from the environment. The legacy server's
//! indiscriminate absorption of every process environment variable is
//! deliberately NOT reproduced.

use melodee_domain::RetryPolicy;
use thiserror::Error;
use time::Duration;

/// Environment keys (the complete set this crate honors).
pub const ENV_DATABASE_URL: &str = "MELODEE__DATABASE__URL";
pub const ENV_LEASE_SECONDS: &str = "MELODEE__WORK__LEASE_SECONDS";
pub const ENV_RETRY_BASE_SECONDS: &str = "MELODEE__WORK__RETRY_BASE_SECONDS";
pub const ENV_RETRY_MAX_SECONDS: &str = "MELODEE__WORK__RETRY_MAX_SECONDS";
pub const ENV_MAX_ATTEMPTS: &str = "MELODEE__WORK__MAX_ATTEMPTS";
pub const ENV_CLAIM_BATCH_SIZE: &str = "MELODEE__WORK__CLAIM_BATCH_SIZE";

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
    std::env::var(key)
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
}
