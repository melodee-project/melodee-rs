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

/// Stateless entry point for operator pool control.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkerPoolStore;

impl WorkerPoolStore {
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
