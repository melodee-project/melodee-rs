//! Discovery and settling reconciliation.

use std::collections::HashSet;
use std::time::Instant;

use melodee_domain::IngestionState;
use melodee_persistence::{SettlingSnapshotFile, WorkItemStore};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tracing::{debug, instrument, warn};
use uuid::Uuid;

use crate::{
    candidate_relative_path,
    config::InboundSource,
    config::IngestionConfig,
    error::{IngestionError, Result},
    snapshot::{CandidateSnapshot, snapshot_release_directory},
    source::discover_release_candidates,
};

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
    let started = Instant::now();
    let mut stats = ReconcileStats::default();
    let mut observed = HashSet::<String>::new();

    let candidates = match discover_release_candidates(source) {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(
                source = %source.name,
                source_root = %source.root.display(),
                error = %error,
                "source discovery failed"
            );
            return Err(IngestionError::Source(error));
        }
    };

    for candidate in candidates {
        let relative = candidate_relative_path(&candidate.relative_path);
        observed.insert(relative.clone());
        stats.candidates_seen += 1;

        let mut item = match get_or_create_work_item(store, pool, source.id, &relative, now).await {
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
                continue;
            }
        };

        let snapshot =
            match snapshot_release_directory(&candidate.absolute_path, &config.ready_marker_name) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    stats.errors += 1;
                    warn!(
                        source = %source.name,
                        source_path = %relative,
                        error = %error,
                        "snapshot capture failed"
                    );
                    let _ = store
                        .upsert_settling_progress(
                            pool,
                            item.id,
                            item.stable_since,
                            item.latest_settling_snapshot_id,
                            now,
                            Some("snapshot-error"),
                        )
                        .await;
                    continue;
                }
            };

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
                continue;
            }
        };

        let signature_changed = latest_snapshot
            .as_ref()
            .is_none_or(|current| current.signature != snapshot.signature);

        let mut snapshot_id = latest_snapshot.as_ref().map(|current| current.id);
        let mut bypassed_settling = latest_snapshot
            .as_ref()
            .is_none_or(|current| current.bypassed_settling);

        if signature_changed {
            stats.snapshot_changed += 1;
            let files = snapshot_rows(&snapshot.files);
            match persist_snapshot(&snapshot, pool, &item, now, files).await {
                Ok(id) => {
                    stats.settling_updated += 1;
                    snapshot_id = Some(id);
                    bypassed_settling = snapshot.ready_marker_seen
                        && settling_period_for_snapshot(config, &snapshot) == Duration::ZERO;
                }
                Err(error) => {
                    stats.errors += 1;
                    warn!(
                        source = %source.name,
                        source_path = %relative,
                        error = %error,
                        "snapshot persistence failed"
                    );
                    continue;
                }
            }
        }

        if !snapshot.ready_marker_seen && !bypassed_settling {
            // Keep previous marker bypass choice unless we took a fresh snapshot.
            bypassed_settling = latest_snapshot
                .as_ref()
                .map(|previous| previous.bypassed_settling)
                .unwrap_or(bypassed_settling);
        }

        let decision = decide_settling_transition(
            item.state,
            signature_changed,
            now,
            item.stable_since,
            &snapshot,
            settling_period_for_snapshot(config, &snapshot),
        );

        if item.state == IngestionState::Ready && signature_changed {
            stats.ready_reverted += 1;
        }

        if let Some(next_state) = decision.next_state
            && item.state != next_state
        {
            match store
                .system_transition(pool, item.id, next_state, Some(decision.reason), now)
                .await
            {
                Ok(updated) => {
                    item = updated;
                    if item.state == IngestionState::Ready {
                        stats.ready_promoted += 1;
                    }
                }
                Err(error) => {
                    stats.errors += 1;
                    warn!(
                        work_item_id = %item.id,
                        source = %source.name,
                        source_path = %relative,
                        from_state = %item.state.as_str(),
                        to_state = %next_state.as_str(),
                        error = %error,
                        "settling transition race while reconciling"
                    );
                }
            }
        }

        if signature_changed {
            let state_reason = if snapshot.ready_marker_seen {
                "ready-marker"
            } else {
                "snapshot-changed"
            };
            let _ = store
                .upsert_settling_progress(
                    pool,
                    item.id,
                    Some(now),
                    snapshot_id,
                    now,
                    Some(state_reason),
                )
                .await;
        } else if item.state == IngestionState::Discovered {
            let _ = store
                .upsert_settling_progress(
                    pool,
                    item.id,
                    item.stable_since,
                    snapshot_id,
                    now,
                    Some("discovered-initial"),
                )
                .await;
        } else if item.state == IngestionState::Settling {
            let _ = store
                .upsert_settling_progress(
                    pool,
                    item.id,
                    item.stable_since,
                    snapshot_id,
                    now,
                    Some("settling-observed"),
                )
                .await;
        } else if item.state == IngestionState::Ready {
            let _ = store
                .upsert_settling_progress(
                    pool,
                    item.id,
                    item.stable_since,
                    snapshot_id,
                    now,
                    Some("already-ready"),
                )
                .await;
        } else {
            let _ = store
                .upsert_settling_progress(
                    pool,
                    item.id,
                    item.stable_since,
                    snapshot_id,
                    now,
                    Some(decision.reason),
                )
                .await;
        }
    }

    match store.list_active_for_reconciliation(pool).await {
        Ok(items) => {
            for item in items {
                if !is_same_source(&item.source_library_id, source.id) {
                    continue;
                }
                if !observed.contains(&item.source_path) {
                    stats.missing_candidates += 1;
                    let _ = store
                        .upsert_settling_progress(
                            pool,
                            item.id,
                            item.stable_since,
                            item.latest_settling_snapshot_id,
                            now,
                            Some("source-missing"),
                        )
                        .await;
                }
            }
        }
        Err(error) => {
            stats.errors += 1;
            warn!(
                source = %source.name,
                error = %error,
                "listing active reconciliation items failed"
            );
        }
    }

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

    Ok(ReconcileResult {
        source: source.name.clone(),
        stats,
    })
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

fn is_same_source(item_library_id: &Option<Uuid>, source_library_id: Option<Uuid>) -> bool {
    match (item_library_id, source_library_id) {
        (None, None) => true,
        (Some(a), Some(b)) => *a == b,
        _ => false,
    }
}

fn snapshot_rows(files: &[crate::snapshot::SnapshotFile]) -> Vec<SettlingSnapshotFile> {
    files
        .iter()
        .map(|file| SettlingSnapshotFile {
            relative_path: file.relative_path.clone(),
            file_type: file.file_type.clone(),
            file_size: file.file_size,
            modified_at: file.modified_at,
        })
        .collect()
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

async fn persist_snapshot(
    snapshot: &CandidateSnapshot,
    pool: &PgPool,
    item: &melodee_domain::ReleaseWorkItem,
    now: OffsetDateTime,
    files: Vec<SettlingSnapshotFile>,
) -> Result<i64> {
    let file_count = i32::try_from(snapshot.file_count).map_err(|_| {
        IngestionError::Persistence(melodee_persistence::WorkStoreError::CorruptState(format!(
            "candidate file_count too large for i32: {}",
            snapshot.file_count,
        )))
    })?;

    WorkItemStore
        .persist_settling_snapshot(
            pool,
            item.id,
            &snapshot.signature,
            file_count,
            snapshot.total_bytes,
            snapshot.ready_marker_seen,
            snapshot.bypassed_settling,
            now,
            &files,
        )
        .await
        .map_err(IngestionError::Persistence)
}

async fn get_or_create_work_item(
    store: &WorkItemStore,
    pool: &PgPool,
    source_library_id: Option<Uuid>,
    source_path: &str,
    now: OffsetDateTime,
) -> Result<(melodee_domain::ReleaseWorkItem, bool)> {
    let discovered = melodee_domain::ReleaseWorkItem::discovered(
        source_path.to_owned(),
        source_library_id,
        Uuid::now_v7(),
        now,
    );
    let item = store
        .create_or_get(pool, &discovered)
        .await
        .map_err(IngestionError::Persistence)?;
    Ok(item)
}

#[derive(Debug)]
struct SettlingDecision {
    next_state: Option<melodee_domain::IngestionState>,
    reason: &'static str,
}
