//! Durable scheduled-job tick history and retention audit (ADR-0158).

use serde::Serialize;
use sqlx::{FromRow, PgPool};
use std::time::Duration;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

pub const MAX_JOB_HISTORY_PAGE: u32 = 500;
pub const DEFAULT_JOB_HISTORY_RETENTION_DAYS: i32 = 90;

/// Returns the UTC epoch-aligned slot for a scheduler cadence. Aligning the
/// slot before the durable unique insert makes two processes that wake for the
/// same cadence compete on one database key instead of both running the tick.
#[must_use]
pub fn scheduled_tick(now: OffsetDateTime, cadence: Duration) -> OffsetDateTime {
    let seconds = i64::try_from(cadence.as_secs().max(1)).unwrap_or(i64::MAX);
    let epoch = now.unix_timestamp();
    let bucket = epoch - epoch.rem_euclid(seconds);
    OffsetDateTime::from_unix_timestamp(bucket).unwrap_or(now)
}

#[derive(Clone, Debug, FromRow, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRunHistory {
    pub id: Uuid,
    pub job_name: String,
    pub scheduled_tick_at: Option<OffsetDateTime>,
    pub started_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
    pub duration_ms: Option<f64>,
    pub success: Option<bool>,
    pub error_message: Option<String>,
    pub was_manual_trigger: bool,
    pub triggered_by: Option<String>,
}

#[derive(Debug, Error)]
pub enum JobHistoryStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("invalid job name")]
    InvalidJobName,
    #[error("invalid page limit")]
    InvalidLimit,
    #[error("invalid retention or actor")]
    InvalidRetention,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JobRunHistoryStore;

impl JobRunHistoryStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn valid_name(job_name: &str) -> bool {
        let mut chars = job_name.chars();
        chars.next().is_some_and(|first| first.is_ascii_lowercase())
            && job_name.len() <= 64
            && chars.all(|value| {
                value.is_ascii_lowercase() || value.is_ascii_digit() || value == '_' || value == '-'
            })
    }

    pub async fn start_tick(
        &self,
        pool: &PgPool,
        job_name: &str,
        scheduled_tick_at: OffsetDateTime,
    ) -> Result<Option<JobRunHistory>, JobHistoryStoreError> {
        if !Self::valid_name(job_name) {
            return Err(JobHistoryStoreError::InvalidJobName);
        }
        let mut tx = pool.begin().await?;
        // The unique index makes the claim durable; the advisory lock keeps
        // concurrent processes from doing duplicate scheduler work between
        // their wake-up and the insert's conflict resolution.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(job_name)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query_as::<_, JobRunHistory>(
            "INSERT INTO job_run_history
                (id,job_name,scheduled_tick_at,started_at,was_manual_trigger)
             VALUES ($1,$2,$3,$3,false)
             ON CONFLICT (job_name, scheduled_tick_at) WHERE scheduled_tick_at IS NOT NULL AND was_manual_trigger = false
             DO NOTHING
             RETURNING id,job_name,scheduled_tick_at,started_at,completed_at,duration_ms,success,error_message,was_manual_trigger,triggered_by",
        )
        .bind(Uuid::now_v7())
        .bind(job_name)
        .bind(scheduled_tick_at)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn start_manual(
        &self,
        pool: &PgPool,
        job_name: &str,
        actor: &str,
        started_at: OffsetDateTime,
    ) -> Result<JobRunHistory, JobHistoryStoreError> {
        if !Self::valid_name(job_name) || actor.trim().is_empty() || actor.len() > 200 {
            return Err(JobHistoryStoreError::InvalidJobName);
        }
        Ok(sqlx::query_as::<_, JobRunHistory>(
            "INSERT INTO job_run_history
                (id,job_name,started_at,was_manual_trigger,triggered_by)
             VALUES ($1,$2,$3,true,$4)
             RETURNING id,job_name,scheduled_tick_at,started_at,completed_at,duration_ms,success,error_message,was_manual_trigger,triggered_by",
        )
        .bind(Uuid::now_v7())
        .bind(job_name)
        .bind(started_at)
        .bind(actor)
        .fetch_one(pool)
        .await?)
    }

    pub async fn finish(
        &self,
        pool: &PgPool,
        id: Uuid,
        completed_at: OffsetDateTime,
        success: bool,
        error_message: Option<&str>,
    ) -> Result<JobRunHistory, JobHistoryStoreError> {
        let row = sqlx::query_as::<_, JobRunHistory>(
            "UPDATE job_run_history
             SET completed_at=$2,
                 duration_ms=GREATEST(0, EXTRACT(EPOCH FROM ($2 - started_at)) * 1000),
                 success=$3,
                 error_message=$4
             WHERE id=$1 AND completed_at IS NULL
             RETURNING id,job_name,scheduled_tick_at,started_at,completed_at,duration_ms,success,error_message,was_manual_trigger,triggered_by",
        )
        .bind(id)
        .bind(completed_at)
        .bind(success)
        .bind(error_message)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn list(
        &self,
        pool: &PgPool,
        job_name: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<JobRunHistory>, i64), JobHistoryStoreError> {
        if limit == 0 || limit > MAX_JOB_HISTORY_PAGE || offset > 1_000_000 {
            return Err(JobHistoryStoreError::InvalidLimit);
        }
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT count(*)::bigint FROM job_run_history WHERE ($1::text IS NULL OR job_name=$1)",
        )
        .bind(job_name)
        .fetch_one(pool)
        .await?;
        let rows = sqlx::query_as::<_, JobRunHistory>(
            "SELECT id,job_name,scheduled_tick_at,started_at,completed_at,duration_ms,success,error_message,was_manual_trigger,triggered_by
             FROM job_run_history
             WHERE ($1::text IS NULL OR job_name=$1)
             ORDER BY started_at DESC,id DESC LIMIT $2 OFFSET $3",
        )
        .bind(job_name)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((rows, total))
    }

    pub async fn failure_count_since(
        &self,
        pool: &PgPool,
        since: OffsetDateTime,
    ) -> Result<i64, JobHistoryStoreError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*)::bigint FROM job_run_history WHERE started_at >= $1 AND success=false",
        )
        .bind(since)
        .fetch_one(pool)
        .await?)
    }

    pub async fn cleanup(
        &self,
        pool: &PgPool,
        retention_days: i32,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<i64, JobHistoryStoreError> {
        if retention_days <= 0 || actor.trim().is_empty() || actor.len() > 200 {
            return Err(JobHistoryStoreError::InvalidRetention);
        }
        let mut tx = pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM job_run_history WHERE started_at < $1 AND completed_at IS NOT NULL",
        )
        .bind(now - time::Duration::days(i64::from(retention_days)))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "INSERT INTO job_run_history_cleanup_audit (id,retention_days,removed_count,actor,created_at)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(Uuid::now_v7())
        .bind(retention_days)
        .bind(i64::try_from(removed).unwrap_or(i64::MAX))
        .bind(actor)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(i64::try_from(removed).unwrap_or(i64::MAX))
    }
}
