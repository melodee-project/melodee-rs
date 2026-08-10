//! Policy configuration: reads runtime settings from environment variables
//! with safe defaults, validated before use.

use std::env;
use std::str::FromStr;
use thiserror::Error;

pub const ENV_POLICY_BATCH_SIZE: &str = "MELODEE__POLICY__BATCH_SIZE";
pub const ENV_POLICY_CONCURRENCY: &str = "MELODEE__POLICY__CONCURRENCY";
pub const ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS: &str =
    "MELODEE__POLICY__DESTRUCTIVE_CONFIDENCE_BPS";

const DEFAULT_BATCH_SIZE: usize = 500;
const DEFAULT_CONCURRENCY: usize = 4;
const DEFAULT_DESTRUCTIVE_CONFIDENCE_BPS: u16 = 8_000;

#[derive(Debug, Error)]
pub enum PolicyConfigError {
    #[error("invalid batch size: {0}")]
    BatchSize(String),
    #[error("invalid concurrency: {0}")]
    Concurrency(String),
    #[error("invalid destructive confidence basis points: {0}")]
    Confidence(String),
}

/// Runtime configuration for policy evaluation, loaded from the environment.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub batch_size: usize,
    pub concurrency: usize,
    pub destructive_confidence_bps: u16,
}

impl PolicyConfig {
    pub fn from_env() -> Result<Self, PolicyConfigError> {
        let batch_size = env::var(ENV_POLICY_BATCH_SIZE)
            .ok()
            .map(|v| usize::from_str(&v).map_err(|_| PolicyConfigError::BatchSize(v)))
            .transpose()?
            .unwrap_or(DEFAULT_BATCH_SIZE);

        let concurrency = env::var(ENV_POLICY_CONCURRENCY)
            .ok()
            .map(|v| usize::from_str(&v).map_err(|_| PolicyConfigError::Concurrency(v)))
            .transpose()?
            .unwrap_or(DEFAULT_CONCURRENCY);

        let confidence = env::var(ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS)
            .ok()
            .map(|v| {
                u16::from_str(&v)
                    .map_err(|_| PolicyConfigError::Confidence(v.clone()))
                    .and_then(|val| {
                        if val > 10_000 {
                            Err(PolicyConfigError::Confidence(v))
                        } else {
                            Ok(val)
                        }
                    })
            })
            .transpose()?
            .unwrap_or(DEFAULT_DESTRUCTIVE_CONFIDENCE_BPS);

        Ok(Self {
            batch_size,
            concurrency,
            destructive_confidence_bps: confidence,
        })
    }
}
