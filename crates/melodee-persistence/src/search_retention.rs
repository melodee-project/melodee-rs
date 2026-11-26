//! Bounded processed search-outbox retention and exact hourly totals
//! (ADR-0214, PRD-SEARCH-007, PRD-PERF-001/006).
//!
//! Successful outbox rows are the operational evidence that a Tantivy commit
//! preceded the SQL acknowledgement. That evidence is valuable for a bounded
//! window and expensive forever: production kept 137,664 completed rows for a
//! chart that only needs one number per Track per hour.
//!
//! The contract implemented here:
//!
//! * **Detail** is retained for a configurable window (7 days by default,
//!   2–90) measured from `processed_at`. Pending, leased, retryable and
//!   permanently failed rows are never candidates.
//! * **Totals** are exact distinct Track counts per projection/hour, kept for
//!   400 days. A closed hour is finalized from the raw rows *before* any of its
//!   rows can be pruned, and finalization is serialized against
//!   acknowledgements so a late commit cannot lose a count.
//! * **Replay safety** survives deletion through a bounded per-entity
//!   tombstone (`pruned_through_revision`), not one row per event.
//! * **A high-water mark** (`highest_pruned_id`) is written atomically with each
//!   delete batch, so readers no longer assume `max(id)` proves completeness.

use serde::Serialize;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

use crate::{SEARCH_PROJECTION_NAME, SearchStoreError};

/// Default successful-detail retention.
pub const DEFAULT_PROCESSED_OUTBOX_RETENTION_DAYS: i64 = 7;
pub const MIN_PROCESSED_OUTBOX_RETENTION_DAYS: i64 = 2;
pub const MAX_PROCESSED_OUTBOX_RETENTION_DAYS: i64 = 90;
/// Exact hourly totals are retained for the chart's long window.
pub const HOURLY_TOTAL_RETENTION_DAYS: i64 = 400;
/// At most this many ordered rows are deleted per transaction.
pub const MAX_PRUNE_BATCH: i64 = 1_000;

/// Exclusive advisory key: prune batches and rebuild registration.
const MAINTENANCE_LOCK: (i32, i32) = (19_761_125, 301);
/// Shared by acknowledgements, exclusive by hour finalization.
const ACKNOWLEDGEMENT_LOCK: (i32, i32) = (19_761_125, 302);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxRetentionConfig {
    pub retention_days: i64,
}

impl Default for OutboxRetentionConfig {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_PROCESSED_OUTBOX_RETENTION_DAYS,
        }
    }
}

impl OutboxRetentionConfig {
    /// Builds the configuration, rejecting an out-of-range retention.
    pub fn new(retention_days: i64) -> Result<Self, SearchStoreError> {
        if !(MIN_PROCESSED_OUTBOX_RETENTION_DAYS..=MAX_PROCESSED_OUTBOX_RETENTION_DAYS)
            .contains(&retention_days)
        {
            return Err(SearchStoreError::InvalidRequest(format!(
                "processed outbox retention must be between \
                 {MIN_PROCESSED_OUTBOX_RETENTION_DAYS} and {MAX_PROCESSED_OUTBOX_RETENTION_DAYS} days"
            )));
        }
        Ok(Self { retention_days })
    }

    #[must_use]
    pub fn cutoff(self, now: OffsetDateTime) -> OffsetDateTime {
        now - Duration::days(self.retention_days)
    }
}

/// Operator-facing retention state. Used by the dry run, the Admin processing
/// view and the maintenance job history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRetentionStatus {
    pub projection_name: String,
    pub retention_days: i64,
    pub cutoff: Option<OffsetDateTime>,
    /// Rows that one apply pass is allowed to delete.
    pub eligible: i64,
    /// Processed rows retained by age, checkpoint or hour-finalization rules.
    pub protected: i64,
    /// Permanently failed rows; preserved regardless of age.
    pub failed: i64,
    pub pending: i64,
    pub leased: i64,
    /// Oldest completion that will be retained after pruning.
    pub oldest_retained_completion: Option<OffsetDateTime>,
    pub last_finalized_hour: Option<OffsetDateTime>,
    pub finalized_through: Option<OffsetDateTime>,
    pub earliest_trustworthy_hour: Option<OffsetDateTime>,
    pub highest_pruned_id: i64,
    pub last_prune_at: Option<OffsetDateTime>,
    pub last_prune_result: Option<serde_json::Value>,
    /// Nonterminal rebuild that pauses pruning, if any.
    pub blocking_rebuild: Option<String>,
    /// Exact hourly totals currently retained.
    pub hourly_total_rows: i64,
    pub oldest_hourly_total: Option<OffsetDateTime>,
}

/// Result of one applied bounded pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxPruneResult {
    pub dry_run: bool,
    pub hours_finalized: u32,
    pub rows_pruned: i64,
    pub batches: u32,
    pub hourly_totals_removed: i64,
    pub status: OutboxRetentionStatus,
}

fn lock_key(key: (i32, i32)) -> i64 {
    (i64::from(key.0) << 32) | i64::from(key.1)
}

async fn ensure_maintenance_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    projection: &str,
    now: OffsetDateTime,
) -> Result<(), SearchStoreError> {
    sqlx::query(
        "INSERT INTO search_outbox_maintenance (projection_name, updated_at)
         VALUES ($1, $2) ON CONFLICT (projection_name) DO NOTHING",
    )
    .bind(projection)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Finalizes every fully closed hour that is not yet finalized.
///
/// Exclusive against acknowledgements: an acknowledgement that is in flight
/// when finalization starts holds the shared lock, so its `processed_at` is
/// visible to the aggregate; acknowledgements that start later wait. The
/// aggregate insert and the watermark advance are idempotent.
pub async fn finalize_closed_hours(
    pool: &PgPool,
    now: OffsetDateTime,
) -> Result<u32, SearchStoreError> {
    let projection = SEARCH_PROJECTION_NAME;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key(ACKNOWLEDGEMENT_LOCK))
        .execute(&mut *tx)
        .await?;
    ensure_maintenance_row(&mut tx, projection, now).await?;
    // The newest fully closed hour. The current partial hour stays in raw
    // detail so the live chart keeps reporting it.
    let target = now
        .replace_minute(0)
        .and_then(|value| value.replace_second(0))
        .and_then(|value| value.replace_nanosecond(0))
        .map_err(|error| {
            SearchStoreError::InvalidRequest(format!(
                "current time cannot be truncated to an hour: {error}"
            ))
        })?;
    let target = target - Duration::hours(1);
    let finalized_through: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT finalized_through FROM search_outbox_maintenance
          WHERE projection_name = $1 FOR UPDATE",
    )
    .bind(projection)
    .fetch_one(&mut *tx)
    .await?;
    if finalized_through.is_some_and(|already| already >= target) {
        tx.commit().await?;
        return Ok(0);
    }
    let inserted = sqlx::query(
        "INSERT INTO search_outbox_hourly_totals (
            projection_name, hour, indexed_tracks, finalized_at
         )
         SELECT $1, date_trunc('hour', processed_at), count(DISTINCT entity_id), $3
           FROM catalog_projection_outbox
          WHERE projection_name = $1
            AND entity_type = 'track'
            AND processed_at IS NOT NULL
            AND processed_at < $2
            AND date_trunc('hour', processed_at) > COALESCE($4, '-infinity'::timestamptz)
          GROUP BY 2
         ON CONFLICT (projection_name, hour) DO NOTHING",
    )
    .bind(projection)
    .bind(target + Duration::hours(1))
    .bind(now)
    .bind(finalized_through)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE search_outbox_maintenance
            SET finalized_through = $2,
                earliest_trustworthy_hour = COALESCE(
                    earliest_trustworthy_hour,
                    (SELECT min(hour) FROM search_outbox_hourly_totals
                      WHERE projection_name = $1)
                ),
                updated_at = $3
          WHERE projection_name = $1",
    )
    .bind(projection)
    .bind(target)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(u32::try_from(inserted).unwrap_or(u32::MAX))
}

/// Reads retention state without mutating anything.
pub async fn retention_status(
    pool: &PgPool,
    config: OutboxRetentionConfig,
    now: OffsetDateTime,
    search_root: Option<&str>,
) -> Result<OutboxRetentionStatus, SearchStoreError> {
    let projection = SEARCH_PROJECTION_NAME;
    let cutoff = config.cutoff(now);
    #[allow(clippy::type_complexity)]
    let row: (
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
        i64,
        Option<OffsetDateTime>,
        Option<serde_json::Value>,
        i64,
        i64,
        i64,
        i64,
        i64,
        Option<OffsetDateTime>,
        i64,
        Option<OffsetDateTime>,
    ) = sqlx::query_as(
        "SELECT maintenance.finalized_through,
                maintenance.earliest_trustworthy_hour,
                COALESCE(maintenance.highest_pruned_id, 0),
                maintenance.last_prune_at,
                maintenance.last_prune_result,
                (SELECT count(*) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name
                    AND o.processed_at IS NOT NULL
                    AND o.failed_at IS NULL AND o.processed_at < $2
                    AND o.id <= COALESCE((SELECT last_processed_outbox_id
                                            FROM search_projection_checkpoint
                                           WHERE projection_name = projection.projection_name), 0)
                    AND date_trunc('hour', o.processed_at) <= COALESCE(maintenance.finalized_through, '-infinity'::timestamptz)),
                (SELECT count(*) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name
                    AND o.processed_at IS NOT NULL
                    AND o.failed_at IS NULL
                    AND (o.processed_at >= $2
                         OR o.id > COALESCE((SELECT last_processed_outbox_id
                                               FROM search_projection_checkpoint
                                              WHERE projection_name = projection.projection_name), 0)
                         OR date_trunc('hour', o.processed_at) > COALESCE(maintenance.finalized_through, '-infinity'::timestamptz))),
                (SELECT count(*) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name AND o.failed_at IS NOT NULL),
                (SELECT count(*) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name
                    AND o.processed_at IS NULL AND o.failed_at IS NULL
                    AND (o.lease_owner IS NULL OR o.lease_expires_at <= $3)),
                (SELECT count(*) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name
                    AND o.processed_at IS NULL AND o.failed_at IS NULL
                    AND o.lease_owner IS NOT NULL AND o.lease_expires_at > $3),
                (SELECT min(o.processed_at) FROM catalog_projection_outbox o
                  WHERE o.projection_name = projection.projection_name
                    AND o.processed_at IS NOT NULL
                    AND (o.processed_at >= $2
                         OR o.id > COALESCE((SELECT last_processed_outbox_id
                                               FROM search_projection_checkpoint
                                              WHERE projection_name = projection.projection_name), 0)
                         OR date_trunc('hour', o.processed_at) > COALESCE(maintenance.finalized_through, '-infinity'::timestamptz))),
                (SELECT count(*) FROM search_outbox_hourly_totals totals
                  WHERE totals.projection_name = projection.projection_name),
                (SELECT min(hour) FROM search_outbox_hourly_totals totals
                  WHERE totals.projection_name = projection.projection_name)
           FROM (SELECT $1::text AS projection_name) projection
           LEFT JOIN search_outbox_maintenance maintenance
             ON maintenance.projection_name = projection.projection_name",
    )
    .bind(projection)
    .bind(cutoff)
    .bind(now)
    .fetch_one(pool)
    .await?;
    let (
        finalized_through,
        earliest_trustworthy_hour,
        highest_pruned_id,
        last_prune_at,
        last_prune_result,
        eligible,
        protected,
        failed,
        pending,
        leased,
        oldest_retained_completion,
        hourly_total_rows,
        oldest_hourly_total,
    ) = row;
    let blocking_rebuild: Option<String> = sqlx::query_scalar(
        "SELECT job.status FROM search_rebuild_jobs job
          WHERE job.status IN ('requested', 'running', 'validated')
            AND ($1::text IS NULL OR job.search_root = $1)
          ORDER BY job.requested_at DESC, job.job_id DESC
          LIMIT 1",
    )
    .bind(search_root)
    .fetch_optional(pool)
    .await?;
    Ok(OutboxRetentionStatus {
        projection_name: projection.to_string(),
        retention_days: config.retention_days,
        cutoff: Some(cutoff),
        eligible,
        protected,
        failed,
        pending,
        leased,
        oldest_retained_completion,
        last_finalized_hour: finalized_through,
        finalized_through,
        earliest_trustworthy_hour,
        highest_pruned_id,
        last_prune_at,
        last_prune_result,
        blocking_rebuild,
        hourly_total_rows,
        oldest_hourly_total,
    })
}

/// Prunes eligible successful detail in bounded, ordered transactions.
///
/// `apply = false` performs the same selection and returns what would be
/// deleted without mutating anything. A protected nonterminal rebuild stops
/// the pass before any delete, and the reason is reported.
pub async fn prune_processed_outbox(
    pool: &PgPool,
    config: OutboxRetentionConfig,
    apply: bool,
    max_batches: u32,
    search_root: Option<&str>,
    now: OffsetDateTime,
) -> Result<OutboxPruneResult, SearchStoreError> {
    let hours_finalized = finalize_closed_hours(pool, now).await?;
    let projection = SEARCH_PROJECTION_NAME;
    let mut status = retention_status(pool, config, now, search_root).await?;
    status.last_finalized_hour = status.finalized_through;
    if !apply {
        return Ok(OutboxPruneResult {
            dry_run: true,
            hours_finalized,
            rows_pruned: 0,
            batches: 0,
            hourly_totals_removed: 0,
            status,
        });
    }
    let mut result = OutboxPruneResult {
        dry_run: false,
        hours_finalized,
        ..OutboxPruneResult::default()
    };
    if status.blocking_rebuild.is_none() {
        for _ in 0..max_batches.max(1) {
            let deleted = prune_batch(pool, projection, config.cutoff(now), now).await?;
            if deleted == 0 {
                break;
            }
            result.rows_pruned += deleted;
            result.batches += 1;
            // Yield between batches so one maintenance pass never monopolises
            // the database or a worker.
            tokio::task::yield_now().await;
            if deleted < MAX_PRUNE_BATCH {
                break;
            }
        }
    }
    // Exact totals age out on their own schedule; the finalized-through
    // watermark is retained so an old hour can never be resurrected.
    result.hourly_totals_removed = sqlx::query(
        "DELETE FROM search_outbox_hourly_totals
          WHERE projection_name = $1 AND hour < $2",
    )
    .bind(projection)
    .bind(now - Duration::days(HOURLY_TOTAL_RETENTION_DAYS))
    .execute(pool)
    .await?
    .rows_affected() as i64;
    let status_after = retention_status(pool, config, now, search_root).await?;
    let summary = serde_json::json!({
        "rowsPruned": result.rows_pruned,
        "batches": result.batches,
        "hourlyTotalsRemoved": result.hourly_totals_removed,
        "eligible": status_after.eligible,
        "protected": status_after.protected,
        "failed": status_after.failed,
        "blockingRebuild": status_after.blocking_rebuild,
        "retentionDays": config.retention_days,
    });
    sqlx::query(
        "INSERT INTO search_outbox_maintenance (projection_name, updated_at)
         VALUES ($1, $2) ON CONFLICT (projection_name) DO NOTHING",
    )
    .bind(projection)
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE search_outbox_maintenance
            SET last_prune_at = $2, last_prune_result = $3, updated_at = $2
          WHERE projection_name = $1",
    )
    .bind(projection)
    .bind(now)
    .bind(&summary)
    .execute(pool)
    .await?;
    result.status = retention_status(pool, config, now, search_root).await?;
    Ok(result)
}

async fn prune_batch(
    pool: &PgPool,
    projection: &str,
    cutoff: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<i64, SearchStoreError> {
    let mut tx = pool.begin().await?;
    // Serializes batch ownership with rebuild registration. A rebuild that
    // registers after this point is still protected by its own check.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key(MAINTENANCE_LOCK))
        .execute(&mut *tx)
        .await?;
    ensure_maintenance_row(&mut tx, projection, now).await?;
    let blocking: Option<String> = sqlx::query_scalar(
        "SELECT job.status FROM search_rebuild_jobs job
          WHERE job.status IN ('requested', 'running', 'validated')
          LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if blocking.is_some() {
        tx.rollback().await?;
        return Ok(0);
    }
    let deleted: i64 = sqlx::query_scalar(
        "WITH maintenance AS (
             SELECT highest_pruned_id,
                    finalized_through
               FROM search_outbox_maintenance
              WHERE projection_name = $1
              FOR UPDATE
         ),
         checkpoint AS (
             SELECT COALESCE(
                        (SELECT last_processed_outbox_id
                           FROM search_projection_checkpoint
                          WHERE projection_name = $1), 0) AS last_processed
         ),
         candidates AS (
             SELECT o.id, o.entity_type, o.entity_id, o.entity_revision
               FROM catalog_projection_outbox o, maintenance, checkpoint
              WHERE o.projection_name = $1
                AND o.processed_at IS NOT NULL
                AND o.failed_at IS NULL
                AND o.processed_at < $2
                AND o.id <= checkpoint.last_processed
                AND date_trunc('hour', o.processed_at)
                    <= COALESCE(maintenance.finalized_through, '-infinity'::timestamptz)
              ORDER BY o.id
              LIMIT $3
              FOR UPDATE SKIP LOCKED
         ),
         tombstones AS (
             INSERT INTO search_outbox_prune_tombstones (
                 projection_name, entity_type, entity_id,
                 pruned_through_revision, updated_at
             )
             SELECT $1, entity_type, entity_id, max(entity_revision), $4
               FROM candidates
              GROUP BY entity_type, entity_id
             ON CONFLICT (projection_name, entity_type, entity_id) DO UPDATE
             SET pruned_through_revision = GREATEST(
                     search_outbox_prune_tombstones.pruned_through_revision,
                     EXCLUDED.pruned_through_revision),
                 updated_at = EXCLUDED.updated_at
             RETURNING 1
         ),
         deleted AS (
             DELETE FROM catalog_projection_outbox outbox
              USING candidates
              WHERE outbox.id = candidates.id
             RETURNING candidates.id
         ),
         advanced AS (
             UPDATE search_outbox_maintenance
                SET highest_pruned_id = GREATEST(
                        highest_pruned_id,
                        COALESCE((SELECT max(id) FROM deleted), highest_pruned_id)),
                    last_prune_at = $4,
                    updated_at = $4
              WHERE projection_name = $1
             RETURNING 1
         )
         SELECT count(*) FROM deleted",
    )
    .bind(projection)
    .bind(cutoff)
    .bind(MAX_PRUNE_BATCH)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(deleted)
}

/// Durable retention state for dashboards and maintenance diagnostics.
pub async fn maintenance_snapshot(
    pool: &PgPool,
) -> Result<Option<OutboxRetentionStatus>, SearchStoreError> {
    Ok(Some(
        retention_status(
            pool,
            OutboxRetentionConfig::default(),
            OffsetDateTime::now_utc(),
            None,
        )
        .await?,
    ))
}

/// Highest outbox id that ever existed for the projection.
///
/// `max(id)` alone is not a proof of completeness once processed detail is
/// pruned, so the durable per-projection watermark participates.
pub async fn projection_high_water(pool: &PgPool) -> Result<i64, SearchStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT GREATEST(
                    COALESCE((SELECT max(id) FROM catalog_projection_outbox
                               WHERE projection_name = $1), 0),
                    COALESCE((SELECT highest_pruned_id FROM search_outbox_maintenance
                               WHERE projection_name = $1), 0)
                )",
    )
    .bind(SEARCH_PROJECTION_NAME)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn retention_bounds_are_enforced() {
        assert!(OutboxRetentionConfig::new(1).is_err());
        assert!(OutboxRetentionConfig::new(91).is_err());
        assert_eq!(
            OutboxRetentionConfig::new(7)
                .expect("default")
                .retention_days,
            7
        );
    }
}
