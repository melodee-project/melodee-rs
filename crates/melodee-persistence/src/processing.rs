//! Bulk registration, weighted queue admission and operator throughput evidence.
use std::collections::HashSet;

use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{WorkEngineConfig, WorkStoreError};

/// Reserve the next occurrence of each immediate inbound directory. Existing
/// occurrences and other inbound roots must never inherit this submission.
pub async fn register_bulk(
    pool: &PgPool,
    source: Option<Uuid>,
    names: &[String],
) -> Result<Uuid, WorkStoreError> {
    let mut seen = HashSet::new();
    if names.is_empty()
        || names.iter().any(|n| {
            n.is_empty() || n == "." || n == ".." || n.contains(['/', '\0']) || !seen.insert(n)
        })
    {
        return Err(WorkStoreError::InvalidRequest(
            "supply unique immediate inbound directory names",
        ));
    }
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO ingest_bulk_submissions(id) VALUES ($1)")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query(
        "INSERT INTO ingest_bulk_entries(batch_id, source_id, source_path)
        SELECT $1,$2,name FROM unnest($3::text[]) name
        WHERE NOT EXISTS (SELECT FROM release_work_items w WHERE w.inbound_active
            AND w.source_path=name AND w.source_library_id IS NOT DISTINCT FROM $2)
        ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(source)
    .bind(names)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != names.len() as u64 {
        return Err(WorkStoreError::InvalidRequest(
            "a directory is already registered or has an active inbound occurrence",
        ));
    }
    tx.commit().await?;
    Ok(id)
}

pub(crate) async fn assign_registered_arrival(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    source: Option<Uuid>,
    path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("WITH assigned AS (UPDATE ingest_bulk_entries SET work_item_id=$1
        WHERE work_item_id IS NULL AND source_path=$2 AND source_id IS NOT DISTINCT FROM $3 RETURNING batch_id)
        UPDATE release_work_items SET bulk_submission_id=assigned.batch_id FROM assigned WHERE id=$1")
        .bind(item)
        .bind(path)
        .bind(source)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(FromRow)]
pub(crate) struct ClaimCandidate {
    pub(crate) id: Uuid,
    pub(crate) state: String,
    pub(crate) release_id: Option<Uuid>,
    source_library_id: Option<Uuid>,
    source_path: String,
}

/// A persistent four-slot cycle preserves the bulk share even for one-slot
/// workers and restarts. The cursor and the actual claims commit together.
pub(crate) async fn claim_candidates(
    tx: &mut Transaction<'_, Postgres>,
    stage: &str,
    config: &WorkEngineConfig,
    now: OffsetDateTime,
    protected_allowed: bool,
) -> Result<Vec<ClaimCandidate>, sqlx::Error> {
    crate::work_item_lifecycle::admit_runtime(tx).await?;
    let mut slot: i32 =
        sqlx::query_scalar("SELECT slot FROM ingest_claim_cursors WHERE stage=$1 FOR UPDATE")
            .bind(stage)
            .fetch_one(&mut **tx)
            .await?;
    let limit = i64::from(config.claim_batch_size);
    let max_attempts = i32::try_from(config.retry_policy.max_attempts).unwrap_or(i32::MAX);
    let mut lanes: [Vec<ClaimCandidate>; 2] = [Vec::new(), Vec::new()];
    for (index, lane) in lanes.iter_mut().enumerate() {
        *lane = sqlx::query_as(include_str!("processing/claim_lane.sql"))
            .bind(stage)
            .bind(index == 1)
            .bind(now)
            .bind(max_attempts)
            .bind(limit)
            .bind(protected_allowed)
            .fetch_all(&mut **tx)
            .await?;
        lane.reverse();
    }
    let mut chosen = Vec::new();
    while chosen.len() < config.claim_batch_size as usize {
        let preferred = usize::from(slot == 3);
        let Some(row) = lanes[preferred]
            .pop()
            .or_else(|| lanes[1 - preferred].pop())
        else {
            break;
        };
        // Candidate rows are already locked. Never wait on the occurrence
        // lock in the inverse order: a purge may hold it while waiting for
        // this item. A failed try-lock skips the candidate until a later turn.
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(1869834357,hashtext(COALESCE($1::uuid::text,'__default__') || '|' || $2::text))")
            .bind(row.source_library_id).bind(&row.source_path).fetch_one(&mut **tx).await?;
        if !locked {
            continue;
        }
        // This is a separate statement under READ COMMITTED, after acquiring
        // the occurrence lock, so a newly committed guard cannot be missed by
        // the earlier candidate-selection snapshot.
        let guarded: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_occurrence_purge_guards WHERE source_library_id IS NOT DISTINCT FROM $1 AND ingest_occurrence_hash(source_path)=ingest_occurrence_hash($2) AND source_path COLLATE \"C\"=$2 COLLATE \"C\") OR EXISTS(SELECT 1 FROM admin_media_reservation_items WHERE source_library_id IS NOT DISTINCT FROM $1 AND ingest_occurrence_hash(source_path)=ingest_occurrence_hash($2) AND source_path COLLATE \"C\"=$2 COLLATE \"C\")")
            .bind(row.source_library_id).bind(&row.source_path).fetch_one(&mut **tx).await?;
        if guarded {
            continue;
        }
        chosen.push(row);
        slot = (slot + 1) % 4;
    }
    if !chosen.is_empty() {
        sqlx::query("UPDATE ingest_claim_cursors SET slot=$2 WHERE stage=$1")
            .bind(stage)
            .bind(slot)
            .execute(&mut **tx)
            .await?;
    }
    Ok(chosen)
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageHealth {
    pub stage: String,
    pub waiting: i64,
    pub in_progress: i64,
    pub oldest_release: Option<String>,
    pub oldest_item: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub oldest_since: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_completion: Option<OffsetDateTime>,
    pub completed_hour: i64,
    pub stalled: bool,
}

/// Stage progress derives from state transitions. Polls, retries and lease
/// heartbeats do not count as a successful handoff.
pub async fn stage_health(
    pool: &PgPool,
    now: OffsetDateTime,
    limits: MonitorLimits,
) -> Result<Vec<StageHealth>, WorkStoreError> {
    Ok(sqlx::query_as(include_str!("processing/stage_health.sql"))
        .bind(now)
        .bind(i32::try_from(limits.max_attempts).unwrap_or(i32::MAX))
        .bind(limits.quiet_seconds)
        .bind(limits.marker_seconds)
        .fetch_all(pool)
        .await?)
}

#[derive(Debug, Clone, Copy)]
pub struct MonitorLimits {
    pub max_attempts: u32,
    pub quiet_seconds: f64,
    pub marker_seconds: f64,
}

/// Aggregate both cohorts from the same database snapshot and retain the full
/// denominator, including pending and held work.
pub async fn bulk_report(pool: &PgPool, id: Uuid) -> Result<serde_json::Value, WorkStoreError> {
    sqlx::query_scalar(include_str!("processing/bulk_report.sql"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(WorkStoreError::NotFound(id))
}
