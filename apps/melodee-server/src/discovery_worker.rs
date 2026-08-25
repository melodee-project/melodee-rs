//! Periodic chart/recommendation aggregate maintenance (PRD-WORK-003,
//!). Refreshes the materialized `native_recommendation_candidates`
//! popularity projection and retries chart-item catalog linkage from durable
//! listening/catalog facts in bounded, idempotent, set-based batches. The
//! pass only reconciles PostgreSQL projections; it never opens media.

use std::{env, fmt, time::Duration};

use melodee_persistence::{
    NativeDiscoveryMaintenanceStats, NativeDiscoveryStore, NativeDiscoveryStoreError,
};
use sqlx::PgPool;
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};
use tracing::{Instrument, error, info, info_span, instrument};

/// Environment key for enabling the discovery maintenance worker.
pub const ENV_DISCOVERY_WORKER_ENABLED: &str = "MELODEE__DISCOVERY__WORKER__ENABLED";

/// Environment key for the discovery maintenance polling interval.
pub const ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS: &str =
    "MELODEE__DISCOVERY__WORKER__POLL_INTERVAL_SECONDS";

/// Environment key for the per-statement reconciliation batch bound.
pub const ENV_DISCOVERY_WORKER_BATCH_SIZE: &str = "MELODEE__DISCOVERY__WORKER__BATCH_SIZE";

/// Environment key for the per-pass total batch bound.
pub const ENV_DISCOVERY_WORKER_MAX_BATCHES_PER_PASS: &str =
    "MELODEE__DISCOVERY__WORKER__MAX_BATCHES_PER_PASS";

const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 300;
const DEFAULT_BATCH_SIZE: u32 = 500;
const DEFAULT_MAX_BATCHES_PER_PASS: u32 = 100;

/// Process-level controls for the periodic discovery maintenance worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryWorkerConfig {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub batch_size: u32,
    pub max_batches_per_pass: u32,
}

impl DiscoveryWorkerConfig {
    /// Loads and validates only the documented discovery-worker environment
    /// keys.
    pub fn from_env() -> Result<Self, DiscoveryWorkerConfigError> {
        let enabled = env::var(ENV_DISCOVERY_WORKER_ENABLED)
            .ok()
            .map(|raw| parse_bool(ENV_DISCOVERY_WORKER_ENABLED, &raw))
            .transpose()?
            .unwrap_or(true);
        let poll_seconds = env::var(ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS)
            .ok()
            .map(|raw| {
                parse_bounded_u64(ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS, &raw, 1, 86_400)
            })
            .transpose()?
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
        let batch_size = env::var(ENV_DISCOVERY_WORKER_BATCH_SIZE)
            .ok()
            .map(|raw| parse_bounded_u32(ENV_DISCOVERY_WORKER_BATCH_SIZE, &raw, 1, 10_000))
            .transpose()?
            .unwrap_or(DEFAULT_BATCH_SIZE);
        let max_batches_per_pass = env::var(ENV_DISCOVERY_WORKER_MAX_BATCHES_PER_PASS)
            .ok()
            .map(|raw| {
                parse_bounded_u32(ENV_DISCOVERY_WORKER_MAX_BATCHES_PER_PASS, &raw, 1, 10_000)
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_BATCHES_PER_PASS);
        Ok(Self {
            enabled,
            poll_interval: Duration::from_secs(poll_seconds),
            batch_size,
            max_batches_per_pass,
        })
    }
}

impl Default for DiscoveryWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_secs(DEFAULT_POLL_INTERVAL_SECONDS),
            batch_size: DEFAULT_BATCH_SIZE,
            max_batches_per_pass: DEFAULT_MAX_BATCHES_PER_PASS,
        }
    }
}

/// Invalid process-level discovery-worker configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryWorkerConfigError {
    key: &'static str,
    value: String,
    expected: &'static str,
}

impl fmt::Display for DiscoveryWorkerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be {}, got '{}'",
            self.key, self.expected, self.value
        )
    }
}

impl std::error::Error for DiscoveryWorkerConfigError {}

fn parse_bool(key: &'static str, raw: &str) -> Result<bool, DiscoveryWorkerConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(DiscoveryWorkerConfigError {
            key,
            value: raw.to_owned(),
            expected: "true or false",
        }),
    }
}

fn parse_bounded_u64(
    key: &'static str,
    raw: &str,
    min: u64,
    max: u64,
) -> Result<u64, DiscoveryWorkerConfigError> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| DiscoveryWorkerConfigError {
            key,
            value: raw.to_owned(),
            expected: "an integer inside the documented range",
        })
}

fn parse_bounded_u32(
    key: &'static str,
    raw: &str,
    min: u32,
    max: u32,
) -> Result<u32, DiscoveryWorkerConfigError> {
    raw.parse::<u32>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| DiscoveryWorkerConfigError {
            key,
            value: raw.to_owned(),
            expected: "an integer inside the documented range",
        })
}

/// Runs one bounded chart/recommendation maintenance pass. Useful to a
/// one-shot operator command or a runtime integration test without starting
/// the scheduler.
#[instrument(skip_all, fields(batch_size = config.batch_size))]
pub async fn run_discovery_once(
    pool: &PgPool,
    config: &DiscoveryWorkerConfig,
) -> Result<NativeDiscoveryMaintenanceStats, NativeDiscoveryStoreError> {
    // Durable pool gate (ADR-0134 §8): a paused pool runs no reconciliation
    // this pass. Every statement is independently committed and idempotent,
    // so resuming later converges the same projections.
    if melodee_persistence::is_pool_paused(pool, melodee_persistence::POOL_DISCOVERY)
        .await
        .map_err(NativeDiscoveryStoreError::Sql)?
    {
        info!("discovery maintenance pool is durably paused; skipping pass");
        return Ok(NativeDiscoveryMaintenanceStats::default());
    }
    let stats = NativeDiscoveryStore
        .refresh_discovery_aggregates(pool, config.batch_size, config.max_batches_per_pass)
        .await?;
    info!(
        candidates_inserted = stats.candidates_inserted,
        candidates_corrected = stats.candidates_corrected,
        chart_items_relinked = stats.chart_items_relinked,
        batches = stats.batches,
        truncated = stats.truncated,
        "discovery maintenance pass completed"
    );
    Ok(stats)
}

/// Scheduler handle for graceful shutdown of the discovery maintenance loop.
#[derive(Debug)]
pub struct DiscoverySchedulerHandle {
    stop: watch::Sender<bool>,
    join: JoinHandle<Option<NativeDiscoveryMaintenanceStats>>,
}

impl DiscoverySchedulerHandle {
    /// Stop scheduling new passes and wait for any in-flight bounded batch to
    /// finish; every reconciliation statement is independently committed and
    /// idempotent, so stopping between batches is safe.
    pub async fn stop(self) -> Option<NativeDiscoveryMaintenanceStats> {
        let _ = self.stop.send(true);
        match self.join.await {
            Ok(stats) => stats,
            Err(join_error) => {
                error!(%join_error, "discovery scheduler task failed during shutdown");
                None
            }
        }
    }
}

/// Starts the periodic discovery maintenance loop, or returns `None` when
/// disabled by configuration.
pub fn start_discovery_loop(
    config: DiscoveryWorkerConfig,
    pool: PgPool,
) -> Option<DiscoverySchedulerHandle> {
    if !config.enabled {
        info!("discovery maintenance worker disabled by configuration");
        return None;
    }
    let (stop, mut task_stop) = watch::channel(false);
    let join = tokio::spawn(
        async move {
            info!(
                poll_interval_ms = config.poll_interval.as_millis(),
                batch_size = config.batch_size,
                max_batches_per_pass = config.max_batches_per_pass,
                "starting discovery maintenance loop"
            );
            let mut ticker = tokio::time::interval(config.poll_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let mut latest = None;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let started = time::OffsetDateTime::now_utc();
                        let tick = melodee_persistence::scheduled_tick(started, config.poll_interval);
                        let history = melodee_persistence::JobRunHistoryStore;
                        let run = match history.start_tick(&pool, "discovery", tick).await {
                            Ok(run) => run,
                            Err(error) => {
                                error!(%error, "discovery tick history could not be recorded");
                                None
                            }
                        };
                        let Some(run) = run else {
                            continue;
                        };
                        let outcome = tokio::select! {
                            result = run_discovery_once(&pool, &config) => Some(result),
                            _ = task_stop.changed() => None,
                        };
                        let Some(outcome) = outcome else {
                            let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), false, Some("scheduler stopped during run")).await;
                            break;
                        };
                        match outcome {
                            Ok(stats) => {
                                let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), true, None).await;
                                latest = Some(stats);
                            }
                            Err(error) => {
                                let message = error.to_string();
                                let _ = history.finish(&pool, run.id, time::OffsetDateTime::now_utc(), false, Some(&message)).await;
                                error!(%error, "discovery maintenance pass failed");
                            }
                        }
                    }
                    _ = task_stop.changed() => break,
                }
            }
            latest
        }
        .instrument(info_span!("discovery_maintenance_loop")),
    );
    Some(DiscoverySchedulerHandle { stop, join })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn discovery_worker_config_defaults_are_enabled_and_bounded() {
        let config = DiscoveryWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.poll_interval, Duration::from_secs(300));
        assert_eq!(config.batch_size, 500);
        assert_eq!(config.max_batches_per_pass, 100);
    }

    #[test]
    fn discovery_worker_boolean_parser_is_strict_and_operator_friendly() {
        assert_eq!(parse_bool(ENV_DISCOVERY_WORKER_ENABLED, "yes"), Ok(true));
        assert_eq!(parse_bool(ENV_DISCOVERY_WORKER_ENABLED, "OFF"), Ok(false));
        assert!(parse_bool(ENV_DISCOVERY_WORKER_ENABLED, "sometimes").is_err());
    }

    #[test]
    fn discovery_worker_numeric_parsers_enforce_documented_ranges() {
        assert!(
            parse_bounded_u64(ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS, "0", 1, 86_400).is_err()
        );
        assert!(
            parse_bounded_u64(
                ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS,
                "soon",
                1,
                86_400
            )
            .is_err()
        );
        assert_eq!(
            parse_bounded_u64(ENV_DISCOVERY_WORKER_POLL_INTERVAL_SECONDS, "7", 1, 86_400),
            Ok(7)
        );
        assert!(parse_bounded_u32(ENV_DISCOVERY_WORKER_BATCH_SIZE, "0", 1, 10_000).is_err());
        assert!(parse_bounded_u32(ENV_DISCOVERY_WORKER_BATCH_SIZE, "10001", 1, 10_000).is_err());
        assert_eq!(
            parse_bounded_u32(ENV_DISCOVERY_WORKER_BATCH_SIZE, "250", 1, 10_000),
            Ok(250)
        );
    }

    #[test]
    fn disabled_worker_starts_no_loop() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let pool =
                PgPool::connect_lazy("postgres://melodee:melodee-dev@127.0.0.1:5433/melodee")
                    .expect("lazy pool");
            let config = DiscoveryWorkerConfig {
                enabled: false,
                ..DiscoveryWorkerConfig::default()
            };
            assert!(start_discovery_loop(config, pool).is_none());
        });
    }
}
