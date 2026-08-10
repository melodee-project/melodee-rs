//! Explicit work-engine configuration.
//!
//! Only these keys are read from the environment. The legacy server's
//! indiscriminate absorption of every process environment variable is
//! deliberately NOT reproduced.

use melodee_domain::RetryPolicy;
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
    pub fn from_env() -> Result<Self, std::num::ParseIntError> {
        let defaults = Self::default();
        let lease_seconds = std::env::var(ENV_LEASE_SECONDS)
            .ok()
            .map(|v| v.parse::<i64>())
            .transpose()?;
        let retry_base = std::env::var(ENV_RETRY_BASE_SECONDS)
            .ok()
            .map(|v| v.parse::<i64>())
            .transpose()?;
        let retry_max = std::env::var(ENV_RETRY_MAX_SECONDS)
            .ok()
            .map(|v| v.parse::<i64>())
            .transpose()?;
        let max_attempts = std::env::var(ENV_MAX_ATTEMPTS)
            .ok()
            .map(|v| v.parse::<u32>())
            .transpose()?;
        let batch = std::env::var(ENV_CLAIM_BATCH_SIZE)
            .ok()
            .map(|v| v.parse::<u32>())
            .transpose()?;
        Ok(Self {
            lease_duration: lease_seconds.map_or(defaults.lease_duration, Duration::seconds),
            retry_policy: RetryPolicy {
                base_delay: retry_base.map_or(defaults.retry_policy.base_delay, Duration::seconds),
                max_delay: retry_max.map_or(defaults.retry_policy.max_delay, Duration::seconds),
                max_attempts: max_attempts.unwrap_or(defaults.retry_policy.max_attempts),
            },
            claim_batch_size: batch.unwrap_or(defaults.claim_batch_size),
        })
    }
}
