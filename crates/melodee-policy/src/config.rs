//! Validated policy configuration and safe disposition preferences.

use std::env;
use std::str::FromStr;

use melodee_domain::Disposition;
use thiserror::Error;

pub const ENV_POLICY_BATCH_SIZE: &str = "MELODEE__POLICY__BATCH_SIZE";
pub const ENV_POLICY_CONCURRENCY: &str = "MELODEE__POLICY__CONCURRENCY";
pub const ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS: &str =
    "MELODEE__POLICY__DESTRUCTIVE_CONFIDENCE_BPS";
pub const ENV_POLICY_RULE_SET_VERSION: &str = "MELODEE__POLICY__RULE_SET_VERSION";
pub const ENV_POLICY_REDUNDANT_GREATEST_HITS_DISPOSITION: &str =
    "MELODEE__POLICY__REDUNDANT_GREATEST_HITS_DISPOSITION";
pub const ENV_POLICY_NOVEL_GREATEST_HITS_DISPOSITION: &str =
    "MELODEE__POLICY__NOVEL_GREATEST_HITS_DISPOSITION";
pub const ENV_POLICY_UNCERTAIN_GREATEST_HITS_DISPOSITION: &str =
    "MELODEE__POLICY__UNCERTAIN_GREATEST_HITS_DISPOSITION";

/// Current immutable version of the built-in policy rule set.
///
/// Version 2 removes the obsolete catch-all Unknown-classification review
/// rule. Operators can still author that rule explicitly when their library
/// requires it.
pub const DEFAULT_RULE_SET_VERSION: u32 = 2;

pub const MAX_POLICY_BATCH_SIZE: usize = 10_000;
pub const MAX_POLICY_CONCURRENCY: usize = 64;
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
    #[error("invalid policy rule-set version: {0}")]
    RuleSetVersion(String),
    #[error(
        "obsolete policy rule-set version {configured}; set {ENV_POLICY_RULE_SET_VERSION} to at least {minimum}"
    )]
    ObsoleteRuleSetVersion { configured: u32, minimum: u32 },
    #[error("invalid disposition for {field}: {value}")]
    Disposition { field: &'static str, value: String },
    #[error("unsafe policy preference for {field}: {disposition}")]
    UnsafePreference {
        field: &'static str,
        disposition: &'static str,
    },
}

/// Runtime settings. Preferences are restricted to the safe outcomes accepted
/// by ADR-0120 and are materialized into the versioned default rule set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConfig {
    pub batch_size: usize,
    pub concurrency: usize,
    pub destructive_confidence_bps: u16,
    pub rule_set_version: u32,
    pub redundant_greatest_hits_disposition: Disposition,
    pub novel_greatest_hits_disposition: Disposition,
    pub uncertain_greatest_hits_disposition: Disposition,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            concurrency: DEFAULT_CONCURRENCY,
            destructive_confidence_bps: DEFAULT_DESTRUCTIVE_CONFIDENCE_BPS,
            rule_set_version: DEFAULT_RULE_SET_VERSION,
            redundant_greatest_hits_disposition: Disposition::Ignore,
            novel_greatest_hits_disposition: Disposition::Review,
            uncertain_greatest_hits_disposition: Disposition::Review,
        }
    }
}

impl PolicyConfig {
    pub fn from_env() -> Result<Self, PolicyConfigError> {
        let defaults = Self::default();
        let value = Self {
            batch_size: parse_usize(ENV_POLICY_BATCH_SIZE, defaults.batch_size)
                .map_err(PolicyConfigError::BatchSize)?,
            concurrency: parse_usize(ENV_POLICY_CONCURRENCY, defaults.concurrency)
                .map_err(PolicyConfigError::Concurrency)?,
            destructive_confidence_bps: parse_u16(
                ENV_POLICY_DESTRUCTIVE_CONFIDENCE_BPS,
                defaults.destructive_confidence_bps,
            )
            .map_err(PolicyConfigError::Confidence)?,
            rule_set_version: parse_u32(ENV_POLICY_RULE_SET_VERSION, defaults.rule_set_version)
                .map_err(PolicyConfigError::RuleSetVersion)?,
            redundant_greatest_hits_disposition: parse_disposition(
                ENV_POLICY_REDUNDANT_GREATEST_HITS_DISPOSITION,
                defaults.redundant_greatest_hits_disposition,
            )?,
            novel_greatest_hits_disposition: parse_disposition(
                ENV_POLICY_NOVEL_GREATEST_HITS_DISPOSITION,
                defaults.novel_greatest_hits_disposition,
            )?,
            uncertain_greatest_hits_disposition: parse_disposition(
                ENV_POLICY_UNCERTAIN_GREATEST_HITS_DISPOSITION,
                defaults.uncertain_greatest_hits_disposition,
            )?,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), PolicyConfigError> {
        if self.batch_size == 0 || self.batch_size > MAX_POLICY_BATCH_SIZE {
            return Err(PolicyConfigError::BatchSize(self.batch_size.to_string()));
        }
        if self.concurrency == 0 || self.concurrency > MAX_POLICY_CONCURRENCY {
            return Err(PolicyConfigError::Concurrency(self.concurrency.to_string()));
        }
        if self.destructive_confidence_bps == 0 || self.destructive_confidence_bps > 10_000 {
            return Err(PolicyConfigError::Confidence(
                self.destructive_confidence_bps.to_string(),
            ));
        }
        if self.rule_set_version == 0 {
            return Err(PolicyConfigError::RuleSetVersion(
                self.rule_set_version.to_string(),
            ));
        }
        if self.rule_set_version < DEFAULT_RULE_SET_VERSION {
            return Err(PolicyConfigError::ObsoleteRuleSetVersion {
                configured: self.rule_set_version,
                minimum: DEFAULT_RULE_SET_VERSION,
            });
        }
        require_preference(
            "redundant greatest hits",
            self.redundant_greatest_hits_disposition,
            &[Disposition::Ignore, Disposition::Delete],
        )?;
        require_preference(
            "novel greatest hits",
            self.novel_greatest_hits_disposition,
            &[Disposition::Review, Disposition::Accept],
        )?;
        require_preference(
            "uncertain greatest hits",
            self.uncertain_greatest_hits_disposition,
            &[Disposition::Review, Disposition::Quarantine],
        )?;
        Ok(())
    }
}

fn parse_usize(name: &'static str, default: usize) -> Result<usize, String> {
    env::var(name)
        .ok()
        .map(|value| usize::from_str(&value).map_err(|_| value))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_u16(name: &'static str, default: u16) -> Result<u16, String> {
    env::var(name)
        .ok()
        .map(|value| u16::from_str(&value).map_err(|_| value))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_u32(name: &'static str, default: u32) -> Result<u32, String> {
    env::var(name)
        .ok()
        .map(|value| u32::from_str(&value).map_err(|_| value))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_disposition(
    name: &'static str,
    default: Disposition,
) -> Result<Disposition, PolicyConfigError> {
    let Some(value) = env::var(name).ok() else {
        return Ok(default);
    };
    Disposition::parse(&value).ok_or(PolicyConfigError::Disposition { field: name, value })
}

fn require_preference(
    field: &'static str,
    disposition: Disposition,
    allowed: &[Disposition],
) -> Result<(), PolicyConfigError> {
    if allowed.contains(&disposition) {
        Ok(())
    } else {
        Err(PolicyConfigError::UnsafePreference {
            field,
            disposition: disposition.as_str(),
        })
    }
}
