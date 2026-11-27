//! Discovery and settling reconciliation.

use std::time::Instant;

use melodee_domain::IngestionState;
use melodee_persistence::{
    SettlingObservation, SettlingSnapshotFile, StagedSnapshotCompletion, WorkItemStore,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tracing::{debug, instrument, warn};
use uuid::Uuid;

use crate::{
    candidate_relative_path,
    config::InboundSource,
    config::IngestionConfig,
    discovery_scan,
    error::{IngestionError, Result},
    snapshot::CandidateSnapshot,
};

#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct SnapshotStallTestHook {
    state: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    started: std::sync::Arc<std::sync::atomic::AtomicBool>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "test-support")]
impl SnapshotStallTestHook {
    pub fn started(&self) -> bool {
        self.started.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn release(&self) {
        let (lock, condition) = &*self.state;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
    }
    fn pause(&self) {
        self.started
            .store(true, std::sync::atomic::Ordering::Release);
        let (lock, condition) = &*self.state;
        let mut released = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = condition
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReconcileStats {
    pub candidates_seen: u64,
    pub new_work_items: u64,
    pub existing_work_items: u64,
    pub settling_updated: u64,
    pub ready_promoted: u64,
    pub ready_reverted: u64,
    pub snapshot_changed: u64,
    pub missing_candidates: u64,
    pub errors: u64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ReconcileResult {
    pub source: String,
    pub stats: ReconcileStats,
}

#[derive(Debug, Clone)]
pub struct Reconciler {
    pub config: IngestionConfig,
    pub store: WorkItemStore,
}

impl Reconciler {
    pub const fn new(config: IngestionConfig, store: WorkItemStore) -> Self {
        Self { config, store }
    }

    pub async fn reconcile_all_sources(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
    ) -> Result<Vec<ReconcileResult>> {
        reconcile_all_sources(&self.config, &self.store, pool, now).await
    }

    pub async fn reconcile_source(
        &self,
        pool: &PgPool,
        source: &InboundSource,
        now: OffsetDateTime,
    ) -> Result<ReconcileResult> {
        reconcile_inbound_source(&self.config, &self.store, pool, source, now).await
    }
}

#[instrument(level = "info", skip(config, store, pool))]
pub async fn reconcile_all_sources(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    now: OffsetDateTime,
) -> Result<Vec<ReconcileResult>> {
    reconcile_all_sources_once(config, store, pool, now).await
}

#[instrument(level = "info", skip(config, store, pool))]
pub async fn reconcile_all_sources_once(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    now: OffsetDateTime,
) -> Result<Vec<ReconcileResult>> {
    let mut results = Vec::with_capacity(config.sources.len());
    for source in &config.sources {
        let result = reconcile_inbound_source(config, store, pool, source, now).await?;
        results.push(result);
    }
    Ok(results)
}

#[instrument(level = "info", skip(config, store, pool, source))]
pub async fn reconcile_inbound_source(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    source: &InboundSource,
    now: OffsetDateTime,
) -> Result<ReconcileResult> {
    reconcile_inbound_source_with_wait(
        config,
        store,
        pool,
        source,
        now,
        std::time::Duration::from_secs(30),
        false,
    )
    .await
}

pub(crate) async fn reconcile_inbound_source_with_wait(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    source: &InboundSource,
    now: OffsetDateTime,
    wait: std::time::Duration,
    bounded_turn: bool,
) -> Result<ReconcileResult> {
    loop {
        match reconcile_inbound_source_turn(config, store, pool, source, now, wait, bounded_turn)
            .await?
        {
            ScanTurnOutcome::Complete(result) => return Ok(result),
            ScanTurnOutcome::Reobserve if bounded_turn => {
                return Err(IngestionError::Persistence(
                    melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(
                        source.name.clone(),
                    ),
                ));
            }
            ScanTurnOutcome::Reobserve => {}
        }
    }
}

enum ScanTurnOutcome {
    Complete(ReconcileResult),
    Reobserve,
}

async fn reconcile_inbound_source_turn(
    config: &IngestionConfig,
    store: &WorkItemStore,
    pool: &PgPool,
    source: &InboundSource,
    now: OffsetDateTime,
    wait: std::time::Duration,
    bounded_turn: bool,
) -> Result<ScanTurnOutcome> {
    let started = Instant::now();
    let mut stats = ReconcileStats::default();

    let task_source = source.clone();
    let root = crate::admission::run(move || crate::source::GuardedRoot::open(task_source))
        .await
        .map_err(IngestionError::InspectionTask)??;
    #[cfg(feature = "test-support")]
    let root = {
        let mut root = root;
        root.set_test_hook(config.discovery_resolve_test_hook.clone());
        root
    };
    let fence = discovery_scan::acquire(pool, source, wait).await?;
    while let Some((partition, replay)) =
        discovery_scan::next_partition(pool, source, &fence, bounded_turn).await?
    {
        let partition_errors = stats.errors;
        let reader_root = root.clone();
        let mut reader =
            crate::admission::run(move || crate::source::CandidateReader::open(reader_root))
                .await
                .map_err(IngestionError::InspectionTask)??;
        loop {
            discovery_scan::renew(pool, &fence).await?;
            let verify_root = root.clone();
            crate::admission::run(move || verify_root.verify_current())
                .await
                .map_err(IngestionError::InspectionTask)??;
            let (next, candidates, finished) =
                crate::admission::run(move || reader.next_batch(partition))
                    .await
                    .map_err(IngestionError::InspectionTask)??;
            reader = next;
            for name in candidates {
                let verify_root = root.clone();
                crate::admission::run(move || verify_root.verify_current())
                    .await
                    .map_err(IngestionError::InspectionTask)??;
                let candidate_root = root.clone();
                let Some(candidate) = crate::admission::run(move || candidate_root.resolve(&name))
                    .await
                    .map_err(IngestionError::InspectionTask)??
                else {
                    continue;
                };
                let relative = candidate_relative_path(&candidate.relative_path);
                stats.candidates_seen += 1;
                let legacy_relative =
                    crate::source::legacy_candidate_relative_path(&candidate.relative_path);
                if legacy_relative.contains('\u{fffd}') {
                    // A pre-upgrade lossy row may represent one of several
                    // byte-distinct paths. Preserve its observation until an
                    // operator resolves the historical ambiguity.
                    discovery_scan::mark_legacy_seen(
                        pool,
                        source,
                        &fence,
                        &legacy_relative,
                        partition,
                    )
                    .await?;
                }
                discovery_scan::mark_seen(pool, source, &fence, &relative, partition).await?;
                let previous_errors = stats.errors;
                'candidate: {
                    let generation_root = root.clone();
                    let generation_candidate = candidate.clone();
                    let generation = match crate::admission::run(move || {
                        generation_root.generation(&generation_candidate)
                    })
                    .await
                    .map_err(IngestionError::InspectionTask)?
                    {
                        Ok(generation) => generation,
                        Err(error) => {
                            stats.errors += 1;
                            warn!(source = %source.name, source_path = %relative, %error, "unable to identify inbound occurrence");
                            break 'candidate;
                        }
                    };
                    match store
                        .inbound_occurrence_is_acquired_fenced(
                            pool,
                            source.id,
                            &relative,
                            &generation,
                            now,
                            Some(&fence),
                        )
                        .await
                    {
                        Ok(true) => break 'candidate,
                        Ok(false) => {}
                        Err(error) => {
                            stats.errors += 1;
                            warn!(source = %source.name, source_path = %relative, %error, "unable to resolve inbound occurrence");
                            break 'candidate;
                        }
                    }

                    let item = match get_or_create_work_item(
                        store, pool, source.id, &relative, now, &fence,
                    )
                    .await
                    {
                        Ok((item, created)) => {
                            if created {
                                stats.new_work_items += 1;
                            } else {
                                stats.existing_work_items += 1;
                            }
                            item
                        }
                        Err(error) => {
                            stats.errors += 1;
                            warn!(
                                source = %source.name,
                                source_path = %relative,
                                error = %error,
                                "unable to create durable work item"
                            );
                            break 'candidate;
                        }
                    };

                    // A newly created occurrence must be marked too; the first mark
                    // above can only update an already-existing active work item.
                    let done =
                        discovery_scan::mark_seen(pool, source, &fence, &relative, partition)
                            .await?;
                    let (snapshot, sealed_snapshot) = match capture_staged_snapshot(
                        store,
                        pool,
                        item.id,
                        now,
                        &fence,
                        root.clone(),
                        candidate.clone(),
                        config.ready_marker_name.clone(),
                        #[cfg(feature = "test-support")]
                        config.snapshot_stall_test_hook.clone(),
                    )
                    .await
                    {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            stats.errors += 1;
                            warn!(
                                source = %source.name,
                                source_path = %relative,
                                error = %error,
                                "snapshot capture failed"
                            );
                            store
                                .upsert_settling_progress_fenced(
                                    pool,
                                    item.id,
                                    item.stable_since,
                                    item.latest_settling_snapshot_id,
                                    now,
                                    Some("snapshot-error"),
                                    Some(&fence),
                                )
                                .await
                                .map_err(IngestionError::Persistence)?;
                            break 'candidate;
                        }
                    };
                    let staged_snapshot_id = sealed_snapshot.snapshot_id();

                    let verify_root = root.clone();
                    let verify_candidate = candidate;
                    crate::admission::run(move || {
                        verify_root.verify_candidate(&verify_candidate)?;
                        verify_root.verify_current()
                    })
                    .await
                    .map_err(IngestionError::InspectionTask)??;

                    let latest_snapshot = match store.latest_snapshot(pool, item.id).await {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            stats.errors += 1;
                            warn!(
                                source = %source.name,
                                source_path = %relative,
                                error = %error,
                                "latest snapshot lookup failed"
                            );
                            break 'candidate;
                        }
                    };

                    let signature_changed = latest_snapshot.as_ref().is_none_or(|current| {
                        !crate::snapshot::signatures_equal(&current.signature, &snapshot.signature)
                    });
                    if replay
                        && done
                        && !signature_changed
                        && item.state_reason.as_deref() != Some("source-missing")
                    {
                        store
                            .mark_staged_snapshot_discarding(
                                pool,
                                staged_snapshot_id,
                                item.id,
                                &fence,
                            )
                            .await?;
                        break 'candidate;
                    }

                    let decision = decide_settling_transition(
                        item.state,
                        signature_changed,
                        now,
                        item.stable_since,
                        &snapshot,
                        settling_period_for_snapshot(config, &snapshot),
                    );
                    let next_state = decision.next_state.filter(|state| *state != item.state);
                    let resulting_state = next_state.unwrap_or(item.state);
                    let state_reason = if signature_changed {
                        if snapshot.ready_marker_seen {
                            "ready-marker"
                        } else {
                            "snapshot-changed"
                        }
                    } else {
                        match resulting_state {
                            IngestionState::Discovered => "discovered-initial",
                            IngestionState::Settling => "settling-observed",
                            IngestionState::Ready => "already-ready",
                            _ => decision.reason,
                        }
                    };
                    let staged = if signature_changed {
                        Some(StagedSnapshotCompletion {
                            verified: &sealed_snapshot,
                            ready_marker_seen: snapshot.ready_marker_seen,
                            bypassed_settling: snapshot.bypassed_settling,
                        })
                    } else {
                        store
                            .mark_staged_snapshot_discarding(
                                pool,
                                staged_snapshot_id,
                                item.id,
                                &fence,
                            )
                            .await?;
                        None
                    };
                    let observation = SettlingObservation {
                        expected: &item,
                        staged,
                        next_state,
                        transition_reason: decision.reason,
                        state_reason,
                        stable_since: if signature_changed {
                            Some(now)
                        } else {
                            item.stable_since
                        },
                        latest_snapshot_id: if signature_changed {
                            Some(staged_snapshot_id)
                        } else {
                            latest_snapshot.as_ref().map(|current| current.id)
                        },
                        observed_at: now,
                    };
                    if let Err(error) = store
                        .apply_settling_observation_fenced(pool, &observation, &fence)
                        .await
                    {
                        stats.errors += 1;
                        warn!(source = %source.name, source_path = %relative, %error,
                            "settling observation could not be committed");
                        break 'candidate;
                    }
                    if signature_changed {
                        stats.snapshot_changed += 1;
                        stats.settling_updated += 1;
                        if item.state == IngestionState::Ready {
                            stats.ready_reverted += 1;
                        }
                    }
                    if next_state == Some(IngestionState::Ready) {
                        stats.ready_promoted += 1;
                    }
                }
                if stats.errors == previous_errors {
                    discovery_scan::mark_done(pool, source, &fence, &relative, partition).await?;
                }
            }
            if finished {
                break;
            }
        }
        let deferred = discovery_scan::finish_partition(
            pool,
            &fence,
            partition,
            stats.errors != partition_errors,
        )
        .await?;
        if deferred {
            return Err(IngestionError::Persistence(
                melodee_persistence::WorkStoreError::DiscoveryScanIncomplete(source.name.clone()),
            ));
        }
    }

    let verify_root = root.clone();
    crate::admission::run(move || verify_root.verify_current())
        .await
        .map_err(IngestionError::InspectionTask)??;
    match discovery_scan::mark_missing(pool, source, &fence, &root, now, bounded_turn).await? {
        discovery_scan::MissingOutcome::Complete(count) => stats.missing_candidates = count,
        discovery_scan::MissingOutcome::Reobserve => return Ok(ScanTurnOutcome::Reobserve),
    }
    discovery_scan::finish(pool, &fence).await?;

    stats.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);

    debug!(
        source = %source.name,
        source_path = %source.root.display(),
        candidates_seen = %stats.candidates_seen,
        new_items = %stats.new_work_items,
        existing_items = %stats.existing_work_items,
        ready_promoted = %stats.ready_promoted,
        duration_ms = %stats.duration_ms
    );

    Ok(ScanTurnOutcome::Complete(ReconcileResult {
        source: source.name.clone(),
        stats,
    }))
}

fn decide_settling_transition(
    state: IngestionState,
    snapshot_changed: bool,
    now: OffsetDateTime,
    stable_since: Option<OffsetDateTime>,
    snapshot: &CandidateSnapshot,
    settling_period: Duration,
) -> SettlingDecision {
    if snapshot_changed {
        return SettlingDecision {
            next_state: Some(match state {
                IngestionState::Ready => IngestionState::Settling,
                _ => IngestionState::Settling,
            }),
            reason: if snapshot.ready_marker_seen {
                "ready-marker-reset"
            } else {
                "snapshot-changed"
            },
        };
    }

    if state == IngestionState::Discovered {
        return SettlingDecision {
            next_state: Some(IngestionState::Settling),
            reason: "discovered-initial",
        };
    }

    if state == IngestionState::Settling {
        let stable_point = stable_since.unwrap_or(now);
        if now - stable_point >= settling_period {
            return SettlingDecision {
                next_state: Some(IngestionState::Ready),
                reason: "settling-complete",
            };
        }

        return SettlingDecision {
            next_state: None,
            reason: "settling-observed",
        };
    }

    SettlingDecision {
        next_state: None,
        reason: if state == IngestionState::Ready {
            "already-ready"
        } else {
            "unsupported-state"
        },
    }
}

// Process-owned threads supervise cancelled producers independently of Tokio
// runtime shutdown. Capacity is reserved before any producer is started.
struct SnapshotReaperJob {
    thread: std::thread::JoinHandle<Result<crate::snapshot::StreamSnapshotSummary>>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
struct SnapshotSupervisor {
    capacity: std::sync::Arc<tokio::sync::Semaphore>,
    sender: std::sync::mpsc::SyncSender<SnapshotReaperJob>,
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    _threads: Vec<std::thread::JoinHandle<()>>,
}
static SNAPSHOT_SUPERVISOR: std::sync::OnceLock<std::result::Result<SnapshotSupervisor, String>> =
    std::sync::OnceLock::new();
// A failed supervisor is an internal fault. Retain ownership rather than
// detaching the affected producer, even if its syscall never returns.
static FAILED_REAPER_JOBS: std::sync::OnceLock<std::sync::Mutex<Vec<SnapshotReaperJob>>> =
    std::sync::OnceLock::new();

fn snapshot_supervisor() -> Result<&'static SnapshotSupervisor> {
    SNAPSHOT_SUPERVISOR
        .get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<SnapshotReaperJob>(4);
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
            let mut threads = Vec::with_capacity(4);
            for index in 0..4 {
                let thread_pending = pending.clone();
                let receiver = receiver.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("melodee-snapshot-reaper-{index}"))
                    .spawn(move || {
                        loop {
                            let job = receiver
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .recv();
                            if let Ok(job) = job {
                                let SnapshotReaperJob { thread, _permit } = job;
                                let _ = thread.join();
                                drop(_permit);
                                thread_pending.fetch_sub(1, std::sync::atomic::Ordering::Release);
                            } else {
                                break;
                            }
                        }
                    });
                match spawned {
                    Ok(thread) => threads.push(thread),
                    Err(error) => {
                        drop(sender);
                        for thread in threads {
                            let _ = thread.join();
                        }
                        return Err(error.to_string());
                    }
                }
            }
            Ok(SnapshotSupervisor {
                capacity: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
                sender,
                pending,
                _threads: threads,
            })
        })
        .as_ref()
        .map_err(|error| IngestionError::InspectionTask(error.clone()))
}

struct SnapshotProducer {
    receiver: Option<tokio::sync::mpsc::Receiver<SettlingSnapshotFile>>,
    thread: Option<std::thread::JoinHandle<Result<crate::snapshot::StreamSnapshotSummary>>>,
    reaper_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl SnapshotProducer {
    fn join(mut self) -> Result<crate::snapshot::StreamSnapshotSummary> {
        self.receiver.take();
        self.thread
            .take()
            .ok_or_else(|| IngestionError::InspectionTask("snapshot producer missing".to_owned()))?
            .join()
            .map_err(|_| IngestionError::InspectionTask("snapshot producer panicked".to_owned()))?
    }
}

impl Drop for SnapshotProducer {
    fn drop(&mut self) {
        self.receiver.take();
        if let (Some(thread), Some(permit)) = (self.thread.take(), self.reaper_permit.take()) {
            let job = SnapshotReaperJob {
                thread,
                _permit: permit,
            };
            if let Ok(supervisor) = snapshot_supervisor() {
                supervisor
                    .pending
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
                if let Err(error) = supervisor.sender.try_send(job) {
                    supervisor
                        .pending
                        .fetch_sub(1, std::sync::atomic::Ordering::Release);
                    tracing::error!("snapshot reaper unavailable; retaining producer ownership");
                    let job = match error {
                        std::sync::mpsc::TrySendError::Full(job)
                        | std::sync::mpsc::TrySendError::Disconnected(job) => job,
                    };
                    FAILED_REAPER_JOBS
                        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(job);
                }
            } else {
                tracing::error!("snapshot reaper unavailable; retaining producer ownership");
                FAILED_REAPER_JOBS
                    .get_or_init(|| std::sync::Mutex::new(Vec::new()))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(job);
            }
        }
    }
}

#[cfg(test)]
mod producer_tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn cancellation_reaps_stalled_producer_without_blocking_executor() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.expect("permit");
        let reaper_permit = snapshot_supervisor()
            .expect("supervisor")
            .capacity
            .clone()
            .acquire_owned()
            .await
            .expect("reaper permit");
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let ended = Arc::new(AtomicBool::new(false));
        let ended_by_thread = ended.clone();
        let thread = std::thread::spawn(move || {
            let _permit = permit;
            ready_tx.send(()).expect("ready");
            release_rx.recv().expect("release filesystem stall");
            assert!(sender.is_closed(), "cancelled producer cannot publish");
            ended_by_thread.store(true, Ordering::SeqCst);
            Ok(crate::snapshot::StreamSnapshotSummary {
                file_count: 0,
                total_bytes: 0,
                ready_marker_seen: false,
            })
        });
        ready_rx.recv().expect("producer stalled");
        let producer = SnapshotProducer {
            receiver: Some(receiver),
            thread: Some(thread),
            reaper_permit: Some(reaper_permit),
        };
        let started = std::time::Instant::now();
        drop(producer);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!ended.load(Ordering::SeqCst));
        assert_eq!(semaphore.available_permits(), 0);
        assert!(
            snapshot_supervisor()
                .expect("supervisor")
                .pending
                .load(Ordering::Acquire)
                > 0
        );
        let other_capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let other_permit = other_capacity
            .clone()
            .acquire_owned()
            .await
            .expect("other permit");
        let other_reaper = snapshot_supervisor()
            .expect("supervisor")
            .capacity
            .clone()
            .acquire_owned()
            .await
            .expect("other reaper permit");
        let (_other_sender, other_receiver) = tokio::sync::mpsc::channel(1);
        let other_thread = std::thread::spawn(move || {
            let _permit = other_permit;
            Ok(crate::snapshot::StreamSnapshotSummary {
                file_count: 0,
                total_bytes: 0,
                ready_marker_seen: false,
            })
        });
        drop(SnapshotProducer {
            receiver: Some(other_receiver),
            thread: Some(other_thread),
            reaper_permit: Some(other_reaper),
        });
        let other = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            other_capacity.acquire_owned(),
        )
        .await
        .expect("other producer is not starved")
        .expect("permit");
        drop(other);
        assert!(!ended.load(Ordering::SeqCst));
        release_tx.send(()).expect("release");
        let permit =
            tokio::time::timeout(std::time::Duration::from_secs(5), semaphore.acquire_owned())
                .await
                .expect("reaped")
                .expect("permit");
        drop(permit);
        assert!(ended.load(Ordering::SeqCst));
    }

    #[test]
    fn runtime_teardown_does_not_detach_a_stalled_producer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let permit = runtime
            .block_on(
                snapshot_supervisor()
                    .expect("supervisor")
                    .capacity
                    .clone()
                    .acquire_owned(),
            )
            .expect("permit");
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let ended = Arc::new(AtomicBool::new(false));
        let thread_ended = ended.clone();
        let thread = std::thread::spawn(move || {
            ready_tx.send(()).expect("ready");
            release_rx.recv().expect("release");
            assert!(sender.is_closed());
            thread_ended.store(true, Ordering::Release);
            Ok(crate::snapshot::StreamSnapshotSummary {
                file_count: 0,
                total_bytes: 0,
                ready_marker_seen: false,
            })
        });
        ready_rx.recv().expect("started");
        drop(SnapshotProducer {
            receiver: Some(receiver),
            thread: Some(thread),
            reaper_permit: Some(permit),
        });
        let started = std::time::Instant::now();
        drop(runtime);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!ended.load(Ordering::Acquire));
        assert!(
            snapshot_supervisor()
                .expect("supervisor")
                .pending
                .load(Ordering::Acquire)
                > 0
        );
        release_tx.send(()).expect("release");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ended.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ended.load(Ordering::Acquire));
    }
}

#[allow(clippy::too_many_arguments)]
async fn capture_staged_snapshot(
    store: &WorkItemStore,
    pool: &PgPool,
    item: Uuid,
    now: OffsetDateTime,
    fence: &melodee_persistence::DiscoveryScanFence,
    root: crate::source::GuardedRoot,
    candidate: crate::source::GuardedCandidate,
    marker: String,
    #[cfg(feature = "test-support")] test_hook: Option<SnapshotStallTestHook>,
) -> Result<(
    CandidateSnapshot,
    melodee_persistence::VerifiedStagedSnapshot,
)> {
    let staged = loop {
        let staged = store.begin_staged_snapshot(pool, item, now, fence).await?;
        if staged.newly_created {
            break staged;
        }
        store
            .mark_staged_snapshot_discarding(pool, staged.snapshot_id, staged.work_item_id, fence)
            .await?;
    };
    let reaper_permit = snapshot_supervisor()?
        .capacity
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    let permit = crate::admission::heavy_ingestion()
        .await
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    let (sender, receiver) = tokio::sync::mpsc::channel::<SettlingSnapshotFile>(128);
    let thread = std::thread::Builder::new()
        .name("melodee-snapshot".to_owned())
        .spawn(move || {
            let _permit = permit;
            #[cfg(feature = "test-support")]
            if let Some(hook) = &test_hook {
                hook.pause();
            }
            let marker_seen = root.ready_marker_seen(&candidate, &marker);
            let directory = root
                .cap_directory(&candidate)
                .map_err(IngestionError::Source)?;
            let cancellation = sender.clone();
            let result = crate::snapshot::stream_release_snapshot(
                &directory,
                marker_seen,
                || cancellation.is_closed(),
                |file| {
                    sender
                        .blocking_send(SettlingSnapshotFile {
                            relative_path: file.relative_path,
                            file_type: file.file_type,
                            file_size: file.file_size,
                            modified_at: file.modified_at,
                        })
                        .map_err(|_| crate::snapshot::SnapshotCaptureError::Capture {
                            path: std::path::PathBuf::new(),
                            message: "snapshot consumer stopped".to_owned(),
                        })
                },
            )
            .map_err(IngestionError::Snapshot);
            #[cfg(feature = "test-support")]
            if let Some(hook) = &test_hook {
                hook.finished
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            result
        })
        .map_err(|error| IngestionError::InspectionTask(error.to_string()))?;
    let mut producer = SnapshotProducer {
        receiver: Some(receiver),
        thread: Some(thread),
        reaper_permit: Some(reaper_permit),
    };
    let mut batch = Vec::with_capacity(128);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        let receiver = producer.receiver.as_mut().ok_or_else(|| {
            IngestionError::InspectionTask("snapshot receiver missing".to_owned())
        })?;
        let next = tokio::select! {
            file = receiver.recv() => file,
            _ = heartbeat.tick() => {
                discovery_scan::renew(pool, fence).await?;
                continue;
            }
        };
        let Some(file) = next else {
            break;
        };
        batch.push(file);
        if batch.len() == 128 {
            discovery_scan::renew(pool, fence).await?;
            store
                .append_staged_snapshot_files(pool, staged.snapshot_id, item, &batch, fence)
                .await?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        discovery_scan::renew(pool, fence).await?;
        store
            .append_staged_snapshot_files(pool, staged.snapshot_id, item, &batch, fence)
            .await?;
    }
    let summary = producer.join()?;
    let verified = store
        .seal_staged_snapshot(pool, staged.snapshot_id, item, fence)
        .await?;
    if i64::from(verified.file_count()) != summary.file_count
        || verified.total_bytes() != summary.total_bytes
    {
        return Err(IngestionError::Persistence(
            melodee_persistence::WorkStoreError::InvalidRequest(
                "settling capture differs from observed filesystem entries",
            ),
        ));
    }
    Ok((
        CandidateSnapshot {
            signature: verified.signature().to_owned(),
            file_count: summary.file_count,
            total_bytes: summary.total_bytes,
            files: Vec::new(),
            ready_marker_seen: summary.ready_marker_seen,
            bypassed_settling: false,
        },
        verified,
    ))
}

fn settling_period_for_snapshot(
    config: &IngestionConfig,
    snapshot: &CandidateSnapshot,
) -> Duration {
    if !snapshot.ready_marker_seen {
        return config.settling_quiet_period;
    }

    if config.ready_marker_shortcut == Duration::ZERO {
        Duration::ZERO
    } else if config.ready_marker_shortcut < config.settling_quiet_period {
        config.ready_marker_shortcut
    } else {
        config.settling_quiet_period
    }
}

async fn get_or_create_work_item(
    store: &WorkItemStore,
    pool: &PgPool,
    source_library_id: Option<Uuid>,
    source_path: &str,
    now: OffsetDateTime,
    fence: &melodee_persistence::DiscoveryScanFence,
) -> Result<(melodee_domain::ReleaseWorkItem, bool)> {
    let discovered = melodee_domain::ReleaseWorkItem::discovered(
        source_path.to_owned(),
        source_library_id,
        Uuid::now_v7(),
        now,
    );
    let item = store
        .create_or_get_fenced(pool, &discovered, Some(fence))
        .await
        .map_err(IngestionError::Persistence)?;
    Ok(item)
}

#[derive(Debug)]
struct SettlingDecision {
    next_state: Option<melodee_domain::IngestionState>,
    reason: &'static str,
}
