//! Durable, separately leased cleanup metadata for consumed protected attempts.
//!
//! Database leases do not revoke filesystem descriptors. The private caller
//! MUST hold the exclusive per-attempt kernel lock across writer termination,
//! identity checks, filesystem cleanup, fsync and completion. This module
//! alone never proves that an attempt namespace is safe to remove.
use crate::{WorkItemStore, WorkStoreError, WorkerId};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
const MAX_PAGE: usize = 16;
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid protected cleanup request")
}
fn millis(value: Duration, max: i128) -> Result<i64, WorkStoreError> {
    let ms = value.whole_milliseconds();
    if !(1..=max).contains(&ms) {
        return Err(invalid());
    }
    i64::try_from(ms).map_err(|_| invalid())
}
/// A separate DB-clock cleanup fence. It is not proof that writers terminated.
#[derive(Debug, Clone)]
pub struct ProtectedCleanupLease {
    attempt: Uuid,
    baseline: Uuid,
    owner: String,
    token: Uuid,
    root_generation: String,
    namespace_name: String,
    namespace_generation: String,
    marker_generation: String,
    lock_generation: String,
}
impl ProtectedCleanupLease {
    pub fn attempt_id(&self) -> Uuid {
        self.attempt
    }
    pub fn baseline_id(&self) -> Uuid {
        self.baseline
    }
    pub fn root_generation(&self) -> &str {
        &self.root_generation
    }
    pub fn namespace_name(&self) -> &str {
        &self.namespace_name
    }
    pub fn namespace_generation(&self) -> &str {
        &self.namespace_generation
    }
    pub fn marker_generation(&self) -> &str {
        &self.marker_generation
    }
    pub fn lock_generation(&self) -> &str {
        &self.lock_generation
    }
    pub fn lock_name(&self) -> String {
        format!("{}.lock", self.namespace_name)
    }
}
#[derive(Debug, Clone, Copy)]
pub enum ProtectedCleanupRetryReason {
    Busy,
    Missing,
    Changed,
    Unreadable,
}
impl ProtectedCleanupRetryReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Missing => "missing",
            Self::Changed => "changed",
            Self::Unreadable => "unreadable",
        }
    }
}
#[derive(FromRow)]
struct ClaimRow {
    id: Uuid,
    consumed_baseline_id: Uuid,
    cleanup_token: Uuid,
    root_generation: String,
    namespace_name: String,
    namespace_generation: String,
    marker_generation: String,
    lock_generation: String,
}
async fn lock_cleanup(
    tx: &mut Transaction<'_, Postgres>,
    lease: &ProtectedCleanupLease,
) -> Result<(String, OffsetDateTime), WorkStoreError> {
    let row:Option<(String,Option<String>,Option<Uuid>,Option<OffsetDateTime>,Uuid,String,String,String,String,String)>=sqlx::query_as("SELECT state,cleanup_owner,cleanup_token,cleanup_expires_at,consumed_baseline_id,root_generation,namespace_name,namespace_generation,marker_generation,lock_generation FROM inbound_protected_stage_attempts WHERE id=$1 FOR UPDATE").bind(lease.attempt).fetch_optional(&mut **tx).await?;
    let Some((state, owner, token, expiry, baseline, root, name, namespace, marker, lock)) = row
    else {
        return Err(invalid());
    };
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if !matches!(state.as_str(), "consumed" | "cleanup_intent")
        || owner.as_deref() != Some(lease.owner.as_str())
        || token != Some(lease.token)
        || expiry.is_none_or(|e| e <= now)
        || baseline != lease.baseline
        || root != lease.root_generation
        || name != lease.namespace_name
        || namespace != lease.namespace_generation
        || marker != lease.marker_generation
        || lock != lease.lock_generation
    {
        return Err(invalid());
    }
    Ok((state, expiry.ok_or_else(invalid)?))
}
async fn end(
    tx: &mut Transaction<'_, Postgres>,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(invalid());
    }
    Ok(())
}
impl WorkItemStore {
    /// Claim one bounded oldest-due cleanup page using an independent fence.
    /// A DB claim does not permit filesystem access without the kernel lock.
    pub async fn claim_protected_stage_cleanup(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        limit: usize,
        duration: Duration,
    ) -> Result<Vec<ProtectedCleanupLease>, WorkStoreError> {
        if !(1..=MAX_PAGE).contains(&limit)
            || worker.as_str().is_empty()
            || worker.as_str().len() > 512
        {
            return Err(invalid());
        }
        let ms = millis(duration, 24 * 60 * 60 * 1000)?;
        let rows:Vec<ClaimRow>=sqlx::query_as("WITH candidate AS (SELECT id FROM inbound_protected_stage_attempts WHERE state IN ('consumed','cleanup_intent') AND cleanup_due_at<=statement_timestamp() AND (cleanup_token IS NULL OR cleanup_expires_at<=statement_timestamp()) ORDER BY cleanup_due_at,id LIMIT $2 FOR UPDATE SKIP LOCKED) UPDATE inbound_protected_stage_attempts a SET cleanup_owner=$1,cleanup_token=gen_random_uuid(),cleanup_expires_at=statement_timestamp()+($3::bigint*interval '1 millisecond'),cleanup_due_at=statement_timestamp()+($3::bigint*interval '1 millisecond'),updated_at=clock_timestamp() FROM candidate c WHERE a.id=c.id RETURNING a.id,a.consumed_baseline_id,a.cleanup_token,a.root_generation,a.namespace_name,a.namespace_generation,a.marker_generation,a.lock_generation").bind(worker.as_str()).bind(i64::try_from(limit).map_err(|_|invalid())?).bind(ms).fetch_all(pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| ProtectedCleanupLease {
                attempt: r.id,
                baseline: r.consumed_baseline_id,
                owner: worker.as_str().to_owned(),
                token: r.cleanup_token,
                root_generation: r.root_generation,
                namespace_name: r.namespace_name,
                namespace_generation: r.namespace_generation,
                marker_generation: r.marker_generation,
                lock_generation: r.lock_generation,
            })
            .collect())
    }
    /// Renew this independent cleanup lease without reviving an expired or
    /// replaced token. The writer lock remains held until work truly ends.
    pub async fn renew_protected_stage_cleanup(
        &self,
        pool: &PgPool,
        lease: &ProtectedCleanupLease,
        duration: Duration,
    ) -> Result<(), WorkStoreError> {
        let ms = millis(duration, 24 * 60 * 60 * 1000)?;
        let mut tx = pool.begin().await?;
        let (_, expiry) = lock_cleanup(&mut tx, lease).await?;
        let renewed:OffsetDateTime=sqlx::query_scalar("UPDATE inbound_protected_stage_attempts SET cleanup_expires_at=statement_timestamp()+($2::bigint*interval '1 millisecond'),cleanup_due_at=statement_timestamp()+($2::bigint*interval '1 millisecond'),updated_at=clock_timestamp() WHERE id=$1 RETURNING cleanup_expires_at").bind(lease.attempt).bind(ms).fetch_one(&mut *tx).await?;
        end(&mut tx, expiry).await?;
        end(&mut tx, renewed).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record intent only after acquiring the verified kernel writer fence and
    /// revalidating the private parent, lock, marker and namespace identities.
    pub async fn begin_protected_stage_cleanup(
        &self,
        pool: &PgPool,
        lease: &ProtectedCleanupLease,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let (state, expiry) = lock_cleanup(&mut tx, lease).await?;
        if state == "consumed" {
            sqlx::query("UPDATE inbound_protected_stage_attempts SET state='cleanup_intent',updated_at=clock_timestamp() WHERE id=$1").bind(lease.attempt).execute(&mut *tx).await?;
        }
        end(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Keep quota and evidence when cleanup cannot be completed unambiguously.
    pub async fn defer_protected_stage_cleanup(
        &self,
        pool: &PgPool,
        lease: &ProtectedCleanupLease,
        reason: ProtectedCleanupRetryReason,
        delay: Duration,
    ) -> Result<(), WorkStoreError> {
        let ms = millis(delay, 30_i128 * 24 * 60 * 60 * 1000)?;
        let mut tx = pool.begin().await?;
        let (_, expiry) = lock_cleanup(&mut tx, lease).await?;
        sqlx::query("UPDATE inbound_protected_stage_attempts SET cleanup_owner=NULL,cleanup_token=NULL,cleanup_expires_at=NULL,cleanup_due_at=clock_timestamp()+($2::bigint*interval '1 millisecond'),cleanup_retry_count=CASE WHEN cleanup_retry_count<2147483647 THEN cleanup_retry_count+1 ELSE cleanup_retry_count END,cleanup_last_reason=$3,updated_at=clock_timestamp() WHERE id=$1").bind(lease.attempt).bind(ms).bind(reason.as_str()).execute(&mut *tx).await?;
        end(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Finish only after the private caller, holding the original kernel lock,
    /// has verified removal and fsynced the affected directories. A crash with
    /// an absent/replaced lock or ambiguous namespace must instead quarantine.
    #[doc(hidden)]
    pub async fn complete_protected_stage_cleanup(
        &self,
        pool: &PgPool,
        lease: &ProtectedCleanupLease,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM inbound_protected_attempt_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let (state, expiry) = lock_cleanup(&mut tx, lease).await?;
        if state != "cleanup_intent" {
            return Err(invalid());
        }
        let (cost,session,root,parent):(i64,Uuid,String,String)=sqlx::query_as("SELECT logical_bytes,consumed_session_id,root_generation,promotion_parent_generation FROM inbound_protected_stage_attempts WHERE id=$1").bind(lease.attempt).fetch_one(&mut *tx).await?;
        let updated=sqlx::query("UPDATE inbound_retained_baselines SET cleaned_attempt_id=$2,cleaned_attempt_at=clock_timestamp(),cleaned_session_id=$3,cleaned_root_generation=$4,cleaned_final_parent_generation=$5 WHERE id=$1 AND state='authoritative' AND cleaned_attempt_id IS NULL AND cleaned_attempt_at IS NULL").bind(lease.baseline).bind(lease.attempt).bind(session).bind(root).bind(parent).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query("DELETE FROM inbound_protected_stage_attempts WHERE id=$1")
            .bind(lease.attempt)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE inbound_protected_attempt_budget SET attempts=attempts-1,bytes=bytes-$1 WHERE singleton").bind(cost).execute(&mut *tx).await?;
        end(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Read-only lost-ack check; it grants no filesystem deletion authority.
    pub async fn protected_stage_cleanup_completed(
        &self,
        pool: &PgPool,
        baseline: Uuid,
        attempt: Uuid,
    ) -> Result<bool, WorkStoreError> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baselines WHERE id=$1 AND state='authoritative' AND cleaned_attempt_id=$2 AND cleaned_attempt_at IS NOT NULL)").bind(baseline).bind(attempt).fetch_one(pool).await?)
    }
}
