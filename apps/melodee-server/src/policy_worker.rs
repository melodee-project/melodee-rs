//! Independently bounded policy evaluation over authoritative
//! PostgreSQL catalog facts. This module never opens source or canonical media.

use std::{
    env, fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

use melodee_domain::{FailureClass, WorkErrorCode};
use melodee_persistence::{
    PolicyClaim, PolicyEvaluationCommit, PolicyStore, PolicyStoreError, WorkEngineConfig, WorkerId,
};
use melodee_policy::{
    PolicyConfig, PolicyEngine, PolicyEngineError, PolicyEvaluation, default_rule_set,
    evaluate_catalog_context,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::{task::JoinSet, time::MissedTickBehavior};
use tracing::{Instrument, debug, error, info, info_span, warn};

/// Environment key for enabling the policy worker.
pub const ENV_POLICY_WORKER_ENABLED: &str = "MELODEE__POLICY__WORKER__ENABLED";

/// Environment key for the independently bounded policy polling interval.
pub const ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS: &str =
    "MELODEE__POLICY__WORKER__POLL_INTERVAL_SECONDS";

const DEFAULT_POLICY_POLL_INTERVAL_SECONDS: u64 = 60;

/// Process-level controls for the periodic policy worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyWorkerConfig {
    pub enabled: bool,
    pub poll_interval: std::time::Duration,
}

impl PolicyWorkerConfig {
    /// Loads and validates only the documented policy-worker environment keys.
    pub fn from_env() -> Result<Self, PolicyWorkerConfigError> {
        let enabled = env::var(ENV_POLICY_WORKER_ENABLED)
            .ok()
            .map(|raw| parse_bool(ENV_POLICY_WORKER_ENABLED, &raw))
            .transpose()?
            .unwrap_or(true);
        let poll_seconds = env::var(ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS)
            .ok()
            .map(|raw| parse_positive_u64(ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS, &raw))
            .transpose()?
            .unwrap_or(DEFAULT_POLICY_POLL_INTERVAL_SECONDS);

        Ok(Self {
            enabled,
            poll_interval: std::time::Duration::from_secs(poll_seconds),
        })
    }
}

impl Default for PolicyWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: std::time::Duration::from_secs(DEFAULT_POLICY_POLL_INTERVAL_SECONDS),
        }
    }
}

/// Invalid process-level policy-worker configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyWorkerConfigError {
    key: &'static str,
    value: String,
    expected: &'static str,
}

impl fmt::Display for PolicyWorkerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be {}, got '{}'",
            self.key, self.expected, self.value
        )
    }
}

impl std::error::Error for PolicyWorkerConfigError {}

fn parse_bool(key: &'static str, raw: &str) -> Result<bool, PolicyWorkerConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(PolicyWorkerConfigError {
            key,
            value: raw.to_owned(),
            expected: "true or false",
        }),
    }
}

fn parse_positive_u64(key: &'static str, raw: &str) -> Result<u64, PolicyWorkerConfigError> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| PolicyWorkerConfigError {
            key,
            value: raw.to_owned(),
            expected: "a positive integer",
        })
}

/// Failures before a bounded policy pass can account for individual work.
#[derive(Debug, Error)]
pub enum PolicyWorkerError {
    #[error("policy persistence failed")]
    Store(#[from] PolicyStoreError),
    #[error("policy engine setup or evaluation failed")]
    Engine(#[from] PolicyEngineError),
    #[error("policy batch/concurrency does not fit the durable claim bound")]
    ClaimLimit,
}

/// Observable result of one bounded policy pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyWorkerStats {
    pub claimed: usize,
    pub evaluated: usize,
    pub decisions_inserted: usize,
    pub decisions_adopted: usize,
    pub released: usize,
    pub failed: usize,
    pub errors: usize,
    pub max_in_flight: usize,
    pub set_based_mutation_statement_count: u32,
    pub duration_ms: u128,
}

/// Sticky cooperative cancellation for work that has not started its atomic
/// policy commit.
#[derive(Debug, Clone, Default)]
pub struct PolicyCancellation {
    cancelled: Arc<AtomicBool>,
    signal: Arc<tokio::sync::Notify>,
}

impl PolicyCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // There is one scheduler waiter; a stored permit closes the race
        // between its atomic check and async notification registration.
        self.signal.notify_one();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        if !self.is_cancelled() {
            self.signal.notified().await;
        }
    }
}

/// Scheduler handle for graceful shutdown of the policy loop.
#[derive(Debug)]
pub struct PolicySchedulerHandle {
    join: tokio::task::JoinHandle<Option<PolicyWorkerStats>>,
    cancellation: PolicyCancellation,
}

impl PolicySchedulerHandle {
    /// Stop claiming new work, release pre-commit claims, and wait for any
    /// already-started fenced transaction to commit or roll back atomically.
    pub async fn stop(self) -> Option<PolicyWorkerStats> {
        self.cancellation.cancel();
        match self.join.await {
            Ok(stats) => stats,
            Err(join_error) => {
                error!(%join_error, "policy scheduler task failed during shutdown");
                None
            }
        }
    }
}

/// Provisions the configured immutable rule set and starts periodic policy
/// evaluation. Rule-set conflicts fail startup rather than changing history.
pub async fn start_policy_loop(
    policy_config: PolicyConfig,
    worker_config: PolicyWorkerConfig,
    pool: PgPool,
    work_config: WorkEngineConfig,
) -> Result<Option<PolicySchedulerHandle>, PolicyWorkerError> {
    let store = PolicyStore;
    let proposed_rule_set = default_rule_set(&policy_config)?;
    let persisted_rule_set = store
        .ensure_rule_set(&pool, &proposed_rule_set, OffsetDateTime::now_utc())
        .await?;
    let engine = PolicyEngine::with_rule_set(policy_config.clone(), persisted_rule_set)?;
    if !worker_config.enabled {
        info!(
            rule_set_id = %engine.rule_set_id(),
            rule_set_version = engine.rule_set_version(),
            "policy rule set loaded; worker disabled by configuration"
        );
        return Ok(None);
    }
    let cancellation = PolicyCancellation::default();
    let task_cancellation = cancellation.clone();
    let claim_batch_size = bounded_claim_config(&policy_config, &work_config)?.claim_batch_size;
    let join = tokio::spawn(async move {
        info!(
            rule_set_id = %engine.rule_set_id(),
            rule_set_version = engine.rule_set_version(),
            concurrency = policy_config.concurrency,
            batch_size = policy_config.batch_size,
            "starting policy worker loop"
        );
        // The poll interval is the *idle* cadence, not a throughput ceiling. One
        // pass evaluates `batch_size.min(concurrency)` releases in ~100 ms, so a
        // scheduler that only ran one pass per tick drained a backlog of several
        // hundred releases at four releases a minute (measured) while the worker sat
        // idle for the rest of every tick. Draining a full batch immediately keeps
        // the stage work-bound: a batch that comes back full means more work is
        // queued, so run again before sleeping. Job history stays per scheduled
        // tick, which is what the diagnostics surface reports.
        let claim_batch_size = claim_batch_size;
        let mut ticker = tokio::time::interval(worker_config.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut latest = None;
        loop {
            if task_cancellation.is_cancelled() {
                break;
            }
            tokio::select! {
                _ = ticker.tick() => {}
                () = task_cancellation.cancelled() => break,
            }
            let started = OffsetDateTime::now_utc();
            let tick = melodee_persistence::scheduled_tick(started, worker_config.poll_interval);
            let history = melodee_persistence::JobRunHistoryStore;
            let run = match history.start_tick(&pool, "policy", tick).await {
                Ok(run) => run,
                Err(error) => {
                    error!(%error, "policy tick history could not be recorded");
                    None
                }
            };
            let Some(run) = run else {
                continue;
            };
            let finished;
            loop {
                match run_policy_once_with_cancellation(
                    &engine,
                    &policy_config,
                    &store,
                    &pool,
                    &work_config,
                    OffsetDateTime::now_utc(),
                    &task_cancellation,
                )
                .await
                {
                    Ok(stats) => {
                        let full_batch = stats.claimed >= claim_batch_size as usize;
                        latest = Some(stats);
                        if !full_batch || task_cancellation.is_cancelled() {
                            finished = Some((true, None));
                            break;
                        }
                        // Yield so timers, shutdown, and other workers keep running.
                        tokio::task::yield_now().await;
                    }
                    Err(worker_error) => {
                        let message = worker_error.to_string();
                        error!(%worker_error, "policy pass failed before completion");
                        finished = Some((false, Some(message)));
                        break;
                    }
                }
            }
            let (succeeded, message) = finished.unwrap_or((true, None));
            let _ = history
                .finish(
                    &pool,
                    run.id,
                    OffsetDateTime::now_utc(),
                    succeeded,
                    message.as_deref(),
                )
                .await;
        }
        latest
    });

    Ok(Some(PolicySchedulerHandle { join, cancellation }))
}

/// Runs one independently bounded policy pass. Useful to a one-shot operator
/// command or a runtime integration test without starting a scheduler.
pub async fn run_policy_once(
    engine: &PolicyEngine,
    config: &PolicyConfig,
    store: &PolicyStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
) -> Result<PolicyWorkerStats, PolicyWorkerError> {
    run_policy_once_with_cancellation(
        engine,
        config,
        store,
        pool,
        work_config,
        now,
        &PolicyCancellation::default(),
    )
    .await
}

/// Cancellation-aware form used by the periodic scheduler and integration
/// tests proving graceful lease release.
pub async fn run_policy_once_with_cancellation(
    engine: &PolicyEngine,
    config: &PolicyConfig,
    store: &PolicyStore,
    pool: &PgPool,
    work_config: &WorkEngineConfig,
    now: OffsetDateTime,
    cancellation: &PolicyCancellation,
) -> Result<PolicyWorkerStats, PolicyWorkerError> {
    let started = Instant::now();
    // Durable pool gate (ADR-0134 §8): a paused pool claims no new work.
    // Leases claimed by earlier passes are untouched — they finish or are
    // released per the existing lease contract, never stranded.
    if melodee_persistence::is_pool_paused(pool, melodee_persistence::POOL_POLICY)
        .await
        .map_err(PolicyStoreError::from)?
    {
        info!("policy worker pool is durably paused; skipping claim batch");
        return Ok(PolicyWorkerStats::default());
    }
    let worker = WorkerId::new();
    let claim_config = bounded_claim_config(config, work_config)?;
    let claims = store
        .claim_cataloged(pool, &worker, &claim_config, now)
        .await?;
    let mut stats = PolicyWorkerStats {
        claimed: claims.len(),
        ..PolicyWorkerStats::default()
    };
    if claims.is_empty() {
        debug!("policy worker: no eligible Cataloged work items");
        return Ok(stats);
    }

    let mut tasks = JoinSet::new();
    let active_tasks = Arc::new(AtomicUsize::new(0));
    let max_active_tasks = Arc::new(AtomicUsize::new(0));
    for claim in claims {
        let store = *store;
        let pool = pool.clone();
        let worker = worker.clone();
        let engine = engine.clone();
        let cancellation = cancellation.clone();
        let lease_duration = work_config.lease_duration;
        let retry_policy = work_config.retry_policy;
        let active_tasks = Arc::clone(&active_tasks);
        let max_active_tasks = Arc::clone(&max_active_tasks);
        let span = info_span!(
            "policy_evaluation",
            work_item_id = %claim.item.id,
            correlation_id = %claim.item.correlation_id,
            release_id = %claim.release_id,
            worker_id = %worker,
        );
        tasks.spawn(
            async move {
                let _activity = TaskActivity::enter(&active_tasks, &max_active_tasks);
                process_claim(
                    &engine,
                    &store,
                    &pool,
                    &worker,
                    claim,
                    lease_duration,
                    &retry_policy,
                    &cancellation,
                )
                .await
            }
            .instrument(span),
        );
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(TaskOutcome::Committed {
                inserted,
                decision,
                set_based_mutation_statement_count,
            }) => {
                stats.evaluated += 1;
                stats.set_based_mutation_statement_count = stats
                    .set_based_mutation_statement_count
                    .saturating_add(set_based_mutation_statement_count);
                if inserted {
                    stats.decisions_inserted += 1;
                } else {
                    stats.decisions_adopted += 1;
                }
                info!(
                    release_id = %decision.release_id,
                    disposition = decision.disposition.as_str(),
                    inserted,
                    "policy evaluation committed"
                );
            }
            Ok(TaskOutcome::Released {
                work_item_id,
                result,
            }) => match result {
                Ok(()) => {
                    stats.released += 1;
                    debug!(%work_item_id, "cancelled policy claim released");
                }
                Err(release_error) => {
                    stats.errors += 1;
                    error!(%work_item_id, %release_error, "failed to release cancelled policy claim");
                }
            },
            Ok(TaskOutcome::Failed {
                work_item_id,
                error: task_error,
                recorded,
            }) => {
                stats.failed += 1;
                if let Err(record_error) = recorded {
                    stats.errors += 1;
                    error!(%work_item_id, %task_error, %record_error, "policy failure could not be durably recorded");
                } else {
                    warn!(%work_item_id, %task_error, "policy failure durably recorded");
                }
            }
            Err(join_error) => {
                stats.failed += 1;
                stats.errors += 1;
                error!(%join_error, "policy evaluation task failed to join");
            }
        }
    }
    stats.max_in_flight = max_active_tasks.load(Ordering::Acquire);

    stats.duration_ms = started.elapsed().as_millis();
    info!(
        claimed = stats.claimed,
        evaluated = stats.evaluated,
        decisions_inserted = stats.decisions_inserted,
        decisions_adopted = stats.decisions_adopted,
        released = stats.released,
        failed = stats.failed,
        errors = stats.errors,
        max_in_flight = stats.max_in_flight,
        set_based_mutation_statement_count = stats.set_based_mutation_statement_count,
        duration_ms = stats.duration_ms,
        "bounded policy pass completed"
    );
    Ok(stats)
}

enum TaskOutcome {
    Committed {
        inserted: bool,
        decision: melodee_persistence::StoredPolicyDecision,
        set_based_mutation_statement_count: u32,
    },
    Released {
        work_item_id: uuid::Uuid,
        result: Result<(), PolicyStoreError>,
    },
    Failed {
        work_item_id: uuid::Uuid,
        error: String,
        recorded: Result<(), PolicyStoreError>,
    },
}

struct TaskActivity<'a> {
    active: &'a AtomicUsize,
}

impl<'a> TaskActivity<'a> {
    fn enter(active: &'a AtomicUsize, maximum: &AtomicUsize) -> Self {
        let current = active.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        maximum.fetch_max(current, Ordering::AcqRel);
        Self { active }
    }
}

impl Drop for TaskActivity<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_claim(
    engine: &PolicyEngine,
    store: &PolicyStore,
    pool: &PgPool,
    worker: &WorkerId,
    claim: PolicyClaim,
    lease_duration: time::Duration,
    retry_policy: &melodee_domain::RetryPolicy,
    cancellation: &PolicyCancellation,
) -> TaskOutcome {
    if cancellation.is_cancelled() {
        return release_claim(store, pool, worker, claim.item.id).await;
    }
    if let Err(store_error) = store
        .renew_lease(
            pool,
            claim.item.id,
            worker,
            lease_duration,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        return TaskOutcome::Failed {
            work_item_id: claim.item.id,
            error: store_error.to_string(),
            recorded: Err(store_error),
        };
    }
    if cancellation.is_cancelled() {
        return release_claim(store, pool, worker, claim.item.id).await;
    }

    let context = match store.load_context(pool, &claim).await {
        Ok(context) => context,
        Err(store_error) => {
            return record_failure(
                store,
                pool,
                worker,
                claim.item.id,
                &store_error,
                retry_policy,
            )
            .await;
        }
    };
    let mut evaluation = match evaluate_catalog_context(engine, &context, OffsetDateTime::now_utc())
    {
        Ok(evaluation) => evaluation,
        Err(engine_error) => {
            let error = engine_error.to_string();
            let recorded = store
                .record_failure(
                    pool,
                    claim.item.id,
                    worker,
                    WorkErrorCode::new("policy.invalid_facts"),
                    &error,
                    FailureClass::Permanent,
                    retry_policy,
                    OffsetDateTime::now_utc(),
                )
                .await
                .map(|_| ());
            return TaskOutcome::Failed {
                work_item_id: claim.item.id,
                error,
                recorded,
            };
        }
    };
    // A durable decision/fact set can predate the state transition after a
    // crash in an older binary. Keep those authoritative IDs and facts so the
    // fenced commit can verify and adopt them instead of generating a conflict
    // from retry-time evidence ordering or timestamps.
    if let Some(classification) = &context.current_classification {
        evaluation.classification = stored_classification(classification);
    }
    if let Some(decision) = &context.current_decision {
        evaluation.decision = stored_decision(decision);
    }
    if cancellation.is_cancelled() {
        return release_claim(store, pool, worker, claim.item.id).await;
    }
    if let Err(store_error) = store
        .renew_lease(
            pool,
            claim.item.id,
            worker,
            lease_duration,
            OffsetDateTime::now_utc(),
        )
        .await
    {
        return TaskOutcome::Failed {
            work_item_id: claim.item.id,
            error: store_error.to_string(),
            recorded: Err(store_error),
        };
    }
    if cancellation.is_cancelled() {
        return release_claim(store, pool, worker, claim.item.id).await;
    }

    let PolicyEvaluation {
        classification,
        novelty,
        decision,
        ..
    } = evaluation;
    let (_, novelty_evaluation, track_novelty) = novelty.into_parts();
    let request = PolicyEvaluationCommit {
        work_item_id: claim.item.id,
        release_id: claim.release_id,
        source_fingerprint: claim.source_fingerprint,
        classification,
        novelty_evaluation,
        track_novelty,
        decision,
        now: OffsetDateTime::now_utc(),
    };
    match store.commit_evaluation(pool, worker, &request).await {
        Ok(result) => TaskOutcome::Committed {
            inserted: result.inserted,
            decision: result.decision,
            set_based_mutation_statement_count: result.set_based_mutation_statement_count,
        },
        Err(store_error) => {
            record_failure(
                store,
                pool,
                worker,
                claim.item.id,
                &store_error,
                retry_policy,
            )
            .await
        }
    }
}

fn stored_classification(
    stored: &melodee_persistence::StoredReleaseClassification,
) -> melodee_domain::ReleaseClassification {
    melodee_domain::ReleaseClassification {
        id: stored.id,
        release_id: stored.release_id,
        traits: stored.traits.clone(),
        evidence: stored.evidence.clone(),
        confidence: stored.confidence,
        classified_at: stored.classified_at,
        classifier_version: stored.classifier_version.clone(),
    }
}

fn stored_decision(
    stored: &melodee_persistence::StoredPolicyDecision,
) -> melodee_domain::ReleasePolicyDecision {
    melodee_domain::ReleasePolicyDecision {
        id: stored.id,
        release_id: stored.release_id,
        rule_set_id: stored.rule_set_id,
        rule_set_version: stored.rule_set_version,
        matched_rule_ids: stored.matched_rule_ids.clone(),
        disposition: stored.disposition,
        evidence_summary: stored.evidence_summary.clone(),
        source_fingerprint: stored.source_fingerprint.clone(),
        actor: stored.actor.clone(),
        decided_at: stored.decided_at,
        completion_state: stored.completion_state,
        completed_at: stored.completed_at,
    }
}

async fn release_claim(
    store: &PolicyStore,
    pool: &PgPool,
    worker: &WorkerId,
    work_item_id: uuid::Uuid,
) -> TaskOutcome {
    TaskOutcome::Released {
        work_item_id,
        result: store
            .release_lease(pool, work_item_id, worker, OffsetDateTime::now_utc())
            .await,
    }
}

async fn record_failure(
    store: &PolicyStore,
    pool: &PgPool,
    worker: &WorkerId,
    work_item_id: uuid::Uuid,
    store_error: &PolicyStoreError,
    retry_policy: &melodee_domain::RetryPolicy,
) -> TaskOutcome {
    let (code, class) = failure_disposition(store_error);
    let error = store_error.to_string();
    let recorded = store
        .record_failure(
            pool,
            work_item_id,
            worker,
            WorkErrorCode::new(code),
            &error,
            class,
            retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .map(|_| ());
    TaskOutcome::Failed {
        work_item_id,
        error,
        recorded,
    }
}

fn failure_disposition(error: &PolicyStoreError) -> (&'static str, FailureClass) {
    match error {
        PolicyStoreError::Database(_) | PolicyStoreError::Work(_) => {
            ("policy.transient", FailureClass::Retryable)
        }
        PolicyStoreError::RuleSetNotFound(_) | PolicyStoreError::RuleSetConflict => {
            ("policy.rule_set", FailureClass::Permanent)
        }
        PolicyStoreError::FingerprintMismatch => {
            ("policy.fingerprint_mismatch", FailureClass::Permanent)
        }
        PolicyStoreError::ForeignFacts
        | PolicyStoreError::IncompleteFacts
        | PolicyStoreError::DecisionConflict
        | PolicyStoreError::AuthorizationRequired
        | PolicyStoreError::NotCataloged(_)
        | PolicyStoreError::NumericOverflow
        | PolicyStoreError::CorruptValue(_)
        | PolicyStoreError::Serialization(_)
        | PolicyStoreError::Domain(_)
        | PolicyStoreError::ScopeExceeded { .. } => {
            ("policy.invalid_facts", FailureClass::Permanent)
        }
    }
}

fn bounded_claim_config(
    config: &PolicyConfig,
    work_config: &WorkEngineConfig,
) -> Result<WorkEngineConfig, PolicyWorkerError> {
    let limit = config.batch_size.min(config.concurrency);
    let claim_batch_size = u32::try_from(limit).map_err(|_| PolicyWorkerError::ClaimLimit)?;
    if claim_batch_size == 0 {
        return Err(PolicyWorkerError::ClaimLimit);
    }
    Ok(WorkEngineConfig {
        claim_batch_size,
        ..*work_config
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn policy_worker_config_defaults_are_enabled_and_bounded() {
        let config = PolicyWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.poll_interval, std::time::Duration::from_secs(60));
    }

    #[test]
    fn policy_worker_boolean_parser_is_strict_and_operator_friendly() {
        assert_eq!(parse_bool(ENV_POLICY_WORKER_ENABLED, "yes"), Ok(true));
        assert_eq!(parse_bool(ENV_POLICY_WORKER_ENABLED, "OFF"), Ok(false));
        assert!(parse_bool(ENV_POLICY_WORKER_ENABLED, "sometimes").is_err());
    }

    #[test]
    fn policy_worker_poll_interval_rejects_zero_and_invalid_values() {
        assert!(parse_positive_u64(ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS, "0").is_err());
        assert!(parse_positive_u64(ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS, "soon").is_err());
        assert_eq!(
            parse_positive_u64(ENV_POLICY_WORKER_POLL_INTERVAL_SECONDS, "7"),
            Ok(7)
        );
    }

    #[test]
    fn policy_claim_is_bounded_by_batch_and_concurrency() {
        let config = PolicyConfig {
            batch_size: 200,
            concurrency: 7,
            ..PolicyConfig::default()
        };
        let bounded = bounded_claim_config(&config, &WorkEngineConfig::default())
            .expect("validated policy bounds fit u32");
        assert_eq!(bounded.claim_batch_size, 7);
    }

    #[tokio::test]
    async fn cancellation_is_sticky_before_waiter_registration() {
        let cancellation = PolicyCancellation::default();
        cancellation.cancel();
        cancellation.cancelled().await;
        assert!(cancellation.is_cancelled());
    }
}
