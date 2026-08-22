//! Durable worker-pool pause/resume control (ADR-0134 §8, PRD-WORK-005).
//!
//! One row per pool in `worker_pool_state` is the multi-process-safe,
//! auditable control plane. Workers call [`is_pool_paused`] between claim
//! batches; operators mutate the row through [`WorkerPoolStore`], which is
//! also the CLI backend (PRD-CLI-002: the CLI uses the same durable state a
//! running server honors — it never signals or bypasses a worker).

use sqlx::{FromRow, PgPool};
use thiserror::Error;
use time::OffsetDateTime;

/// Pool row honored by the podcast download/refresh worker.
pub const POOL_PODCAST: &str = "podcast";
/// Pool row honored by the policy evaluation worker.
pub const POOL_POLICY: &str = "policy";
/// Pool row honored by the discovery maintenance worker.
pub const POOL_DISCOVERY: &str = "discovery";

/// Bound matching the `worker_pool_state_pool_check` migration constraint.
pub const MAX_POOL_NAME_LENGTH: usize = 64;
/// Bound for the operator identity recorded with a pause.
pub const MAX_POOL_ACTOR_LENGTH: usize = 200;

/// Durable pause/resume state for one worker pool. After a resume,
/// `paused_by`/`paused_at` remain as the last-pause audit.
#[derive(Clone, Debug, Eq, FromRow, PartialEq)]
pub struct WorkerPoolState {
    pub pool: String,
    pub paused: bool,
    pub paused_by: Option<String>,
    pub paused_at: Option<OffsetDateTime>,
    pub resume_requested_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Error)]
pub enum WorkerPoolStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("worker pool name must match ^[a-z][a-z0-9_-]{{0,63}}$")]
    InvalidPoolName,
    #[error("a non-blank operator identity (--by) is required")]
    MissingActor,
}

/// A pool name is a stable lowercase machine identifier shared by the
/// migration seed, the workers, and the CLI.
#[must_use]
pub fn is_valid_pool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && name.len() <= MAX_POOL_NAME_LENGTH
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Whether the named pool is durably paused. A pool without a row is never
/// paused, so a worker wired before its row exists keeps running.
///
/// The only failure channel is the database itself; pool names at worker
/// call sites are compile-time constants.
pub async fn is_pool_paused(pool: &PgPool, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT paused FROM worker_pool_state WHERE pool=$1")
        .bind(name)
        .fetch_optional(pool)
        .await
        .map(|paused| paused.unwrap_or(false))
}

/// Per-pool work-queue metrics derived from durable state.
///
/// Only pools whose worker owns a durable queue table report metrics:
/// `policy` reads `release_work_items`/`release_work_item_events`, `podcast`
/// reads `native_podcast_episodes`. Pools without a durable queue (discovery
/// maintenance runs bounded idempotent passes) report `None` — the API
/// surfaces that honestly instead of inventing zeros. "Running" is lease
/// evidence (an unexpired lease exists), not a live-worker liveness claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkerPoolMetrics {
    /// Work waiting to be claimed (no active lease).
    pub queued: i64,
    /// Work under an unexpired lease right now.
    pub running: i64,
    /// Last update of the oldest queued item (queue age evidence).
    pub oldest_queued_at: Option<OffsetDateTime>,
    /// Items completed by this pool in the last hour.
    pub completed_last_hour: i64,
    /// Failures recorded by this pool in the last 24 hours.
    pub failed_last_24h: i64,
}

/// Stateless entry point for operator pool control.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkerPoolStore;

impl WorkerPoolStore {
    /// lease-health probe: leases that expired more than `grace` ago on
    /// claimed queue rows. An expired lease is normally reclaimed on the next
    /// claim pass, so a count that survives past the grace window is evidence
    /// of a dead or stalled worker, not ordinary poll lag. Bounded aggregate
    /// over the two durable queue tables (work items and podcast episodes).
    pub async fn stale_lease_count(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
        grace: time::Duration,
    ) -> Result<i64, WorkerPoolStoreError> {
        let cutoff = now - grace;
        let work_items: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::bigint FROM release_work_items
             WHERE lease_expires_at IS NOT NULL AND lease_expires_at < $1",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await?;
        let episodes: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::bigint FROM native_podcast_episodes
             WHERE soft_deleted_at IS NULL
               AND download_lease_expires_at IS NOT NULL
               AND download_lease_expires_at < $1",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await?;
        Ok(work_items.0 + episodes.0)
    }

    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Lists every registered pool, ordered by name (bounded by the pool
    /// namespace: one row per wired or operator-registered pool).
    pub async fn list(&self, pool: &PgPool) -> Result<Vec<WorkerPoolState>, WorkerPoolStoreError> {
        Ok(sqlx::query_as::<_, WorkerPoolState>(
            "SELECT pool,paused,paused_by,paused_at,resume_requested_at,updated_at
             FROM worker_pool_state ORDER BY pool",
        )
        .fetch_all(pool)
        .await?)
    }

    pub async fn get(
        &self,
        pool: &PgPool,
        name: &str,
    ) -> Result<Option<WorkerPoolState>, WorkerPoolStoreError> {
        if !is_valid_pool_name(name) {
            return Err(WorkerPoolStoreError::InvalidPoolName);
        }
        Ok(sqlx::query_as::<_, WorkerPoolState>(
            "SELECT pool,paused,paused_by,paused_at,resume_requested_at,updated_at
             FROM worker_pool_state WHERE pool=$1",
        )
        .bind(name)
        .fetch_optional(pool)
        .await?)
    }

    /// Set-based queue metrics for one pool, or `None` when the pool has no
    /// durable queue table (see [`WorkerPoolMetrics`]).
    pub async fn metrics(
        &self,
        pool: &PgPool,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<Option<WorkerPoolMetrics>, WorkerPoolStoreError> {
        if !is_valid_pool_name(name) {
            return Err(WorkerPoolStoreError::InvalidPoolName);
        }
        let hour_ago = now - time::Duration::hours(1);
        let day_ago = now - time::Duration::hours(24);
        match name {
            POOL_POLICY => {
                let queue: (i64, i64, Option<OffsetDateTime>) = sqlx::query_as(
                    "SELECT
                         COUNT(*) FILTER (WHERE lease_expires_at IS NULL OR lease_expires_at <= $1)::bigint,
                         COUNT(*) FILTER (WHERE lease_expires_at > $1)::bigint,
                         MIN(updated_at) FILTER (WHERE lease_expires_at IS NULL OR lease_expires_at <= $1)
                     FROM release_work_items WHERE state = 'cataloged'",
                )
                .bind(now)
                .fetch_one(pool)
                .await?;
                let events: (i64, i64) = sqlx::query_as(
                    "SELECT
                         COUNT(*) FILTER (WHERE event_type = 'transitioned'
                              AND reason_code = 'policy.decision_applied'
                              AND created_at >= $1)::bigint,
                         COUNT(*) FILTER (WHERE event_type = 'failed'
                              AND reason_code LIKE 'policy.%'
                              AND created_at >= $2)::bigint
                     FROM release_work_item_events",
                )
                .bind(hour_ago)
                .bind(day_ago)
                .fetch_one(pool)
                .await?;
                Ok(Some(WorkerPoolMetrics {
                    queued: queue.0,
                    running: queue.1,
                    oldest_queued_at: queue.2,
                    completed_last_hour: events.0,
                    failed_last_24h: events.1,
                }))
            }
            POOL_PODCAST => {
                let row: (i64, i64, Option<OffsetDateTime>, i64, i64) = sqlx::query_as(
                    "SELECT
                         COUNT(*) FILTER (WHERE download_status = 'Queued')::bigint,
                         COUNT(*) FILTER (WHERE download_status = 'Downloading'
                              AND download_lease_expires_at > $1)::bigint,
                         MIN(updated_at) FILTER (WHERE download_status = 'Queued'),
                         COUNT(*) FILTER (WHERE download_status = 'Downloaded'
                              AND updated_at >= $2)::bigint,
                         COUNT(*) FILTER (WHERE download_status = 'Failed'
                              AND updated_at >= $3)::bigint
                     FROM native_podcast_episodes
                     WHERE soft_deleted_at IS NULL",
                )
                .bind(now)
                .bind(hour_ago)
                .bind(day_ago)
                .fetch_one(pool)
                .await?;
                Ok(Some(WorkerPoolMetrics {
                    queued: row.0,
                    running: row.1,
                    oldest_queued_at: row.2,
                    completed_last_hour: row.3,
                    failed_last_24h: row.4,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Durably pauses or resumes a pool, recording the operator audit.
    ///
    /// Pause sets `paused_by`/`paused_at`; resume keeps them as the
    /// last-pause history and stamps `resume_requested_at`. Unknown pool
    /// names are upserted so a pool wired in a later binary is paused the
    /// moment it first checks its row. Idempotent: re-pausing an already
    /// paused pool refreshes the audit.
    pub async fn set_paused(
        &self,
        pool: &PgPool,
        name: &str,
        paused: bool,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<WorkerPoolState, WorkerPoolStoreError> {
        if !is_valid_pool_name(name) {
            return Err(WorkerPoolStoreError::InvalidPoolName);
        }
        if actor.trim().is_empty() || actor.len() > MAX_POOL_ACTOR_LENGTH {
            return Err(WorkerPoolStoreError::MissingActor);
        }
        Ok(sqlx::query_as::<_, WorkerPoolState>(
            "INSERT INTO worker_pool_state (pool,paused,paused_by,paused_at,resume_requested_at,updated_at)
             VALUES ($1,$2,CASE WHEN $2 THEN $3 END,CASE WHEN $2 THEN $4 END,CASE WHEN $2 THEN NULL ELSE $4 END,$4)
             ON CONFLICT (pool) DO UPDATE SET
               paused=EXCLUDED.paused,
               paused_by=CASE WHEN EXCLUDED.paused THEN EXCLUDED.paused_by ELSE worker_pool_state.paused_by END,
               paused_at=CASE WHEN EXCLUDED.paused THEN EXCLUDED.paused_at ELSE worker_pool_state.paused_at END,
               resume_requested_at=CASE WHEN EXCLUDED.paused
                   THEN worker_pool_state.resume_requested_at ELSE EXCLUDED.resume_requested_at END,
               updated_at=EXCLUDED.updated_at
             RETURNING pool,paused,paused_by,paused_at,resume_requested_at,updated_at",
        )
        .bind(name)
        .bind(paused)
        .bind(actor)
        .bind(now)
        .fetch_one(pool)
        .await?)
    }
}
