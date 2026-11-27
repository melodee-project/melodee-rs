//! Bounded outbox consumer and server scheduler for the active generation.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use melodee_persistence::{
    OutboxRetentionConfig, SEARCH_PROJECTION_NAME, SearchCatalogRecord, SearchDocumentCounts,
    SearchEntityType, SearchOperation, SearchOutboxItem, SearchProjectionStore, SettingsResolver,
    WorkItemStore, prune_processed_outbox, retention_status,
};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{Instrument as _, error, info, warn};
use uuid::Uuid;

use crate::config::SearchConfig;
use crate::documents::{ArtistDoc, ReleaseDoc, TrackDoc};
use crate::error::SearchError;
use crate::generation::SearchRoot;
use crate::rebuild::run_rebuild_once_with_cancel;
use crate::store::SearchStore;

/// OpenSubsonic/Native: successful outbox detail is retained for a bounded
/// window; see ADR-0214. Maintenance runs hourly in bounded passes.
const OUTBOX_MAINTENANCE_INTERVAL: time::Duration = time::Duration::hours(1);

/// Maximum consecutive full outbox batches drained before one idle-backoff
/// turn. Bounds runtime/database monopolization while still removing the
/// fixed post-batch delay for ordinary sustained drains.
const MAX_CONSECUTIVE_FULL_PASSES: u32 = 64;

/// Upper bound on lifecycles completed per projection pass. Completions are set
/// based and cheap; the bound keeps one pass from monopolising a worker while a
/// large batch drains.
const COMPLETION_BATCH_LIMIT: u32 = 500;

#[cfg(feature = "test-support")]
static INJECTED_DOCUMENT_FAILURE: std::sync::OnceLock<std::sync::Mutex<Option<Uuid>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct SearchApplyTestBarrier {
    state: Arc<SearchApplyTestBarrierState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Default)]
pub struct SearchRebuildTestBarrier {
    state: Arc<SearchRebuildTestBarrierState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
struct SearchRebuildTestBarrierState {
    paused: AtomicBool,
    paused_notify: Notify,
    released: AtomicBool,
    released_notify: Notify,
    snapshot_pause_requested: AtomicBool,
    snapshot_paused: AtomicBool,
    snapshot_paused_notify: Notify,
    snapshot_released: AtomicBool,
    snapshot_released_notify: Notify,
    activated_at: std::sync::Mutex<Option<std::time::Instant>>,
    query_pause_requested: AtomicBool,
    query_paused: AtomicBool,
    query_paused_notify: Notify,
    query_released: AtomicBool,
    query_released_notify: Notify,
}

#[cfg(feature = "test-support")]
impl SearchRebuildTestBarrier {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request an additional pause immediately after the snapshot starts.
    pub fn request_snapshot_pause(&self) {
        self.state
            .snapshot_pause_requested
            .store(true, Ordering::Release);
    }

    /// Wait until the rebuild owns its snapshot but has not yet read pages.
    pub async fn wait_until_snapshot_paused(&self) {
        loop {
            if self.state.snapshot_paused.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.snapshot_paused_notify.notified();
            if self.state.snapshot_paused.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Release the optional snapshot-start pause.
    pub fn release_snapshot(&self) {
        self.state.snapshot_released.store(true, Ordering::Release);
        self.state.snapshot_released_notify.notify_waiters();
    }

    /// Test-only hook; unrequested pauses preserve the original behavior.
    pub async fn pause_after_snapshot_started(&self) {
        if !self.state.snapshot_pause_requested.load(Ordering::Acquire) {
            return;
        }
        self.state.snapshot_paused.store(true, Ordering::Release);
        self.state.snapshot_paused_notify.notify_waiters();
        loop {
            if self.state.snapshot_released.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.snapshot_released_notify.notified();
            if self.state.snapshot_released.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Record when the activation transaction returns successfully.
    pub fn mark_activated(&self) {
        let mut activated_at = self
            .state
            .activated_at
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *activated_at = Some(std::time::Instant::now());
    }

    /// Return the observed activation time, if activation completed.
    #[must_use]
    pub fn activated_at(&self) -> Option<std::time::Instant> {
        *self
            .state
            .activated_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Request a pause after a query has selected its generation reader.
    pub fn request_query_pause(&self) {
        self.state
            .query_pause_requested
            .store(true, Ordering::Release);
    }

    /// Wait for a query to select its reader and reach the optional pause.
    pub async fn wait_until_query_paused(&self) {
        loop {
            if self.state.query_paused.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.query_paused_notify.notified();
            if self.state.query_paused.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Resume queries paused after their generation reader was selected.
    pub fn release_query(&self) {
        self.state.query_released.store(true, Ordering::Release);
        self.state.query_released_notify.notify_waiters();
    }

    /// Test-only query hook; unrequested pauses preserve normal behavior.
    pub async fn pause_after_query_reader(&self) {
        if !self.state.query_pause_requested.load(Ordering::Acquire) {
            return;
        }
        self.state.query_paused.store(true, Ordering::Release);
        self.state.query_paused_notify.notify_waiters();
        loop {
            if self.state.query_released.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.query_released_notify.notified();
            if self.state.query_released.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Waits until a rebuild reaches the pause point after snapshot validation
    /// and before catch-up/activation.
    pub async fn wait_until_paused(&self) {
        loop {
            if self.state.paused.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.paused_notify.notified();
            if self.state.paused.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Releases a paused rebuild.
    pub fn release(&self) {
        self.state.released.store(true, Ordering::Release);
        self.state.released_notify.notify_waiters();
    }

    /// Pauses after snapshot validation so a test can drive the ordinary
    /// outbox consumer against the previous generation before activation.
    pub async fn pause_before_catchup(&self) {
        self.state.paused.store(true, Ordering::Release);
        self.state.paused_notify.notify_waiters();
        loop {
            if self.state.released.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.released_notify.notified();
            if self.state.released.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(feature = "test-support")]
impl Default for SearchRebuildTestBarrierState {
    fn default() -> Self {
        Self {
            paused: AtomicBool::new(false),
            paused_notify: Notify::new(),
            released: AtomicBool::new(false),
            released_notify: Notify::new(),
            snapshot_pause_requested: AtomicBool::new(false),
            snapshot_paused: AtomicBool::new(false),
            snapshot_paused_notify: Notify::new(),
            snapshot_released: AtomicBool::new(false),
            snapshot_released_notify: Notify::new(),
            activated_at: std::sync::Mutex::new(None),
            query_pause_requested: AtomicBool::new(false),
            query_paused: AtomicBool::new(false),
            query_paused_notify: Notify::new(),
            query_released: AtomicBool::new(false),
            query_released_notify: Notify::new(),
        }
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Default)]
pub struct SearchSchedulerTestProbe {
    state: Arc<SearchSchedulerTestProbeState>,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Default)]
struct SearchSchedulerTestProbeState {
    idle: AtomicBool,
    idle_notify: Notify,
}

#[cfg(feature = "test-support")]
impl SearchSchedulerTestProbe {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn wait_until_idle(&self) {
        loop {
            if self.state.idle.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.idle_notify.notified();
            if self.state.idle.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn mark_idle(&self) {
        self.state.idle.store(true, Ordering::Release);
        self.state.idle_notify.notify_waiters();
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
struct SearchApplyTestBarrierState {
    armed: AtomicBool,
    reached: AtomicBool,
    reached_notify: Notify,
    cancelled: AtomicBool,
    cancelled_notify: Notify,
}

#[cfg(feature = "test-support")]
impl Default for SearchApplyTestBarrier {
    fn default() -> Self {
        Self {
            state: Arc::new(SearchApplyTestBarrierState {
                armed: AtomicBool::new(true),
                reached: AtomicBool::new(false),
                reached_notify: Notify::new(),
                cancelled: AtomicBool::new(false),
                cancelled_notify: Notify::new(),
            }),
        }
    }
}

#[cfg(feature = "test-support")]
impl SearchApplyTestBarrier {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn wait_until_reached(&self) {
        loop {
            if self.state.reached.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.reached_notify.notified();
            if self.state.reached.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait_until_cancelled(&self) {
        loop {
            if self.state.cancelled.load(Ordering::Acquire) {
                return;
            }
            let notified = self.state.cancelled_notify.notified();
            if self.state.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn pause_until_cancelled(&self, cancel: &AtomicBool) {
        if self
            .state
            .armed
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.state.reached.store(true, Ordering::Release);
        self.state.reached_notify.notify_waiters();
        while !cancel.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(1));
        }
        self.state.cancelled.store(true, Ordering::Release);
        self.state.cancelled_notify.notify_waiters();
    }
}

#[derive(Debug, Default)]
struct SearchCancellation {
    stopped: AtomicBool,
    notify: Notify,
}

impl SearchCancellation {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            let notified = self.notify.notified();
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

/// Inject or clear one deterministic document-application failure.
///
/// This is compiled only for explicit `test-support` builds and has no
/// environment/configuration surface in production.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn set_document_failure_for_test(entity_id: Option<Uuid>) {
    if let Ok(mut guard) = INJECTED_DOCUMENT_FAILURE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = entity_id;
    }
}

fn injected_document_failure(entity_id: Uuid) -> bool {
    #[cfg(feature = "test-support")]
    {
        INJECTED_DOCUMENT_FAILURE
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .is_ok_and(|guard| *guard == Some(entity_id))
    }
    #[cfg(not(feature = "test-support"))]
    {
        let _ = entity_id;
        false
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchPassStats {
    pub claimed: usize,
    pub applied: usize,
    pub acknowledged: usize,
    pub stale_or_idempotent: usize,
    pub errors: usize,
    pub no_active_generation: bool,
}

pub struct SearchSchedulerHandle {
    cancel: Arc<SearchCancellation>,
    task: JoinHandle<()>,
}

impl SearchSchedulerHandle {
    pub async fn stop(self) {
        self.cancel.stop();
        if let Err(error) = self.task.await {
            error!(%error, "search scheduler task failed while stopping");
        }
    }
}

pub fn start_search_loop(config: SearchConfig, pool: PgPool) -> SearchSchedulerHandle {
    let cancel = Arc::new(SearchCancellation::default());
    let task_cancel = Arc::clone(&cancel);
    let task = tokio::spawn(async move {
        // Monotonic gate for the hourly retention check. The durable
        // `last_prune_at` remains authoritative across restarts and processes;
        // this only avoids an extra SQL round trip on every busy pass.
        let mut maintenance_after = std::time::Instant::now();
        let mut consecutive_full_passes: u32 = 0;
        while !task_cancel.stopped.load(Ordering::Acquire) {
            if let Err(error) = ensure_initial_rebuild(&config, &pool).await {
                error!(error = ?error, "unable to provision initial search rebuild");
            }
            match run_rebuild_once_with_cancel(&config, &pool, &task_cancel.stopped).await {
                Ok(Some(job)) => info!(job_id = %job, "search rebuild pass completed"),
                Ok(None) => {}
                Err(error) => error!(error = ?error, "search rebuild pass failed"),
            }
            if task_cancel.stopped.load(Ordering::Acquire) {
                break;
            }
            if std::time::Instant::now() >= maintenance_after {
                run_outbox_maintenance_if_due(&config, &pool).await;
                maintenance_after = std::time::Instant::now()
                    + std::time::Duration::from_secs(
                        u64::try_from(OUTBOX_MAINTENANCE_INTERVAL.whole_seconds()).unwrap_or(3_600),
                    );
            }
            let pass =
                apply_outbox_once_with_cancel(&config, &pool, Arc::clone(&task_cancel)).await;
            let full_pass = match &pass {
                Ok(stats) if stats.claimed > 0 => {
                    info!(
                        claimed = stats.claimed,
                        applied = stats.applied,
                        acknowledged = stats.acknowledged,
                        errors = stats.errors,
                        "search projection pass completed"
                    );
                    // A full, clean batch means work remains: drain it without
                    // paying the idle poll interval. A bounded turn budget
                    // yields to rebuild/maintenance and other tasks so a
                    // continuously full queue cannot monopolise the runtime.
                    stats.errors == 0 && stats.claimed >= config.claim_batch_size
                }
                Ok(_) => false,
                Err(error) => {
                    error!(error = ?error, "search projection pass failed");
                    false
                }
            };
            #[cfg(feature = "test-support")]
            if let Some(probe) = &config.scheduler_test_probe {
                probe.mark_idle();
            }
            if full_pass && consecutive_full_passes < MAX_CONSECUTIVE_FULL_PASSES {
                consecutive_full_passes = consecutive_full_passes.saturating_add(1);
                tokio::task::yield_now().await;
                continue;
            }
            // Idle, errored, or turn-budget exhausted: back off for the
            // configured poll interval. Empty queues therefore do not
            // hot-poll the database.
            consecutive_full_passes = 0;
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(config.poll_interval_ms)) => {}
                () = task_cancel.cancelled() => break,
            }
        }
    });
    SearchSchedulerHandle { cancel, task }
}

/// Runs bounded outbox retention once per hour.
///
/// The durable `last_prune_at` is consulted so a restart cannot re-run an
/// interval that already completed. Failures are logged and retried on the
/// next pass; retention never blocks projection work.
async fn run_outbox_maintenance_if_due(config: &SearchConfig, pool: &PgPool) {
    let now = OffsetDateTime::now_utc();
    let retention_days = match resolve_retention_days(config, pool).await {
        Ok(days) => days,
        Err(error) => {
            warn!(error = %error, "search outbox retention setting could not be resolved");
            return;
        }
    };
    let root = SearchRoot::open(&config.search_root).ok();
    let root_text = root
        .as_ref()
        .map(|root| root.path().to_string_lossy().to_string());
    let current = match retention_status(
        pool,
        OutboxRetentionConfig { retention_days },
        now,
        root_text.as_deref(),
    )
    .await
    {
        Ok(status) => status,
        Err(error) => {
            warn!(error = %error, "search outbox retention status could not be read");
            return;
        }
    };
    if current
        .last_prune_at
        .is_some_and(|last| last > now - OUTBOX_MAINTENANCE_INTERVAL)
    {
        return;
    }
    match prune_processed_outbox(
        pool,
        OutboxRetentionConfig { retention_days },
        true,
        // One hourly pass stays bounded; the next tick resumes.
        32,
        root_text.as_deref(),
        now,
    )
    .await
    {
        Ok(result) => {
            if result.rows_pruned > 0 || result.status.blocking_rebuild.is_some() {
                info!(
                    rows_pruned = result.rows_pruned,
                    batches = result.batches,
                    eligible = result.status.eligible,
                    protected = result.status.protected,
                    blocking_rebuild = ?result.status.blocking_rebuild,
                    "search outbox retention pass completed"
                );
            }
        }
        Err(error) => warn!(error = %error, "search outbox retention pass failed"),
    }
}

/// Effective processed-outbox retention, from the ordinary Admin-editable
/// setting (environment → config file → database → default), clamped to the
/// documented range.
async fn resolve_retention_days(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<i64, melodee_persistence::SettingsError> {
    let resolver = SettingsResolver::from_env(pool)?;
    let value = resolver
        .resolve("search.processedOutboxRetentionDays")
        .await?
        .value
        .and_then(|value| value.as_i64())
        .unwrap_or(config.processed_outbox_retention_days);
    Ok(value.clamp(
        melodee_persistence::MIN_PROCESSED_OUTBOX_RETENTION_DAYS,
        melodee_persistence::MAX_PROCESSED_OUTBOX_RETENTION_DAYS,
    ))
}

/// Advance Releases whose search projection is durable from `indexing` to the
/// terminal `indexed` state (SPEC §11.2, `WorkItemStore::complete_indexed`).
///
/// The search worker owns the only durable evidence that indexing finished, so
/// it owns this hop. It runs on every pass with an active generation — including
/// passes that claimed nothing — so a queue that was already drained still
/// completes. Best-effort: a failure here must never roll back an acknowledged,
/// durable index, and the next pass retries the (idempotent) completion.
async fn complete_projected_releases(pool: &PgPool) {
    match WorkItemStore
        .complete_indexed(
            pool,
            OffsetDateTime::now_utc(),
            COMPLETION_BATCH_LIMIT,
            "search.projection.complete",
        )
        .await
    {
        Ok(completed) if !completed.is_empty() => {
            info!(
                completed = completed.len(),
                "search projection completed Releases"
            );
        }
        Ok(_) => {}
        Err(error) => {
            warn!(error = ?error, "could not advance completed search projections to Indexed");
        }
    }
}

async fn ensure_initial_rebuild(config: &SearchConfig, pool: &PgPool) -> Result<(), SearchError> {
    let root = SearchRoot::open(&config.search_root)?;
    let root_text = root.path().to_string_lossy();
    if SearchProjectionStore::active_generation(pool, root_text.as_ref())
        .await?
        .is_none()
    {
        let status = SearchProjectionStore::status(
            pool,
            SEARCH_PROJECTION_NAME,
            root_text.as_ref(),
            OffsetDateTime::now_utc(),
        )
        .await?;
        if status.latest_rebuild_status.as_deref() == Some("failed") {
            return Err(SearchError::InvalidQuery(
                "latest search rebuild failed; inspect status and request a replacement rebuild after correcting the cause"
                    .to_string(),
            ));
        }
        match SearchProjectionStore::request_rebuild(
            pool,
            root_text.as_ref(),
            "server-startup",
            OffsetDateTime::now_utc(),
        )
        .await
        {
            Ok(job) => info!(job_id = %job.job_id, "requested initial search generation"),
            Err(melodee_persistence::SearchStoreError::RebuildConflict(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub async fn apply_outbox_once(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<SearchPassStats, SearchError> {
    apply_outbox_once_with_cancel(config, pool, Arc::new(SearchCancellation::default())).await
}

async fn apply_outbox_once_with_cancel(
    config: &SearchConfig,
    pool: &PgPool,
    cancel: Arc<SearchCancellation>,
) -> Result<SearchPassStats, SearchError> {
    config.validate().map_err(SearchError::InvalidQuery)?;
    let root = SearchRoot::open(&config.search_root)?;
    let root_text = root.path().to_string_lossy();
    let Some(active) = SearchProjectionStore::active_generation(pool, root_text.as_ref()).await?
    else {
        return Ok(SearchPassStats {
            no_active_generation: true,
            ..SearchPassStats::default()
        });
    };
    let generation_path = root.generation_path(active.generation_id);
    let _ = root.read_manifest(active.generation_id)?;
    let worker = format!("search-{}", Uuid::now_v7());
    let now = OffsetDateTime::now_utc();
    // one span per claimed outbox pass; every nested load/apply/failure
    // event inherits the worker and generation identity.
    let pass_span = tracing::info_span!(
        "search_outbox_pass",
        %worker,
        generation_id = %active.generation_id,
        claimed = tracing::field::Empty,
    );
    let items = SearchProjectionStore::claim_outbox(
        pool,
        SEARCH_PROJECTION_NAME,
        &worker,
        config.claim_batch_size,
        Duration::from_secs(config.lease_seconds),
        now,
    )
    .instrument(pass_span.clone())
    .await?;
    pass_span.record("claimed", items.len() as u64);
    if items.is_empty() {
        // Nothing to project, but a Release whose projection was already
        // durable still has to reach the terminal state.
        complete_projected_releases(pool).await;
        return Ok(SearchPassStats::default());
    }
    let ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    // PERF-005 / RM-13: hydrate the whole batch with one round trip per entity
    // type, preserving the per-event order. On a batch read error, fall back to
    // per-event reads so the existing retryable/contract failure classification
    // is unchanged.
    let mut loaded = Vec::with_capacity(items.len());
    let mut load_failures = Vec::new();
    match SearchProjectionStore::load_catalog_records(pool, &items).await {
        Ok(records) => {
            for (item, record) in items.into_iter().zip(records) {
                loaded.push((item, record));
            }
        }
        Err(batch_error) => {
            warn!(%batch_error, "batched catalog hydration failed; falling back to per-event reads");
            for item in items {
                let record = if item.operation == SearchOperation::Delete {
                    None
                } else {
                    match SearchProjectionStore::load_catalog_record(
                        pool,
                        item.entity_type,
                        item.entity_id,
                    )
                    .await
                    {
                        Ok(record) => record,
                        Err(error) => {
                            let now = OffsetDateTime::now_utc();
                            let (failure_kind, retry_at) = match &error {
                                melodee_persistence::SearchStoreError::Database(_) => {
                                    ("catalog_read", Some(now + time::Duration::seconds(1)))
                                }
                                _ => ("catalog_contract", None),
                            };
                            load_failures.push((
                                item.id,
                                failure_kind,
                                error.to_string(),
                                retry_at,
                            ));
                            continue;
                        }
                    }
                };
                loaded.push((item, record));
            }
        }
    }
    let load_failures_count = load_failures.len();
    if loaded.is_empty() {
        for (outbox_id, failure_kind, message, retry_at) in load_failures {
            if let Err(release_error) = SearchProjectionStore::record_outbox_failure(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                &[outbox_id],
                failure_kind,
                &message,
                retry_at,
                config.max_outbox_attempts,
                true,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                warn!(%release_error, %worker, generation_id = %active.generation_id, outbox_id = outbox_id, "search catalog-read failure evidence could not be recorded");
                return Err(release_error.into());
            }
        }
        return Ok(SearchPassStats {
            claimed: ids.len(),
            errors: load_failures_count,
            ..SearchPassStats::default()
        });
    }
    let process_ids = loaded.iter().map(|item| item.0.id).collect::<Vec<_>>();
    SearchProjectionStore::renew_outbox_lease(
        pool,
        SEARCH_PROJECTION_NAME,
        &worker,
        &ids,
        Duration::from_secs(config.lease_seconds),
        OffsetDateTime::now_utc(),
    )
    .await?;

    let generation_id = active.generation_id;
    let apply_cancel = Arc::new(AtomicBool::new(false));
    let task_cancel = Arc::clone(&apply_cancel);
    #[cfg(feature = "test-support")]
    let test_barrier = config.apply_test_barrier.clone();
    let mut handle = tokio::task::spawn_blocking(move || {
        apply_batch(
            &generation_path,
            loaded,
            &task_cancel,
            #[cfg(feature = "test-support")]
            test_barrier.as_ref(),
        )
    });
    let lease_duration = Duration::from_secs(config.lease_seconds);
    let heartbeat_period = (lease_duration / 3).max(Duration::from_millis(100));
    let mut heartbeat = tokio::time::interval(heartbeat_period);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let applied = loop {
        tokio::select! {
            result = &mut handle => {
                break result
                    .map_err(|error| SearchError::Retryable(format!("index task failed: {error}")))?;
            }
            () = cancel.cancelled() => {
                apply_cancel.store(true, Ordering::Release);
                let _ = handle.await;
                release_owned_outbox_lease(pool, &worker, "cancellation").await;
                return Err(SearchError::Retryable("search outbox application cancelled".to_string()));
            }
            _ = heartbeat.tick() => {
                if let Err(error) = SearchProjectionStore::renew_outbox_lease(
                    pool,
                    SEARCH_PROJECTION_NAME,
                    &worker,
                    &ids,
                    lease_duration,
                    OffsetDateTime::now_utc(),
                ).await {
                    // Lease loss fences only this application pass. Scheduler
                    // shutdown is a distinct signal; conflating the two would
                    // turn one transient renewal failure into a dead worker.
                    apply_cancel.store(true, Ordering::Release);
                    let _ = handle.await;
                    release_owned_outbox_lease(pool, &worker, "lease loss").await;
                    return Err(error.into());
                }
            }
        }
    };
    match applied {
        Ok(batch) => {
            if !batch.acknowledged_ids.is_empty() {
                SearchProjectionStore::acknowledge_outbox_with_generation(
                    pool,
                    SEARCH_PROJECTION_NAME,
                    &worker,
                    &batch.acknowledged_ids,
                    generation_id,
                    &batch.counts,
                    OffsetDateTime::now_utc(),
                )
                .await?;
            }
            for failure in &batch.failures {
                if let Err(failure_error) = SearchProjectionStore::record_outbox_failure(
                    pool,
                    SEARCH_PROJECTION_NAME,
                    &worker,
                    &[failure.outbox_id],
                    "index_document",
                    &failure.error,
                    None,
                    config.max_outbox_attempts,
                    false,
                    OffsetDateTime::now_utc(),
                )
                .await
                {
                    if let Err(lease_error) = SearchProjectionStore::release_outbox_lease(
                        pool,
                        SEARCH_PROJECTION_NAME,
                        &worker,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    {
                        warn!(%lease_error, outbox_id = failure.outbox_id, "search outbox lease owner could not be released after index failure");
                    }
                    return Err(failure_error.into());
                }
            }
            for (outbox_id, failure_kind, message, retry_at) in load_failures {
                if let Err(failure_error) = SearchProjectionStore::record_outbox_failure(
                    pool,
                    SEARCH_PROJECTION_NAME,
                    &worker,
                    &[outbox_id],
                    failure_kind,
                    &message,
                    retry_at,
                    config.max_outbox_attempts,
                    false,
                    OffsetDateTime::now_utc(),
                )
                .await
                {
                    warn!(%failure_error, %worker, generation_id = %generation_id, outbox_id = outbox_id, "search catalog-read failure evidence could not be recorded");
                    if let Err(lease_error) = SearchProjectionStore::release_outbox_lease(
                        pool,
                        SEARCH_PROJECTION_NAME,
                        &worker,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    {
                        warn!(%lease_error, outbox_id = outbox_id, "search outbox lease owner could not be released after index failure");
                    }
                    return Err(failure_error.into());
                }
            }
            if let Err(release_error) = SearchProjectionStore::release_outbox_lease(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                warn!(%release_error, "search outbox lease owner could not be released after batch application");
            }
            complete_projected_releases(pool).await;
            Ok(SearchPassStats {
                claimed: ids.len(),
                applied: batch.applied,
                acknowledged: batch.acknowledged_ids.len(),
                stale_or_idempotent: batch.acknowledged_ids.len().saturating_sub(batch.applied),
                errors: batch.failures.len() + load_failures_count,
                no_active_generation: false,
            })
        }
        Err(error) => {
            let retry_at = OffsetDateTime::now_utc() + time::Duration::seconds(1);
            if let Err(release_error) = SearchProjectionStore::record_outbox_failure(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                &process_ids,
                "index_io",
                &error.to_string(),
                Some(retry_at),
                config.max_outbox_attempts,
                true,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                if let Err(lease_error) = SearchProjectionStore::release_outbox_lease(
                    pool,
                    SEARCH_PROJECTION_NAME,
                    &worker,
                    OffsetDateTime::now_utc(),
                )
                .await
                {
                    warn!(%lease_error, "search outbox lease owner could not be released after index_io failure");
                }
                warn!(%release_error, error = ?error, "search failure evidence could not be recorded");
            } else if let Err(lease_error) = SearchProjectionStore::release_outbox_lease(
                pool,
                SEARCH_PROJECTION_NAME,
                &worker,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                warn!(%lease_error, error = ?error, "search outbox lease owner could not be released after index_io failure");
            }
            Err(error)
        }
    }
}

async fn release_owned_outbox_lease(pool: &PgPool, worker: &str, cause: &'static str) {
    if let Err(error) = SearchProjectionStore::release_outbox_lease(
        pool,
        SEARCH_PROJECTION_NAME,
        worker,
        OffsetDateTime::now_utc(),
    )
    .await
    {
        warn!(error = ?error, cause, "search outbox lease could not be released");
    }
}

#[derive(Debug)]
struct FailedOutboxRow {
    outbox_id: i64,
    error: String,
}

#[derive(Debug)]
struct BatchApplyResult {
    applied: usize,
    acknowledged_ids: Vec<i64>,
    failures: Vec<FailedOutboxRow>,
    counts: SearchDocumentCounts,
}

fn apply_batch(
    generation_path: &Path,
    rows: Vec<(SearchOutboxItem, Option<SearchCatalogRecord>)>,
    cancel: &AtomicBool,
    #[cfg(feature = "test-support")] test_barrier: Option<&SearchApplyTestBarrier>,
) -> Result<BatchApplyResult, SearchError> {
    let mut store = SearchStore::open(generation_path)?;
    let mut accepted = Vec::with_capacity(rows.len());
    let mut failures = Vec::new();
    let mut applied = 0;
    for row in rows {
        check_apply_cancel(cancel)?;
        match apply_row(&mut store, &row) {
            Ok(wrote) => {
                applied += usize::from(wrote);
                accepted.push(row);
            }
            Err(error) => {
                failures.push(FailedOutboxRow {
                    outbox_id: row.0.id,
                    error: error.to_string(),
                });
                // Upsert schedules deletion before Tantivy validates the new
                // document. Drop the uncommitted writer so the failed row
                // cannot partially remove an existing document, then replay
                // only rows already proven valid into a fresh writer.
                drop(store);
                store = SearchStore::open(generation_path)?;
                applied = 0;
                for accepted_row in &accepted {
                    check_apply_cancel(cancel)?;
                    applied += usize::from(apply_row(&mut store, accepted_row)?);
                }
            }
        }
    }
    #[cfg(feature = "test-support")]
    if let Some(test_barrier) = test_barrier {
        test_barrier.pause_until_cancelled(cancel);
    }
    check_apply_cancel(cancel)?;
    // The caller acknowledges PostgreSQL only after this durable commit.
    store.commit()?;
    let stats = store.stats()?;
    let counts = SearchDocumentCounts {
        artists: u64::try_from(stats.artists)
            .map_err(|_| SearchError::Retryable("Artist count overflow".to_string()))?,
        releases: u64::try_from(stats.releases)
            .map_err(|_| SearchError::Retryable("Release count overflow".to_string()))?,
        tracks: u64::try_from(stats.tracks)
            .map_err(|_| SearchError::Retryable("Track count overflow".to_string()))?,
    };
    Ok(BatchApplyResult {
        applied,
        acknowledged_ids: accepted.into_iter().map(|(item, _)| item.id).collect(),
        failures,
        counts,
    })
}

fn check_apply_cancel(cancel: &AtomicBool) -> Result<(), SearchError> {
    if cancel.load(Ordering::Acquire) {
        Err(SearchError::Retryable(
            "search outbox application cancelled before commit".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn apply_row(
    store: &mut SearchStore,
    (item, record): &(SearchOutboxItem, Option<SearchCatalogRecord>),
) -> Result<bool, SearchError> {
    if injected_document_failure(item.entity_id) {
        return Err(SearchError::InvalidQuery(
            "injected document-application failure".to_string(),
        ));
    }
    match (item.operation, record) {
        (_, Some(SearchCatalogRecord::NoDocument)) => Ok(false),
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Artist(record))) => {
            store.upsert_artist(&ArtistDoc::from(record.clone()))
        }
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Release(record))) => {
            store.upsert_release(&ReleaseDoc::from(record.clone()))
        }
        (SearchOperation::Upsert, Some(SearchCatalogRecord::Track(record))) => {
            store.upsert_track(&TrackDoc::from(record.clone()))
        }
        (SearchOperation::Delete, _) | (SearchOperation::Upsert, None) => {
            if item.entity_type == SearchEntityType::AudioIdentity {
                Ok(false)
            } else {
                store.delete(
                    item.entity_type.as_str(),
                    &item.entity_id.to_string(),
                    item.entity_revision,
                )
            }
        }
    }
}
