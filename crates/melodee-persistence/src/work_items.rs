//! Durable Release work-item store: transactional claiming, leases, recovery,
//! retries, failure evidence, and append-only history.
//!
//! Concurrency model (PRD-INGEST-002, SPEC 11.5): PostgreSQL row locking with
//! `FOR UPDATE SKIP LOCKED`; every mutation and its history event commit in
//! the same transaction, so a crash never leaves a half-applied claim or an
//! untracked state change.
//!
//! Time is always supplied by the caller (`now: OffsetDateTime`) — no hidden
//! clocks in this module (testable, deterministic).

use melodee_domain::{
    FailureClass, IngestionState, ReleaseManifest, ReleaseWorkItem, ReleaseWorkItemEvent,
    RetryPolicy, TransitionKind, WorkErrorCode, WorkEventType,
};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::config::WorkEngineConfig;
use crate::error::WorkStoreError;
use crate::worker::WorkerId;

/// Per-state row counts for operator diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCount {
    pub state: IngestionState,
    pub count: i64,
}

#[derive(Debug, FromRow)]
struct WorkItemRow {
    id: Uuid,
    api_key: Uuid,
    source_library_id: Option<Uuid>,
    target_library_id: Option<Uuid>,
    source_path: String,
    current_path: String,
    target_path: Option<String>,
    source_fingerprint: Option<String>,
    manifest_hash: Option<String>,
    state: String,
    state_reason: Option<String>,
    attempt_count: i32,
    next_attempt_at: Option<OffsetDateTime>,
    lease_owner: Option<String>,
    lease_acquired_at: Option<OffsetDateTime>,
    lease_expires_at: Option<OffsetDateTime>,
    latest_settling_snapshot_id: Option<i64>,
    stable_since: Option<OffsetDateTime>,
    last_observed_at: Option<OffsetDateTime>,
    last_error_code: Option<String>,
    last_error_message: Option<String>,
    correlation_id: Uuid,
    discovered_at: OffsetDateTime,
    first_started_at: Option<OffsetDateTime>,
    last_attempt_at: Option<OffsetDateTime>,
    updated_at: OffsetDateTime,
    created_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

impl TryFrom<WorkItemRow> for ReleaseWorkItem {
    type Error = WorkStoreError;

    fn try_from(row: WorkItemRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            api_key: row.api_key,
            source_library_id: row.source_library_id,
            target_library_id: row.target_library_id,
            source_path: row.source_path,
            current_path: row.current_path,
            target_path: row.target_path,
            source_fingerprint: row.source_fingerprint,
            manifest_hash: row.manifest_hash,
            state: IngestionState::parse(&row.state)
                .ok_or_else(|| WorkStoreError::CorruptState(row.state.clone()))?,
            state_reason: row.state_reason,
            attempt_count: u32::try_from(row.attempt_count).map_err(|_| {
                WorkStoreError::CorruptState(format!("negative attempt_count on {}", row.id))
            })?,
            next_attempt_at: row.next_attempt_at,
            lease_owner: row.lease_owner,
            lease_acquired_at: row.lease_acquired_at,
            lease_expires_at: row.lease_expires_at,
            latest_settling_snapshot_id: row.latest_settling_snapshot_id,
            stable_since: row.stable_since,
            last_observed_at: row.last_observed_at,
            last_error_code: row.last_error_code.map(WorkErrorCode::new),
            last_error_message: row.last_error_message,
            correlation_id: row.correlation_id,
            discovered_at: row.discovered_at,
            first_started_at: row.first_started_at,
            last_attempt_at: row.last_attempt_at,
            updated_at: row.updated_at,
            created_at: row.created_at,
            completed_at: row.completed_at,
        })
    }
}

#[derive(Debug, FromRow)]
struct EventRow {
    id: i64,
    work_item_id: Uuid,
    event_type: String,
    from_state: Option<String>,
    to_state: Option<String>,
    reason_code: Option<String>,
    worker_id: Option<String>,
    correlation_id: Uuid,
    created_at: OffsetDateTime,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, FromRow)]
struct SettlingSnapshotRow {
    id: i64,
    work_item_id: Uuid,
    observed_at: OffsetDateTime,
    signature: String,
    file_count: i32,
    total_bytes: i64,
    ready_marker_seen: bool,
    bypassed_settling: bool,
}

#[derive(Debug, Clone)]
pub struct SettlingSnapshot {
    pub id: i64,
    pub work_item_id: Uuid,
    pub observed_at: OffsetDateTime,
    pub signature: String,
    pub file_count: i32,
    pub total_bytes: i64,
    pub ready_marker_seen: bool,
    pub bypassed_settling: bool,
}

#[derive(Debug, Clone)]
pub struct SettlingSnapshotFile {
    pub relative_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub modified_at: OffsetDateTime,
}

/// Durable manifest payload and integrity identity published for one work item.
#[derive(Debug, Clone)]
pub struct StoredManifest {
    pub id: i64,
    pub work_item_id: Uuid,
    pub schema_version: i32,
    pub manifest_hash: String,
    pub payload: serde_json::Value,
    pub source_snapshot_id: Option<i64>,
    pub source_signature: Option<String>,
    pub generated_at: OffsetDateTime,
}

impl TryFrom<EventRow> for ReleaseWorkItemEvent {
    type Error = WorkStoreError;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            work_item_id: row.work_item_id,
            event_type: WorkEventType::parse(&row.event_type)
                .ok_or_else(|| WorkStoreError::CorruptState(row.event_type.clone()))?,
            from_state: row
                .from_state
                .map(|s| IngestionState::parse(&s).ok_or(WorkStoreError::CorruptState(s)))
                .transpose()?,
            to_state: row
                .to_state
                .map(|s| IngestionState::parse(&s).ok_or(WorkStoreError::CorruptState(s)))
                .transpose()?,
            reason_code: row.reason_code,
            worker_id: row.worker_id,
            correlation_id: row.correlation_id,
            created_at: row.created_at,
            metadata: row.metadata,
        })
    }
}

/// The work-item repository. Stateless; every method takes its dependencies
/// explicitly. All methods are async over a shared `PgPool`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkItemStore;

impl WorkItemStore {
    /// Persists a newly discovered work item plus its `created` event,
    /// atomically.
    #[instrument(skip_all, fields(work_item_id = %item.id, correlation_id = %item.correlation_id))]
    pub async fn create(
        &self,
        pool: &PgPool,
        item: &ReleaseWorkItem,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO release_work_items (
                id, api_key, source_library_id, target_library_id, source_path,
                current_path, target_path, source_fingerprint, manifest_hash, state,
                state_reason, attempt_count, next_attempt_at, lease_owner,
                lease_acquired_at, lease_expires_at, latest_settling_snapshot_id,
                stable_since, last_observed_at, last_error_code, last_error_message,
                correlation_id, discovered_at, first_started_at, last_attempt_at,
                updated_at, created_at, completed_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28)",
        )
        .bind(item.id)
        .bind(item.api_key)
        .bind(item.source_library_id)
        .bind(item.target_library_id)
        .bind(&item.source_path)
        .bind(&item.current_path)
        .bind(&item.target_path)
        .bind(&item.source_fingerprint)
        .bind(&item.manifest_hash)
        .bind(item.state.as_str())
        .bind(&item.state_reason)
        .bind(i32::try_from(item.attempt_count).unwrap_or(i32::MAX))
        .bind(item.next_attempt_at)
        .bind(&item.lease_owner)
        .bind(item.lease_acquired_at)
        .bind(item.lease_expires_at)
        .bind(item.latest_settling_snapshot_id)
        .bind(item.stable_since)
        .bind(item.last_observed_at)
        .bind(item.last_error_code.as_ref().map(|c| c.as_str().to_string()))
        .bind(&item.last_error_message)
        .bind(item.correlation_id)
        .bind(item.discovered_at)
        .bind(item.first_started_at)
        .bind(item.last_attempt_at)
        .bind(item.updated_at)
        .bind(item.created_at)
        .bind(item.completed_at)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            item.id,
            WorkEventType::Created,
            None,
            Some(item.state),
            None,
            None,
            item.correlation_id,
            item.created_at,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %item.id, state = %item.state.as_str(), "work item created");
        Ok(())
    }

    /// Creates the work item if absent by stable identity and returns the row plus a
    /// `created` flag. `created == true` means this invocation inserted the row.
    /// Stable identity is `(source_library_id, source_path)` with NULL libraries
    /// grouped under a deterministic default identity.
    #[instrument(
        skip_all,
        fields(work_item_id = %item.id, source_path = %item.source_path)
    )]
    pub async fn create_or_get(
        &self,
        pool: &PgPool,
        item: &ReleaseWorkItem,
    ) -> Result<(ReleaseWorkItem, bool), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let inserted = sqlx::query_as::<_, WorkItemRow>(
            "INSERT INTO release_work_items (
                id, api_key, source_library_id, target_library_id, source_path,
                current_path, target_path, source_fingerprint, manifest_hash, state,
                state_reason, attempt_count, next_attempt_at, lease_owner,
                lease_acquired_at, lease_expires_at, latest_settling_snapshot_id,
                stable_since, last_observed_at, last_error_code, last_error_message,
                correlation_id, discovered_at, first_started_at, last_attempt_at,
                updated_at, created_at, completed_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28
            )
            ON CONFLICT ((COALESCE(source_library_id::text, '__default__') || '|' || source_path))
            DO NOTHING
            RETURNING *",
        )
        .bind(item.id)
        .bind(item.api_key)
        .bind(item.source_library_id)
        .bind(item.target_library_id)
        .bind(&item.source_path)
        .bind(&item.current_path)
        .bind(&item.target_path)
        .bind(&item.source_fingerprint)
        .bind(&item.manifest_hash)
        .bind(item.state.as_str())
        .bind(&item.state_reason)
        .bind(i32::try_from(item.attempt_count).unwrap_or(i32::MAX))
        .bind(item.next_attempt_at)
        .bind(&item.lease_owner)
        .bind(item.lease_acquired_at)
        .bind(item.lease_expires_at)
        .bind(item.latest_settling_snapshot_id)
        .bind(item.stable_since)
        .bind(item.last_observed_at)
        .bind(item.last_error_code.as_ref().map(|c| c.as_str().to_string()))
        .bind(&item.last_error_message)
        .bind(item.correlation_id)
        .bind(item.discovered_at)
        .bind(item.first_started_at)
        .bind(item.last_attempt_at)
        .bind(item.updated_at)
        .bind(item.created_at)
        .bind(item.completed_at)
        .fetch_optional(&mut *tx)
        .await?;

        let row = match inserted {
            Some(row) => {
                let row_state = row.state.clone();
                let state = IngestionState::parse(&row_state)
                    .ok_or_else(|| WorkStoreError::CorruptState(row_state))?;
                insert_event(
                    &mut tx,
                    row.id,
                    WorkEventType::Created,
                    None,
                    Some(state),
                    None,
                    None,
                    row.correlation_id,
                    row.created_at,
                    Some(serde_json::json!({
                        "source_library_id": row.source_library_id,
                        "source_path": row.source_path,
                    })),
                )
                .await?;
                (row, true)
            }
            None => {
                let row = sqlx::query_as::<_, WorkItemRow>(
                    "SELECT * FROM release_work_items
                     WHERE source_library_id IS NOT DISTINCT FROM $1 AND source_path = $2",
                )
                .bind(item.source_library_id)
                .bind(&item.source_path)
                .fetch_optional(&mut *tx)
                .await?;
                (row.ok_or(WorkStoreError::NotFound(item.id))?, false)
            }
        };

        tx.commit().await?;
        Ok((ReleaseWorkItem::try_from(row.0)?, row.1))
    }

    /// Looks up a work item by durable source identity.
    pub async fn get_by_source_identity(
        &self,
        pool: &PgPool,
        source_library_id: Option<Uuid>,
        source_path: &str,
    ) -> Result<Option<ReleaseWorkItem>, WorkStoreError> {
        let row = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items WHERE source_library_id IS NOT DISTINCT FROM $1 AND source_path = $2",
        )
        .bind(source_library_id)
        .bind(source_path)
        .fetch_optional(pool)
        .await?;
        row.map(ReleaseWorkItem::try_from).transpose()
    }

    /// Lists active discovery work items used by the reconciler (`discovered`,
    /// `settling`, `ready`).
    pub async fn list_active_for_reconciliation(
        &self,
        pool: &PgPool,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE state IN ('discovered', 'settling', 'ready')
             ORDER BY updated_at DESC",
        )
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(ReleaseWorkItem::try_from).collect()
    }

    /// Inserts a new snapshot for a work item and returns the new snapshot id.
    /// Snapshot files are persisted for operator observability and to keep
    /// comparison deterministic.
    #[allow(clippy::too_many_arguments)]
    pub async fn persist_settling_snapshot(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        signature: &str,
        file_count: i32,
        total_bytes: i64,
        ready_marker_seen: bool,
        bypassed_settling: bool,
        observed_at: OffsetDateTime,
        files: &[SettlingSnapshotFile],
    ) -> Result<i64, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let snapshot = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO release_work_item_settling_snapshots (
                work_item_id, observed_at, signature, file_count, total_bytes,
                ready_marker_seen, bypassed_settling
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
             RETURNING id",
        )
        .bind(work_item_id)
        .bind(observed_at)
        .bind(signature)
        .bind(file_count)
        .bind(total_bytes)
        .bind(ready_marker_seen)
        .bind(bypassed_settling)
        .fetch_one(&mut *tx)
        .await?;
        let snapshot_id = snapshot.0;

        for file in files {
            sqlx::query(
                "INSERT INTO release_work_item_settling_snapshot_files (
                    snapshot_id, relative_path, file_type, file_size, modified_at
                ) VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(snapshot_id)
            .bind(&file.relative_path)
            .bind(&file.file_type)
            .bind(file.file_size)
            .bind(file.modified_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(snapshot_id)
    }

    /// Fetches the latest persisted snapshot for an item.
    pub async fn latest_snapshot(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<SettlingSnapshot>, WorkStoreError> {
        let row = sqlx::query_as::<_, SettlingSnapshotRow>(
            "SELECT * FROM release_work_item_settling_snapshots
             WHERE work_item_id = $1
             ORDER BY observed_at DESC, id DESC
             LIMIT 1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(SettlingSnapshot {
            id: row.id,
            work_item_id: row.work_item_id,
            observed_at: row.observed_at,
            signature: row.signature,
            file_count: row.file_count,
            total_bytes: row.total_bytes,
            ready_marker_seen: row.ready_marker_seen,
            bypassed_settling: row.bypassed_settling,
        }))
    }

    /// Publishes a complete manifest and advances `inspecting -> normalizing`
    /// in one transaction. The unique work-item constraint makes publication
    /// idempotent and prevents competing workers from becoming authoritative.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_manifest(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        manifest: &ReleaseManifest,
        manifest_hash: &str,
        source_snapshot_id: Option<i64>,
        source_signature: Option<&str>,
        generation_duration_ms: i64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if item.state != IngestionState::Inspecting {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::Normalizing, TransitionKind::Worker)?;
        let payload = serde_json::to_value(manifest)?;
        let existing = sqlx::query_as::<_, (String,)>(
            "SELECT manifest_hash FROM release_work_item_manifests WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((existing_hash,)) = existing {
            if existing_hash != manifest_hash {
                return Err(WorkStoreError::ManifestConflict(id));
            }
        } else {
            sqlx::query(
                "INSERT INTO release_work_item_manifests (
                    work_item_id, schema_version, manifest_hash, source_snapshot_id,
                    source_signature, manifest_payload, ffprobe_path, ffprobe_version,
                    tool_metadata, source_stable, generation_duration_ms, generated_at
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            )
            .bind(id)
            .bind(i32::from(manifest.schema_version))
            .bind(manifest_hash)
            .bind(source_snapshot_id)
            .bind(source_signature)
            .bind(payload)
            .bind(manifest.inspection.ffprobe_path.clone())
            .bind(manifest.inspection.ffprobe_version.clone())
            .bind(serde_json::json!({ "schema_version": manifest.schema_version }))
            .bind(manifest.inspection.source_stable)
            .bind(generation_duration_ms.max(0))
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "UPDATE release_work_items
             SET state = 'normalizing', state_reason = 'inspection-complete',
                 manifest_hash = $2, lease_owner = NULL, lease_acquired_at = NULL,
                 lease_expires_at = NULL, updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(manifest_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(IngestionState::Inspecting),
            Some(IngestionState::Normalizing),
            Some("inspection-complete"),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "manifest_hash": manifest_hash,
                "schema_version": manifest.schema_version,
            })),
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, manifest_hash, "manifest published");
        Ok(())
    }

    /// Returns the authoritative manifest for a work item, if inspection has
    /// completed successfully.
    pub async fn get_manifest(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<StoredManifest>, WorkStoreError> {
        let row = sqlx::query_as::<
            _,
            (
                i64,
                Uuid,
                i32,
                String,
                serde_json::Value,
                Option<i64>,
                Option<String>,
                OffsetDateTime,
            ),
        >(
            "SELECT id, work_item_id, schema_version, manifest_hash, manifest_payload,
                    source_snapshot_id, source_signature, generated_at
             FROM release_work_item_manifests WHERE work_item_id = $1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(
            |(
                id,
                work_item_id,
                schema_version,
                manifest_hash,
                payload,
                source_snapshot_id,
                source_signature,
                generated_at,
            )| StoredManifest {
                id,
                work_item_id,
                schema_version,
                manifest_hash,
                payload,
                source_snapshot_id,
                source_signature,
                generated_at,
            },
        ))
    }

    /// Updates durable settling progress fields without changing core state.
    pub async fn upsert_settling_progress(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
        stable_since: Option<OffsetDateTime>,
        latest_snapshot_id: Option<i64>,
        last_observed_at: OffsetDateTime,
        state_reason: Option<&str>,
    ) -> Result<(), WorkStoreError> {
        sqlx::query(
            "UPDATE release_work_items
             SET stable_since = $2,
                 latest_settling_snapshot_id = $3,
                 last_observed_at = $4,
                 state_reason = COALESCE($5, state_reason),
                 updated_at = $4
             WHERE id = $1",
        )
        .bind(work_item_id)
        .bind(stable_since)
        .bind(latest_snapshot_id)
        .bind(last_observed_at)
        .bind(state_reason)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Atomically claims up to `config.claim_batch_size` eligible items for
    /// `worker`, establishing leases. Eligible: `ready` (not scheduled in the
    /// future) plus `failed` items whose retry time has arrived and whose
    /// attempts remain — those are transitioned `failed -> ready` (System) in
    /// the same transaction. Items with active leases are never claimed.
    ///
    /// Returns only work actually owned by this worker.
    #[instrument(skip_all, fields(worker_id = %worker, batch = config.claim_batch_size))]
    pub async fn claim(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        config: &WorkEngineConfig,
        now: OffsetDateTime,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let lease_expiry = now + config.lease_duration;
        let max_attempts = i32::try_from(config.retry_policy.max_attempts).unwrap_or(i32::MAX);
        let mut tx = pool.begin().await?;
        // Select candidates with row locks (skipped when locked by rivals),
        // then update each within the same transaction. Two steps so the
        // pre-update state is known for the failed->ready requeue event.
        let candidates = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE (
                     (state = 'ready' AND (next_attempt_at IS NULL OR next_attempt_at <= $2))
                     OR (state = 'failed' AND next_attempt_at IS NOT NULL
                         AND next_attempt_at <= $2 AND attempt_count < $4)
                   )
               AND (lease_expires_at IS NULL OR lease_expires_at <= $2)
             ORDER BY next_attempt_at NULLS FIRST, created_at, id
             LIMIT $3
             FOR UPDATE SKIP LOCKED",
        )
        .bind(worker.as_str())
        .bind(now)
        .bind(i64::from(config.claim_batch_size))
        .bind(max_attempts)
        .fetch_all(&mut *tx)
        .await?;

        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let was_failed = candidate.state == "failed";
            let row = sqlx::query_as::<_, WorkItemRow>(
                "UPDATE release_work_items w
                 SET state = CASE WHEN w.state = 'failed' THEN 'ready' ELSE w.state END,
                     state_reason = CASE WHEN w.state = 'failed' THEN NULL ELSE w.state_reason END,
                     next_attempt_at = NULL,
                     lease_owner = $1,
                     lease_acquired_at = $2,
                     lease_expires_at = $3,
                     first_started_at = COALESCE(w.first_started_at, $2),
                     updated_at = $2
                 WHERE w.id = $4
                 RETURNING w.*",
            )
            .bind(worker.as_str())
            .bind(now)
            .bind(lease_expiry)
            .bind(candidate.id)
            .fetch_one(&mut *tx)
            .await?;
            let item = ReleaseWorkItem::try_from(row)?;
            if was_failed {
                insert_event(
                    &mut tx,
                    item.id,
                    WorkEventType::Transitioned,
                    Some(IngestionState::Failed),
                    Some(IngestionState::Ready),
                    Some("retry.eligible"),
                    Some(worker.as_str()),
                    item.correlation_id,
                    now,
                    None,
                )
                .await?;
            }
            insert_event(
                &mut tx,
                item.id,
                WorkEventType::Claimed,
                None,
                None,
                None,
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({ "lease_expires_at": lease_expiry.to_string() })),
            )
            .await?;
            items.push(item);
        }
        tx.commit().await?;
        if items.is_empty() {
            debug!(worker_id = %worker, "claim: no eligible work");
        } else {
            info!(worker_id = %worker, claimed = items.len(), "lease acquired");
        }
        Ok(items)
    }

    /// System-kind transitions (discovery, settling, retry requeue): no lease
    /// required, but an active lease held by a worker blocks system moves.
    /// Used by discovery/settling (Phase 1B) and by tests/diagnostics.
    #[instrument(skip_all, fields(work_item_id = %id, to_state = to.as_str()))]
    pub async fn system_transition(
        &self,
        pool: &PgPool,
        id: Uuid,
        to: IngestionState,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if item.has_active_lease(now) {
            return Err(WorkStoreError::LeaseConflict(id));
        }
        item.state.ensure_transition(to, TransitionKind::System)?;
        let from = item.state;
        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2, state_reason = $3, updated_at = $4
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(to.as_str())
        .bind(reason)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(from),
            Some(to),
            reason,
            None,
            item.correlation_id,
            now,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, from_state = %from.as_str(), to_state = %to.as_str(), "state transitioned");
        ReleaseWorkItem::try_from(row)
    }

    /// Advances state for a leased worker or an operator. Illegal transitions
    /// fail atomically without changing persisted state.
    // The parameter list mirrors the persisted transition contract; bundling
    // into a request struct would only rename the same surface.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, to_state = to.as_str(), ?kind))]
    pub async fn transition(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: Option<&WorkerId>,
        kind: TransitionKind,
        to: IngestionState,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;

        // Ownership rules: workers must hold an active lease; operators must
        // not race someone else's active lease.
        match kind {
            TransitionKind::Worker => {
                let worker = worker.ok_or(WorkStoreError::InvalidState(id, item.state))?;
                ensure_active_lease(&item, worker, now)?;
            }
            TransitionKind::Operator => {
                if let Some(owner) = &item.lease_owner
                    && item.has_active_lease(now)
                    && Some(owner.as_str()) != worker.map(WorkerId::as_str)
                {
                    return Err(WorkStoreError::LeaseConflict(id));
                }
            }
            TransitionKind::System => {
                return Err(WorkStoreError::InvalidState(id, item.state));
            }
        }

        item.state.ensure_transition(to, kind)?;
        let from = item.state;
        let terminal = to.is_terminal();
        let completed_at = if terminal { Some(now) } else { None };
        // Leases belong to active processing: terminal states, operator
        // decisions, and parking in NeedsReview all release the lease so the
        // item is never stranded behind a dead lease.
        let clear_lease =
            terminal || kind == TransitionKind::Operator || to == IngestionState::NeedsReview;

        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2,
                 state_reason = $3,
                 updated_at = $4,
                 completed_at = $5,
                 lease_owner = CASE WHEN $6 THEN NULL ELSE lease_owner END,
                 lease_acquired_at = CASE WHEN $6 THEN NULL ELSE lease_acquired_at END,
                 lease_expires_at = CASE WHEN $6 THEN NULL ELSE lease_expires_at END
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(to.as_str())
        .bind(reason)
        .bind(now)
        .bind(completed_at)
        .bind(clear_lease)
        .fetch_one(&mut *tx)
        .await?;

        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(from),
            Some(to),
            reason,
            worker.map(WorkerId::as_str),
            item.correlation_id,
            now,
            None,
        )
        .await?;
        if terminal {
            insert_event(
                &mut tx,
                id,
                WorkEventType::Completed,
                None,
                Some(to),
                None,
                worker.map(WorkerId::as_str),
                item.correlation_id,
                now,
                None,
            )
            .await?;
        }
        tx.commit().await?;
        info!(work_item_id = %id, from_state = %from.as_str(), to_state = %to.as_str(), "state transitioned");
        ReleaseWorkItem::try_from(row)
    }

    /// Records a processing failure with durable evidence and computes retry
    /// eligibility. Clears the lease. Classification decides the outcome:
    /// retryable (scheduled or exhausted), permanent, or review-required.
    // Parameters mirror the durable failure record; see transition().
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker, error_code = %code.as_str()))]
    pub async fn record_failure(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        code: WorkErrorCode,
        message: &str,
        class: FailureClass,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        if !item.state.can_fail() {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }

        let attempt_count = item.attempt_count + 1;
        let (new_state, next_attempt_at): (IngestionState, Option<OffsetDateTime>) = match class {
            FailureClass::Retryable if policy.can_retry(attempt_count) => (
                IngestionState::Failed,
                policy.next_attempt_at(attempt_count, now),
            ),
            FailureClass::Retryable => (IngestionState::Failed, None),
            FailureClass::Permanent => (IngestionState::Failed, None),
            FailureClass::NeedsReview => {
                // SPEC 11.2: NeedsReview is reached from Validating only.
                item.state
                    .ensure_transition(IngestionState::NeedsReview, TransitionKind::Worker)?;
                (IngestionState::NeedsReview, None)
            }
        };

        let row = sqlx::query_as::<_, WorkItemRow>(
            "UPDATE release_work_items
             SET state = $2,
                 state_reason = $3,
                 attempt_count = $4,
                 last_attempt_at = $5,
                 last_error_code = $6,
                 last_error_message = $7,
                 next_attempt_at = $8,
                 lease_owner = NULL,
                 lease_acquired_at = NULL,
                 lease_expires_at = NULL,
                 updated_at = $5
             WHERE id = $1
             RETURNING *",
        )
        .bind(id)
        .bind(new_state.as_str())
        .bind(message)
        .bind(i32::try_from(attempt_count).unwrap_or(i32::MAX))
        .bind(now)
        .bind(code.as_str())
        .bind(message)
        .bind(next_attempt_at)
        .fetch_one(&mut *tx)
        .await?;

        insert_event(
            &mut tx,
            id,
            WorkEventType::Failed,
            Some(item.state),
            Some(new_state),
            Some(code.as_str()),
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({
                "attempt_count": attempt_count,
                "failure_class": class,
            })),
        )
        .await?;
        if let Some(next) = next_attempt_at {
            insert_event(
                &mut tx,
                id,
                WorkEventType::RetryScheduled,
                None,
                None,
                Some(code.as_str()),
                Some(worker.as_str()),
                item.correlation_id,
                now,
                Some(serde_json::json!({
                    "attempt_count": attempt_count,
                    "next_attempt_at": next.to_string(),
                })),
            )
            .await?;
        }
        tx.commit().await?;
        info!(
            work_item_id = %id,
            attempt_count,
            next_attempt_at = ?next_attempt_at,
            "failure recorded"
        );
        ReleaseWorkItem::try_from(row)
    }

    /// Explicitly releases a held lease without changing state (work becomes
    /// claimable again).
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn release_lease(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if item.lease_owner.as_deref() != Some(worker.as_str()) {
            return Err(WorkStoreError::NotLeaseOwner(id));
        }
        sqlx::query(
            "UPDATE release_work_items
             SET lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL, updated_at = $2
             WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::LeaseReleased,
            None,
            None,
            None,
            Some(worker.as_str()),
            item.correlation_id,
            now,
            None,
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, "lease released");
        Ok(())
    }

    /// Renews a held, unexpired lease (heartbeat). Deferred long-running stages
    /// can call this without any schema change — proof the Phase 1A design
    /// supports renewal without redesign.
    #[instrument(skip_all, fields(work_item_id = %id, worker_id = %worker))]
    pub async fn renew_lease(
        &self,
        pool: &PgPool,
        id: Uuid,
        worker: &WorkerId,
        lease_duration: time::Duration,
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        ensure_active_lease(&item, worker, now)?;
        let new_expiry = now + lease_duration;
        sqlx::query(
            "UPDATE release_work_items
             SET lease_expires_at = $2, updated_at = $3
             WHERE id = $1",
        )
        .bind(id)
        .bind(new_expiry)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::LeaseRenewed,
            None,
            None,
            None,
            Some(worker.as_str()),
            item.correlation_id,
            now,
            Some(serde_json::json!({ "lease_expires_at": new_expiry.to_string() })),
        )
        .await?;
        tx.commit().await?;
        info!(work_item_id = %id, lease_expires_at = %new_expiry, "lease renewed");
        Ok(new_expiry)
    }

    /// Recovers items whose leases expired mid-processing: marks them failed
    /// with `lease.lost` and schedules retry per policy (crash-loop protection
    /// via attempt counting). Returns recovered ids.
    #[instrument(skip_all)]
    pub async fn recover_expired_leases(
        &self,
        pool: &PgPool,
        policy: &RetryPolicy,
        now: OffsetDateTime,
    ) -> Result<Vec<Uuid>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE lease_expires_at IS NOT NULL AND lease_expires_at <= $1
             FOR UPDATE SKIP LOCKED",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;

        let mut recovered = Vec::with_capacity(rows.len());
        for row in rows {
            let item = ReleaseWorkItem::try_from(row)?;
            let attempt_count = item.attempt_count + 1;
            let next_attempt_at = if policy.can_retry(attempt_count) {
                policy.next_attempt_at(attempt_count, now)
            } else {
                None
            };
            // Items merely claimed (still ready) return to the queue without a
            // failure mark; items interrupted mid-stage become failed/lease.lost.
            let (new_state, error_code): (IngestionState, Option<&str>) =
                if item.state.is_active_processing() {
                    (IngestionState::Failed, Some(WorkErrorCode::LEASE_LOST))
                } else {
                    (item.state, None)
                };
            sqlx::query(
                "UPDATE release_work_items
                 SET state = $2,
                     state_reason = CASE WHEN $3 THEN 'worker lease expired' ELSE state_reason END,
                     attempt_count = CASE WHEN $3 THEN $4 ELSE attempt_count END,
                     last_attempt_at = CASE WHEN $3 THEN $5 ELSE last_attempt_at END,
                     last_error_code = CASE WHEN $3 THEN $6 ELSE last_error_code END,
                     last_error_message = CASE WHEN $3 THEN 'worker lease expired' ELSE last_error_message END,
                     next_attempt_at = CASE WHEN $3 THEN $7 ELSE NULL END,
                     lease_owner = NULL,
                     lease_acquired_at = NULL,
                     lease_expires_at = NULL,
                     updated_at = $5
                 WHERE id = $1",
            )
            .bind(item.id)
            .bind(new_state.as_str())
            .bind(item.state.is_active_processing())
            .bind(i32::try_from(attempt_count).unwrap_or(i32::MAX))
            .bind(now)
            .bind(error_code)
            .bind(next_attempt_at)
            .execute(&mut *tx)
            .await?;

            insert_event(
                &mut tx,
                item.id,
                WorkEventType::LeaseExpiredRecovered,
                Some(item.state),
                Some(new_state),
                error_code,
                item.lease_owner.as_deref(),
                item.correlation_id,
                now,
                None,
            )
            .await?;
            if item.state.is_active_processing() {
                insert_event(
                    &mut tx,
                    item.id,
                    WorkEventType::Failed,
                    Some(item.state),
                    Some(new_state),
                    error_code,
                    item.lease_owner.as_deref(),
                    item.correlation_id,
                    now,
                    Some(serde_json::json!({ "attempt_count": attempt_count })),
                )
                .await?;
                if let Some(next) = next_attempt_at {
                    insert_event(
                        &mut tx,
                        item.id,
                        WorkEventType::RetryScheduled,
                        None,
                        None,
                        error_code,
                        item.lease_owner.as_deref(),
                        item.correlation_id,
                        now,
                        Some(serde_json::json!({
                            "attempt_count": attempt_count,
                            "next_attempt_at": next.to_string(),
                        })),
                    )
                    .await?;
                }
            }
            recovered.push(item.id);
        }
        tx.commit().await?;
        if !recovered.is_empty() {
            info!(recovered = recovered.len(), "expired leases recovered");
        }
        Ok(recovered)
    }

    /// Fetches one work item by internal id or operator-safe api key.
    pub async fn get(
        &self,
        pool: &PgPool,
        id_or_api_key: Uuid,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let row = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items WHERE id = $1 OR api_key = $1",
        )
        .bind(id_or_api_key)
        .fetch_optional(pool)
        .await?
        .ok_or(WorkStoreError::NotFound(id_or_api_key))?;
        ReleaseWorkItem::try_from(row)
    }

    /// Lists work items, optionally filtered by state, newest first, bounded.
    pub async fn list(
        &self,
        pool: &PgPool,
        state: Option<IngestionState>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ReleaseWorkItem>, WorkStoreError> {
        let rows = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM release_work_items
             WHERE ($1::text IS NULL OR state = $1)
             ORDER BY created_at DESC, id
             LIMIT $2 OFFSET $3",
        )
        .bind(state.map(IngestionState::as_str))
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(ReleaseWorkItem::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Per-state counts for operator diagnostics (PRD-WORK-002 shape).
    pub async fn counts_by_state(&self, pool: &PgPool) -> Result<Vec<StateCount>, WorkStoreError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT state, COUNT(*)::bigint FROM release_work_items GROUP BY state ORDER BY state",
        )
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(|(state, count)| {
                Ok(StateCount {
                    state: IngestionState::parse(&state)
                        .ok_or(WorkStoreError::CorruptState(state))?,
                    count,
                })
            })
            .collect()
    }

    /// Append-only history for one work item, oldest first.
    pub async fn list_events(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Vec<ReleaseWorkItemEvent>, WorkStoreError> {
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT * FROM release_work_item_events
             WHERE work_item_id = $1 ORDER BY created_at, id",
        )
        .bind(work_item_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .map(ReleaseWorkItemEvent::try_from)
            .collect::<Result<Vec<_>, _>>()
    }
}

/// Locks a work item row for update inside a transaction.
async fn lock_for_update(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<ReleaseWorkItem, WorkStoreError> {
    let row = sqlx::query_as::<_, WorkItemRow>(
        "SELECT * FROM release_work_items WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WorkStoreError::NotFound(id))?;
    ReleaseWorkItem::try_from(row)
}

/// Requires an active lease held by `worker` at `now`.
fn ensure_active_lease(
    item: &ReleaseWorkItem,
    worker: &WorkerId,
    now: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    if item.lease_owner.as_deref() != Some(worker.as_str()) {
        return Err(WorkStoreError::NotLeaseOwner(item.id));
    }
    if !item.has_active_lease(now) {
        return Err(WorkStoreError::LeaseExpired(item.id));
    }
    Ok(())
}

/// Inserts one history event. Always called inside the mutation's transaction.
#[allow(clippy::too_many_arguments)]
async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
    event_type: WorkEventType,
    from_state: Option<IngestionState>,
    to_state: Option<IngestionState>,
    reason_code: Option<&str>,
    worker_id: Option<&str>,
    correlation_id: Uuid,
    now: OffsetDateTime,
    metadata: Option<serde_json::Value>,
) -> Result<(), WorkStoreError> {
    sqlx::query(
        "INSERT INTO release_work_item_events (
            work_item_id, event_type, from_state, to_state, reason_code,
            worker_id, correlation_id, created_at, metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(work_item_id)
    .bind(event_type.as_str())
    .bind(from_state.map(IngestionState::as_str))
    .bind(to_state.map(IngestionState::as_str))
    .bind(reason_code)
    .bind(worker_id)
    .bind(correlation_id)
    .bind(now)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
