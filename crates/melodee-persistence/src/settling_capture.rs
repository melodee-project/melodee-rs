//! Incremental, fenced settling evidence. Incomplete captures are never visible
//! as snapshots and are discarded a page at a time before a scope starts over.

use melodee_domain::{IngestionState, ReleaseWorkItem, TransitionKind, WorkEventType};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DiscoveryScanFence, SettlingSnapshotFile, WorkItemStore, WorkStoreError};

/// Upper bound on rows in one capture or cleanup transaction.
pub const SETTLING_CAPTURE_PAGE_SIZE: usize = 128;

/// A scope's unfinished capture; it must be discarded before a new capture.
#[derive(Debug, Clone, Copy)]
pub struct StagedSnapshot {
    pub snapshot_id: i64,
    pub work_item_id: Uuid,
    pub newly_created: bool,
}

/// A frozen snapshot whose ordered metadata signature was computed by this store.
#[derive(Debug, Clone)]
pub struct VerifiedStagedSnapshot {
    snapshot_id: i64,
    work_item_id: Uuid,
    scope_key: String,
    generation: Uuid,
    owner: Uuid,
    signature: String,
    file_count: i32,
    total_bytes: i64,
}

impl VerifiedStagedSnapshot {
    pub fn snapshot_id(&self) -> i64 {
        self.snapshot_id
    }
    pub fn signature(&self) -> &str {
        &self.signature
    }
    pub fn file_count(&self) -> i32 {
        self.file_count
    }
    pub fn total_bytes(&self) -> i64 {
        self.total_bytes
    }
}

/// Verified snapshot details to publish with the settling decision.
#[derive(Debug, Clone, Copy)]
pub struct StagedSnapshotCompletion<'a> {
    pub verified: &'a VerifiedStagedSnapshot,
    pub ready_marker_seen: bool,
    pub bypassed_settling: bool,
}

/// One observation, its expected pre-state and resulting settling decision.
#[derive(Debug)]
pub struct SettlingObservation<'a> {
    pub expected: &'a ReleaseWorkItem,
    pub staged: Option<StagedSnapshotCompletion<'a>>,
    pub next_state: Option<IngestionState>,
    pub transition_reason: &'a str,
    pub state_reason: &'a str,
    pub stable_since: Option<OffsetDateTime>,
    pub latest_snapshot_id: Option<i64>,
    pub observed_at: OffsetDateTime,
}

pub(crate) async fn lock_item(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    fence: &DiscoveryScanFence,
) -> Result<(String, String), WorkStoreError> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT source_path, state FROM release_work_items
         WHERE id=$1 AND COALESCE(source_library_id::text, 'default')=$2
           AND inbound_active AND state IN ('discovered','settling','ready')
           AND lease_owner IS NULL AND discovery_scan_generation=$3 FOR UPDATE",
    )
    .bind(item)
    .bind(&fence.scope_key)
    .bind(fence.generation)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WorkStoreError::InvalidRequest(
        "settling capture work-item binding changed",
    ))
}

fn capture_database_error(error: sqlx::Error) -> WorkStoreError {
    if matches!(&error, sqlx::Error::Database(db) if db.code().as_deref() == Some("53400")) {
        WorkStoreError::SettlingCaptureBudgetExhausted
    } else {
        WorkStoreError::Database(error)
    }
}

pub(crate) async fn lock_capture(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    item: Uuid,
    fence: &DiscoveryScanFence,
    writable: bool,
) -> Result<(), WorkStoreError> {
    fence.lock(tx).await?;
    let (path, state) = lock_item(tx, item, fence).await?;
    let valid = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM release_work_item_settling_snapshots
         WHERE id=$1 AND work_item_id=$2 AND NOT capture_complete AND NOT capture_discarding
           AND capture_scope=$3 AND capture_generation=$4 AND capture_owner=$5
           AND capture_source_path=$6 AND capture_source_state=$7
           AND (NOT $8 OR NOT capture_frozen) FOR UPDATE",
    )
    .bind(id)
    .bind(item)
    .bind(&fence.scope_key)
    .bind(fence.generation)
    .bind(fence.owner)
    .bind(path)
    .bind(state)
    .bind(writable)
    .fetch_optional(&mut **tx)
    .await?;
    if valid.is_none() {
        return Err(WorkStoreError::InvalidRequest(
            "settling capture binding changed",
        ));
    }
    Ok(())
}

impl WorkItemStore {
    /// Begin one capture for a discovery scope. An interrupted capture is
    /// returned instead; discard it in pages before restarting filesystem work.
    pub async fn begin_staged_snapshot(
        &self,
        pool: &PgPool,
        item: Uuid,
        observed_at: OffsetDateTime,
        fence: &DiscoveryScanFence,
    ) -> Result<StagedSnapshot, WorkStoreError> {
        let mut tx = pool.begin().await?;
        fence.lock(&mut tx).await?;
        if let Some((snapshot_id, work_item_id)) = sqlx::query_as::<_, (i64, Uuid)>(
            "SELECT id, work_item_id FROM release_work_item_settling_snapshots
             WHERE capture_scope=$1 AND NOT capture_complete AND NOT capture_discarding FOR UPDATE",
        )
        .bind(&fence.scope_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(StagedSnapshot {
                snapshot_id,
                work_item_id,
                newly_created: false,
            });
        }
        let (path, state) = lock_item(&mut tx, item, fence).await?;
        let snapshot_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO release_work_item_settling_snapshots
             (work_item_id, observed_at, signature, file_count, total_bytes,
              ready_marker_seen, bypassed_settling, capture_complete, capture_scope,
              capture_generation, capture_owner, capture_source_path, capture_source_state)
             VALUES ($1,$2,'',0,0,false,false,false,$3,$4,$5,$6,$7) RETURNING id",
        )
        .bind(item)
        .bind(observed_at)
        .bind(&fence.scope_key)
        .bind(fence.generation)
        .bind(fence.owner)
        .bind(path)
        .bind(state)
        .fetch_one(&mut *tx)
        .await
        .map_err(capture_database_error)?;
        tx.commit().await?;
        Ok(StagedSnapshot {
            snapshot_id,
            work_item_id: item,
            newly_created: true,
        })
    }

    /// Append a bounded page. Retrying the same records is idempotent; a path
    /// repeated with different metadata fails rather than changing evidence.
    pub async fn append_staged_snapshot_files(
        &self,
        pool: &PgPool,
        id: i64,
        item: Uuid,
        files: &[SettlingSnapshotFile],
        fence: &DiscoveryScanFence,
    ) -> Result<(), WorkStoreError> {
        if files.len() > SETTLING_CAPTURE_PAGE_SIZE {
            return Err(WorkStoreError::InvalidRequest(
                "settling capture page exceeds 128 entries",
            ));
        }
        let mut tx = pool.begin().await?;
        lock_capture(&mut tx, id, item, fence, true).await?;
        let paths: Vec<&str> = files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect();
        let types: Vec<&str> = files.iter().map(|file| file.file_type.as_str()).collect();
        let sizes: Vec<i64> = files.iter().map(|file| file.file_size).collect();
        let times: Vec<OffsetDateTime> = files.iter().map(|file| file.modified_at).collect();
        let nanos: Vec<String> = files
            .iter()
            .map(|file| file.modified_at.unix_timestamp_nanos().to_string())
            .collect();
        sqlx::query("INSERT INTO release_work_item_settling_snapshot_files
                (snapshot_id,relative_path,file_type,file_size,modified_at,modified_at_nanos)
             SELECT $1,path,kind,size,modified,nano FROM unnest($2::text[],$3::text[],$4::bigint[],$5::timestamptz[],$6::text[]) AS input(path,kind,size,modified,nano)
             ON CONFLICT (snapshot_id,relative_path) DO NOTHING")
            .bind(id).bind(&paths).bind(&types).bind(&sizes).bind(&times).bind(&nanos).execute(&mut *tx).await.map_err(capture_database_error)?;
        let identical = sqlx::query_scalar::<_, bool>("SELECT COALESCE(bool_and(COALESCE(stored.file_type=input.kind AND stored.file_size=input.size AND stored.modified_at_nanos=input.nano,false)),true)
            FROM unnest($2::text[],$3::text[],$4::bigint[],$5::text[]) AS input(path,kind,size,nano)
            LEFT JOIN release_work_item_settling_snapshot_files stored ON stored.snapshot_id=$1 AND stored.relative_path=input.path")
            .bind(id).bind(&paths).bind(&types).bind(&sizes).bind(&nanos).fetch_one(&mut *tx).await?;
        if !identical {
            return Err(WorkStoreError::InvalidRequest(
                "conflicting settling capture path",
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Read a bounded page in the same byte order used by Rust's string sort.
    pub async fn staged_snapshot_files_page(
        &self,
        pool: &PgPool,
        id: i64,
        item: Uuid,
        after: Option<&str>,
        fence: &DiscoveryScanFence,
    ) -> Result<Vec<SettlingSnapshotFile>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_capture(&mut tx, id, item, fence, false).await?;
        sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE scope_key=$1 AND generation=$2 AND owner=$3")
            .bind(&fence.scope_key).bind(fence.generation).bind(fence.owner).execute(&mut *tx).await?;
        let rows = sqlx::query_as::<_, (String, String, i64, String)>(
            "SELECT relative_path,file_type,file_size,modified_at_nanos
             FROM release_work_item_settling_snapshot_files
             WHERE snapshot_id=$1 AND ($2::text IS NULL OR relative_path COLLATE \"C\" > $2 COLLATE \"C\")
             ORDER BY relative_path COLLATE \"C\" LIMIT 128",
        ).bind(id).bind(after).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|(relative_path, file_type, file_size, nanos)| {
                let nanos = nanos.parse::<i128>().map_err(|_| {
                    WorkStoreError::CorruptState("invalid settling timestamp".into())
                })?;
                let modified_at =
                    OffsetDateTime::from_unix_timestamp_nanos(nanos).map_err(|_| {
                        WorkStoreError::CorruptState(
                            "settling timestamp outside supported range".into(),
                        )
                    })?;
                Ok(SettlingSnapshotFile {
                    relative_path,
                    file_type,
                    file_size,
                    modified_at,
                })
            })
            .collect()
    }

    /// Freeze and hash one deterministic, bounded-page pass. Lease renewal
    /// occurs on each page; cancellation leaves invisible recoverable evidence.
    pub async fn seal_staged_snapshot(
        &self,
        pool: &PgPool,
        id: i64,
        item: Uuid,
        fence: &DiscoveryScanFence,
    ) -> Result<VerifiedStagedSnapshot, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_capture(&mut tx, id, item, fence, false).await?;
        sqlx::query(
            "UPDATE release_work_item_settling_snapshots SET capture_frozen=true WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut digest = blake3::Hasher::new();
        let mut file_count = 0_i32;
        let mut total_bytes = 0_i64;
        let mut after: Option<String> = None;
        loop {
            let page = self
                .staged_snapshot_files_page(pool, id, item, after.as_deref(), fence)
                .await?;
            if page.is_empty() {
                break;
            }
            for file in &page {
                digest.update(
                    format!(
                        "{}\u{001f}{}\u{001f}{}\u{001f}{}\n",
                        file.relative_path,
                        file.file_type,
                        file.file_size,
                        file.modified_at.unix_timestamp_nanos()
                    )
                    .as_bytes(),
                );
                file_count = file_count
                    .checked_add(1)
                    .ok_or(WorkStoreError::InvalidRequest(
                        "settling file count overflow",
                    ))?;
                total_bytes = total_bytes
                    .checked_add(file.file_size)
                    .ok_or(WorkStoreError::InvalidRequest("settling size overflow"))?;
            }
            after = page.last().map(|file| file.relative_path.clone());
        }
        Ok(VerifiedStagedSnapshot {
            snapshot_id: id,
            work_item_id: item,
            scope_key: fence.scope_key.clone(),
            generation: fence.generation,
            owner: fence.owner,
            signature: format!("melodee-snapshot-v2-blake3:{}", digest.finalize().to_hex()),
            file_count,
            total_bytes,
        })
    }

    /// Publish verified evidence and the settling decision in the same fenced
    /// work-item transaction, so a worker cannot claim between those effects.
    pub async fn apply_settling_observation_fenced(
        &self,
        pool: &PgPool,
        observation: &SettlingObservation<'_>,
        fence: &DiscoveryScanFence,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let expected = observation.expected;
        if let Some(staged) = observation.staged {
            if observation.latest_snapshot_id != Some(staged.verified.snapshot_id)
                || staged.verified.work_item_id != expected.id
                || staged.verified.scope_key != fence.scope_key
                || staged.verified.generation != fence.generation
                || staged.verified.owner != fence.owner
            {
                return Err(WorkStoreError::InvalidRequest(
                    "settling snapshot pointer mismatch",
                ));
            }
        }
        let mut tx = pool.begin().await?;
        if let Some(staged) = observation.staged {
            lock_capture(
                &mut tx,
                staged.verified.snapshot_id,
                expected.id,
                fence,
                false,
            )
            .await?;
        } else {
            fence.lock(&mut tx).await?;
            lock_item(&mut tx, expected.id, fence).await?;
        }
        let current = crate::work_items::lock_for_update(&mut tx, expected.id).await?;
        if current.state != expected.state
            || current.source_library_id != expected.source_library_id
            || current.source_path != expected.source_path
            || current.latest_settling_snapshot_id != expected.latest_settling_snapshot_id
            || current.stable_since != expected.stable_since
            || current.last_observed_at != expected.last_observed_at
            || current.lease_owner.is_some()
        {
            return Err(WorkStoreError::InvalidRequest(
                "settling observation work-item changed",
            ));
        }
        let next = observation.next_state.unwrap_or(current.state);
        if next != current.state {
            current
                .state
                .ensure_transition(next, TransitionKind::System)?;
        }
        if let Some(staged) = observation.staged {
            let changed = sqlx::query("UPDATE release_work_item_settling_snapshots SET signature=$2,file_count=$3,total_bytes=$4,ready_marker_seen=$5,bypassed_settling=$6,capture_complete=true WHERE id=$1 AND capture_frozen AND NOT capture_complete AND NOT capture_discarding AND capture_rows=$3 AND capture_file_bytes=$4")
                .bind(staged.verified.snapshot_id).bind(&staged.verified.signature).bind(staged.verified.file_count).bind(staged.verified.total_bytes)
                .bind(staged.ready_marker_seen).bind(staged.bypassed_settling).execute(&mut *tx).await?.rows_affected();
            if changed != 1 {
                return Err(WorkStoreError::InvalidRequest("settling capture changed"));
            }
        } else if let Some(id) = observation.latest_snapshot_id {
            let exists = sqlx::query_scalar::<_, bool>("SELECT true FROM release_work_item_settling_snapshots WHERE id=$1 AND work_item_id=$2 AND capture_complete")
                .bind(id).bind(expected.id).fetch_optional(&mut *tx).await?.is_some();
            if !exists {
                return Err(WorkStoreError::InvalidRequest(
                    "settling snapshot pointer invalid",
                ));
            }
        }
        let row = sqlx::query_as::<_, crate::work_items::WorkItemRow>(
            "UPDATE release_work_items SET state=$2,state_reason=$3,stable_since=$4,
               latest_settling_snapshot_id=$5,last_observed_at=$6,updated_at=$6
             WHERE id=$1 RETURNING *",
        )
        .bind(expected.id)
        .bind(next.as_str())
        .bind(observation.state_reason)
        .bind(observation.stable_since)
        .bind(observation.latest_snapshot_id)
        .bind(observation.observed_at)
        .fetch_one(&mut *tx)
        .await?;
        if next != current.state {
            crate::work_items::insert_event(
                &mut tx,
                expected.id,
                WorkEventType::Transitioned,
                Some(current.state),
                Some(next),
                Some(observation.transition_reason),
                None,
                current.correlation_id,
                observation.observed_at,
                None,
            )
            .await?;
        }
        let updated = ReleaseWorkItem::try_from(row)?;
        tx.commit().await?;
        Ok(updated)
    }

    /// Queue an incomplete capture for incremental background deletion.
    /// This releases the scope's active-capture slot immediately.
    pub async fn mark_staged_snapshot_discarding(
        &self,
        pool: &PgPool,
        id: i64,
        item: Uuid,
        fence: &DiscoveryScanFence,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        fence.lock(&mut tx).await?;
        let count = sqlx::query("UPDATE release_work_item_settling_snapshots SET capture_discarding=true WHERE id=$1 AND work_item_id=$2 AND capture_scope=$3 AND NOT capture_complete")
            .bind(id).bind(item).bind(&fence.scope_key).execute(&mut *tx).await?.rows_affected();
        if count != 1 {
            return Err(WorkStoreError::InvalidRequest("settling capture not found"));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Delete at most 128 rows, returning true when the abandoned capture is gone.
    /// The current scan owner may clean a predecessor's capture after a crash.
    pub async fn discard_staged_snapshot_page(
        &self,
        pool: &PgPool,
        id: i64,
        item: Uuid,
        fence: &DiscoveryScanFence,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        fence.lock(&mut tx).await?;
        let valid = sqlx::query_scalar::<_, bool>("SELECT true FROM release_work_item_settling_snapshots WHERE id=$1 AND work_item_id=$2 AND capture_scope=$3 AND NOT capture_complete FOR UPDATE")
            .bind(id).bind(item).bind(&fence.scope_key).fetch_optional(&mut *tx).await?;
        if valid.is_none() {
            return Err(WorkStoreError::InvalidRequest("settling capture not found"));
        }
        sqlx::query(
            "UPDATE release_work_item_settling_snapshots SET capture_discarding=true WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM release_work_item_settling_snapshot_files WHERE id IN (SELECT id FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1 ORDER BY relative_path LIMIT 128)")
            .bind(id).execute(&mut *tx).await?.rows_affected();
        if deleted == 0 {
            sqlx::query("DELETE FROM release_work_item_settling_snapshots WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(deleted == 0)
    }

    /// Reclaim one bounded page from an expired scope, including sources no
    /// longer configured. A fresh owner cannot be acquired while this short
    /// transaction holds the scan row lock.
    pub async fn cleanup_expired_staged_snapshot_page(
        &self,
        pool: &PgPool,
        prefer_expired: bool,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        // Alternate the priority across calls so continuously arriving
        // discarded captures cannot starve an expired active capture.
        let mut candidate = None;
        for expired in [prefer_expired, !prefer_expired] {
            let query = if expired {
                "SELECT snapshot.id FROM ingestion_discovery_scan scan
                 JOIN release_work_item_settling_snapshots snapshot
                   ON snapshot.capture_scope=scan.scope_key AND NOT snapshot.capture_complete AND NOT snapshot.capture_discarding
                 WHERE scan.lease_expires_at <= clock_timestamp()
                 ORDER BY scan.lease_expires_at LIMIT 1 FOR UPDATE OF scan SKIP LOCKED"
            } else {
                "SELECT id FROM release_work_item_settling_snapshots
                 WHERE NOT capture_complete AND capture_discarding
                 ORDER BY capture_cleanup_at NULLS FIRST, id LIMIT 1 FOR UPDATE SKIP LOCKED"
            };
            candidate = sqlx::query_as::<_, (i64,)>(query)
                .fetch_optional(&mut *tx)
                .await?;
            if candidate.is_some() {
                break;
            }
        }
        let found = candidate.is_some();
        if let Some((id,)) = candidate {
            sqlx::query("UPDATE release_work_item_settling_snapshots SET capture_discarding=true, capture_cleanup_at=clock_timestamp() WHERE id=$1 AND NOT capture_complete")
                .bind(id).execute(&mut *tx).await?;
            let deleted = sqlx::query("DELETE FROM release_work_item_settling_snapshot_files WHERE id IN (SELECT id FROM release_work_item_settling_snapshot_files WHERE snapshot_id=$1 ORDER BY relative_path LIMIT 128)")
                .bind(id).execute(&mut *tx).await?.rows_affected();
            if deleted == 0 {
                sqlx::query("DELETE FROM release_work_item_settling_snapshots WHERE id=$1 AND NOT capture_complete")
                    .bind(id).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(found)
    }
}
