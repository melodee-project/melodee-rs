//! Durable, independently leased observation of retained inbound occurrences.
//!
//! A retained source is never removed by this module. Two ordered, matching
//! content observations are required before either rescheduling an unchanged
//! occurrence or atomically handing a changed occurrence back to discovery.

use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{CustodyHashPurpose, SealedCustodyHash, WorkItemStore, WorkStoreError, WorkerId};

/// Maximum retained-obligation records returned by one operator query.
pub const MAX_RETAINED_OBLIGATION_LIST: usize = 128;
/// A read-only active obligation. `source_path` is the exact stored reversible
/// occurrence representation, not a lossy or display-normalized path.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct RetainedObligationView {
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub baseline_id: Option<Uuid>,
    pub baseline_change_pending: bool,
    pub baseline_unreadable_reason: Option<String>,
    pub baseline_unreadable_count: i32,
    pub last_retry_reason: Option<String>,
    pub retry_count: i32,
    pub baseline_directory_unavailable_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub baseline_directory_blocked_until: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub next_reobserve_at: OffsetDateTime,
    pub lease_owner: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub lease_expires_at: Option<OffsetDateTime>,
    pub lease_active: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// The scheduler must allocate its own per-class budget and may not claim an
/// unbounded collection even if many retained sources are due.
pub const MAX_RETAINED_SOURCE_CLAIM: usize = 32;
const MAX_LEASE_MILLIS: i128 = 24 * 60 * 60 * 1000;
const MAX_RECHECK_MILLIS: i128 = 30 * 24 * 60 * 60 * 1000;

/// A DB-issued retained-observation fence. Its random token is distinct from
/// the processing work-item lease and changes on every reclaim.
#[derive(Debug, Clone)]
pub struct RetainedSourceLease {
    item_id: Uuid,
    source_library_id: Option<Uuid>,
    baseline_id: Option<Uuid>,
    baseline_directory_turn: bool,
    source_path: String,
    source_path_format: i16,
    baseline_generation: String,
    baseline_digest: String,
    owner: String,
    token: Uuid,
    expires_at: OffsetDateTime,
    due_at: OffsetDateTime,
}

impl RetainedSourceLease {
    #[must_use]
    pub fn work_item_id(&self) -> Uuid {
        self.item_id
    }
    #[must_use]
    pub fn source_library_id(&self) -> Option<Uuid> {
        self.source_library_id
    }
    /// The optional bound baseline identity; callers must still verify its
    /// authoritative state through the fenced baseline reader.
    #[must_use]
    pub fn baseline_id(&self) -> Option<Uuid> {
        self.baseline_id
    }
    /// Selects the independently budgeted directory class for this turn.
    #[must_use]
    pub fn baseline_directory_turn(&self) -> bool {
        self.baseline_directory_turn
    }
    #[must_use]
    pub fn source_path(&self) -> &str {
        &self.source_path
    }
    #[must_use]
    pub fn source_path_format(&self) -> i16 {
        self.source_path_format
    }
    #[must_use]
    pub fn baseline_generation(&self) -> &str {
        &self.baseline_generation
    }
    #[must_use]
    pub fn baseline_digest(&self) -> &str {
        &self.baseline_digest
    }
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }
    #[must_use]
    pub fn token(&self) -> Uuid {
        self.token
    }
    /// Expiry at claim time. A successful renewal returns the updated expiry;
    /// this immutable snapshot is not updated by subsequent renewals.
    #[must_use]
    pub fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }
}

/// A store-issued first observation. It does not by itself establish that the
/// source is unchanged or authorize work-item mutation.
#[derive(Debug, Clone)]
pub struct RetainedRecheckSession {
    id: Uuid,
    item_id: Uuid,
    lease_token: Uuid,
    capture_id: Uuid,
    before_epoch: Uuid,
    digest: String,
    entries: i64,
    generation: String,
    root_generation: String,
}

impl RetainedRecheckSession {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedRetryReason {
    Missing,
    Unreadable,
    Unstable,
    Capacity,
    Ambiguous,
    AwaitingIntake,
    Other,
}

impl RetainedRetryReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::Unstable => "unstable",
            Self::Capacity => "capacity",
            Self::Ambiguous => "ambiguous",
            Self::AwaitingIntake => "awaiting_intake",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedRecheckOutcome {
    Unchanged,
    Superseded { work_item_id: Uuid, created: bool },
}

#[derive(FromRow)]
struct ClaimedRow {
    work_item_id: Uuid,
    source_library_id: Option<Uuid>,
    baseline_id: Option<Uuid>,
    baseline_directory_turn: bool,
    source_path: String,
    source_path_format: i16,
    source_generation: String,
    content_digest: String,
    lease_token: Uuid,
    lease_expires_at: OffsetDateTime,
    next_reobserve_at: OffsetDateTime,
}

fn duration_millis(duration: Duration, maximum: i128) -> Result<i64, WorkStoreError> {
    let millis = duration.whole_milliseconds();
    if !(1..=maximum).contains(&millis) {
        return Err(WorkStoreError::InvalidRequest(
            "invalid retained-source duration",
        ));
    }
    i64::try_from(millis)
        .map_err(|_| WorkStoreError::InvalidRequest("invalid retained-source duration"))
}

fn valid_worker(worker: &WorkerId) -> Result<(), WorkStoreError> {
    if worker.as_str().is_empty() || worker.as_str().len() > 512 {
        return Err(WorkStoreError::InvalidRequest(
            "invalid retained-source owner",
        ));
    }
    Ok(())
}

/// Lock and validate the independent lease using the database clock *after*
/// the row lock. A queued operation cannot revive or acknowledge an expired
/// lease even when its caller supplied an old local timestamp.
pub(crate) async fn lock_retained_source_lease(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
) -> Result<OffsetDateTime, WorkStoreError> {
    #[allow(clippy::type_complexity)]
    let row: Option<(Option<Uuid>, String, i16, String, String, String, Option<String>, Option<Uuid>, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT source_library_id,source_path,source_path_format,source_generation,content_digest,status,lease_owner,lease_token,lease_expires_at FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE",
    ).bind(lease.item_id).fetch_optional(&mut **tx).await?;
    let Some((library, path, path_format, generation, digest, status, owner, token, expires)) = row
    else {
        return Err(WorkStoreError::LeaseExpired(lease.item_id));
    };
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if library != lease.source_library_id
        || path != lease.source_path
        || path_format != lease.source_path_format
        || generation != lease.baseline_generation
        || digest != lease.baseline_digest
        || status != "active"
        || owner.as_deref() != Some(lease.owner.as_str())
        || token != Some(lease.token)
        || expires.is_none_or(|value| value <= now)
    {
        return Err(WorkStoreError::LeaseExpired(lease.item_id));
    }
    expires.ok_or(WorkStoreError::LeaseExpired(lease.item_id))
}

pub(crate) async fn ensure_retained_source_lease_live(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar(
        "SELECT COALESCE(status='active' AND lease_owner=$2 AND lease_token=$3 AND lease_expires_at>clock_timestamp(),false) FROM inbound_retained_sources WHERE work_item_id=$1",
    ).bind(lease.item_id).bind(&lease.owner).bind(lease.token).fetch_optional(&mut **tx).await?.unwrap_or(false);
    if !live {
        return Err(WorkStoreError::LeaseExpired(lease.item_id));
    }
    Ok(())
}

/// Check the original locked expiry after a potentially blocked mutation.
/// A renewal or token release must not make an already-expired lease appear
/// valid at commit time.
async fn ensure_original_expiry_live(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    original_expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz > clock_timestamp()")
        .bind(original_expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(WorkStoreError::LeaseExpired(item));
    }
    Ok(())
}

fn validate_retained_seal_binding(
    lease: &RetainedSourceLease,
    seal: &SealedCustodyHash,
) -> Result<(), WorkStoreError> {
    if seal.item_id() != lease.item_id
        || seal.owner() != lease.owner
        || seal.purpose() != CustodyHashPurpose::Retained
        || seal.retained_lease_token() != Some(lease.token)
        || seal.target_binding() != lease.source_path
        || seal.entries() < 0
    {
        return Err(WorkStoreError::InvalidRequest(
            "retained-source observation binding changed",
        ));
    }
    Ok(())
}

async fn lock_retained_seal(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
    seal: &SealedCustodyHash,
) -> Result<(), WorkStoreError> {
    validate_retained_seal_binding(lease, seal)?;
    let present: Option<bool> = sqlx::query_scalar(
        "SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND retained_lease_token=$4 AND purpose='retained' AND source_generation=$5 AND target_binding COLLATE \"C\"=$6 COLLATE \"C\" AND state='frozen' AND observation_epoch=$7 AND sealed_epoch=$7 AND sealed_digest=$8 AND entry_count=$9 FOR UPDATE",
    ).bind(seal.capture_id()).bind(lease.item_id).bind(&lease.owner).bind(lease.token)
     .bind(seal.generation()).bind(&lease.source_path).bind(seal.epoch()).bind(seal.digest()).bind(seal.entries())
     .fetch_optional(&mut **tx).await?;
    if present.is_none() {
        return Err(WorkStoreError::InvalidRequest(
            "retained-source observation is stale",
        ));
    }
    Ok(())
}

#[derive(FromRow)]
struct CompletionRow {
    source_library_id: Option<Uuid>,
    source_path: String,
    source_path_format: i16,
    source_generation: String,
    content_digest: String,
    completed_lease_token: Option<Uuid>,
    completed_session_id: Option<Uuid>,
    completed_epoch: Option<Uuid>,
    completed_digest: Option<String>,
    completed_generation: Option<String>,
    completed_outcome: Option<String>,
    completed_work_item_id: Option<Uuid>,
    completed_created: Option<bool>,
}

impl WorkItemStore {
    /// List active obligations in durable keyset order without mutating them.
    /// The exact path encoding and its format are returned together.
    pub async fn list_retained_obligations(
        &self,
        pool: &PgPool,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<RetainedObligationView>, WorkStoreError> {
        if !(1..=MAX_RETAINED_OBLIGATION_LIST).contains(&limit) {
            return Err(WorkStoreError::InvalidRequest(
                "invalid retained obligation page limit",
            ));
        }
        let limit = i64::try_from(limit).map_err(|_| {
            WorkStoreError::InvalidRequest("invalid retained obligation page limit")
        })?;
        Ok(sqlx::query_as("SELECT work_item_id,source_library_id,source_path,source_path_format,baseline_id,baseline_change_pending,baseline_unreadable_reason,baseline_unreadable_count,last_retry_reason,retry_count,baseline_directory_unavailable_reason,baseline_directory_blocked_until,next_reobserve_at,lease_owner,lease_expires_at,COALESCE(lease_owner IS NOT NULL AND lease_expires_at>clock_timestamp(),false) AS lease_active,created_at,updated_at FROM inbound_retained_sources WHERE status='active' AND ($1::uuid IS NULL OR work_item_id>$1) ORDER BY work_item_id LIMIT $2").bind(after).bind(limit).fetch_all(pool).await?)
    }

    /// Claim a bounded oldest-due page. The due ordering is durable; failed or
    /// interrupted work becomes eligible after its retry time or lease expiry.
    pub async fn claim_due_retained_sources(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        limit: usize,
        lease_duration: Duration,
    ) -> Result<Vec<RetainedSourceLease>, WorkStoreError> {
        valid_worker(worker)?;
        if !(1..=MAX_RETAINED_SOURCE_CLAIM).contains(&limit) {
            return Err(WorkStoreError::InvalidRequest(
                "invalid retained-source claim limit",
            ));
        }
        let millis = duration_millis(lease_duration, MAX_LEASE_MILLIS)?;
        let rows: Vec<ClaimedRow> = sqlx::query_as(
            "WITH candidate AS (SELECT work_item_id,next_reobserve_at AS original_due FROM inbound_retained_sources WHERE status='active' AND (source_path_format=1 OR strpos(source_path,chr(65533))=0) AND next_reobserve_at<=statement_timestamp() AND (lease_token IS NULL OR lease_expires_at<=statement_timestamp()) ORDER BY next_reobserve_at,work_item_id LIMIT $2 FOR UPDATE SKIP LOCKED) UPDATE inbound_retained_sources r SET lease_owner=$1,lease_token=gen_random_uuid(),lease_expires_at=statement_timestamp()+($3::bigint*interval '1 millisecond'),next_reobserve_at=statement_timestamp()+($3::bigint*interval '1 millisecond'),updated_at=clock_timestamp() FROM candidate c WHERE r.work_item_id=c.work_item_id RETURNING r.work_item_id,r.source_library_id,r.baseline_id,CASE WHEN r.baseline_id IS NULL THEN r.baseline_directory_turn WHEN r.baseline_file_due_at<=statement_timestamp() AND COALESCE(r.baseline_directory_blocked_until<=statement_timestamp(),true) AND EXISTS(SELECT 1 FROM inbound_retained_baseline_directories d WHERE d.baseline_id=r.baseline_id AND d.next_scan_at<=statement_timestamp()) THEN r.baseline_directory_turn WHEN COALESCE(r.baseline_directory_blocked_until<=statement_timestamp(),true) AND EXISTS(SELECT 1 FROM inbound_retained_baseline_directories d WHERE d.baseline_id=r.baseline_id AND d.next_scan_at<=statement_timestamp()) THEN true ELSE false END AS baseline_directory_turn,r.source_path,r.source_path_format,r.source_generation,r.content_digest,r.lease_token,r.lease_expires_at,c.original_due AS next_reobserve_at",
        ).bind(worker.as_str()).bind(i64::try_from(limit).map_err(|_| WorkStoreError::InvalidRequest("invalid retained-source claim limit"))?).bind(millis)
         .fetch_all(pool).await?;
        let mut leases: Vec<_> = rows
            .into_iter()
            .map(|row| RetainedSourceLease {
                item_id: row.work_item_id,
                source_library_id: row.source_library_id,
                baseline_id: row.baseline_id,
                baseline_directory_turn: row.baseline_directory_turn,
                source_path: row.source_path,
                source_path_format: row.source_path_format,
                baseline_generation: row.source_generation,
                baseline_digest: row.content_digest,
                owner: worker.as_str().to_owned(),
                token: row.lease_token,
                expires_at: row.lease_expires_at,
                due_at: row.next_reobserve_at,
            })
            .collect();
        leases.sort_unstable_by_key(|entry| (entry.due_at, entry.item_id));
        Ok(leases)
    }

    /// Renew only an unexpired fence. On cancellation the transaction rolls back.
    pub async fn renew_retained_source_lease(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        duration: Duration,
    ) -> Result<OffsetDateTime, WorkStoreError> {
        let millis = duration_millis(duration, MAX_LEASE_MILLIS)?;
        let mut tx = pool.begin().await?;
        let original_expiry = lock_retained_source_lease(&mut tx, lease).await?;
        let expiry: OffsetDateTime = sqlx::query_scalar("UPDATE inbound_retained_sources SET lease_expires_at=statement_timestamp()+($2::bigint*interval '1 millisecond'),next_reobserve_at=statement_timestamp()+($2::bigint*interval '1 millisecond'),updated_at=clock_timestamp() WHERE work_item_id=$1 RETURNING lease_expires_at")
            .bind(lease.item_id).bind(millis).fetch_one(&mut *tx).await?;
        ensure_original_expiry_live(&mut tx, lease.item_id, original_expiry).await?;
        tx.commit().await?;
        Ok(expiry)
    }

    /// Yield a completed bounded cursor turn without recording a failure.
    /// The cursor and pending change signal remain durable for the next turn.
    pub async fn yield_retained_source(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        delay: Duration,
    ) -> Result<(), WorkStoreError> {
        let millis = duration_millis(delay, MAX_RECHECK_MILLIS)?;
        let mut tx = pool.begin().await?;
        let original_expiry = lock_retained_source_lease(&mut tx, lease).await?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)")
            .bind(lease.item_id).bind(lease.token).fetch_one(&mut *tx).await?;
        if pending {
            return Err(WorkStoreError::InvalidRequest(
                "cannot yield an active retained proof session",
            ));
        }
        sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()+($2::bigint*interval '1 millisecond'),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE work_item_id=$1")
            .bind(lease.item_id).bind(millis).execute(&mut *tx).await?;
        ensure_original_expiry_live(&mut tx, lease.item_id, original_expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Yield a baseline turn to its earliest independently due work class.
    pub async fn yield_retained_baseline_source(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        minimum_delay: Duration,
    ) -> Result<(), WorkStoreError> {
        let millis = duration_millis(minimum_delay, MAX_RECHECK_MILLIS)?;
        let mut tx = pool.begin().await?;
        let expiry = lock_retained_source_lease(&mut tx, lease).await?;
        if lease.baseline_id.is_none() {
            return Err(WorkStoreError::InvalidRequest("retained baseline required"));
        }
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)").bind(lease.item_id).bind(lease.token).fetch_one(&mut *tx).await?;
        if pending {
            return Err(WorkStoreError::InvalidRequest(
                "cannot yield an active retained proof session",
            ));
        }
        sqlx::query("UPDATE inbound_retained_sources r SET next_reobserve_at=GREATEST(clock_timestamp()+($2::bigint*interval '1 millisecond'),LEAST(r.baseline_file_due_at,GREATEST(COALESCE(r.baseline_directory_blocked_until,'-infinity'::timestamptz),COALESCE((SELECT d.next_scan_at FROM inbound_retained_baseline_directories d WHERE d.baseline_id=r.baseline_id ORDER BY d.next_scan_at,d.stage_ordinal LIMIT 1),'infinity'::timestamptz)))),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE r.work_item_id=$1").bind(lease.item_id).bind(millis).execute(&mut *tx).await?;
        ensure_original_expiry_live(&mut tx, lease.item_id, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Retry missing, unreadable or unstable observations without declaring the
    /// source absent or deleting any source bytes.
    pub async fn defer_retained_source(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        retry_after: Duration,
        reason: RetainedRetryReason,
    ) -> Result<(), WorkStoreError> {
        let millis = duration_millis(retry_after, MAX_RECHECK_MILLIS)?;
        let mut tx = pool.begin().await?;
        let original_expiry = lock_retained_source_lease(&mut tx, lease).await?;
        sqlx::query("DELETE FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2")
            .bind(lease.item_id).bind(lease.token).execute(&mut *tx).await?;
        sqlx::query("UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp()+($2::bigint*interval '1 millisecond'),retry_count=CASE WHEN retry_count<2147483647 THEN retry_count+1 ELSE retry_count END,last_retry_reason=$3,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE work_item_id=$1")
            .bind(lease.item_id).bind(millis).bind(reason.as_str()).execute(&mut *tx).await?;
        ensure_original_expiry_live(&mut tx, lease.item_id, original_expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persist the first complete observation under the independent lease. A
    /// matching second epoch is required before acknowledging any work.
    pub async fn begin_retained_recheck(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        before: &SealedCustodyHash,
        root_generation: &str,
    ) -> Result<RetainedRecheckSession, WorkStoreError> {
        if root_generation.is_empty()
            || root_generation.len() > 512
            || root_generation.contains('\0')
        {
            return Err(WorkStoreError::InvalidRequest(
                "invalid retained-source root generation",
            ));
        }
        let mut tx = pool.begin().await?;
        lock_retained_source_lease(&mut tx, lease).await?;
        lock_retained_seal(&mut tx, lease, before).await?;
        #[allow(clippy::type_complexity)]
        let existing: Option<(Uuid, Uuid, Uuid, Uuid, String, i64, String, String)> = sqlx::query_as(
            "SELECT session_id,lease_token,capture_id,before_epoch,before_digest,before_entries,source_generation,source_root_generation FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 FOR UPDATE",
        ).bind(lease.item_id).fetch_optional(&mut *tx).await?;
        let id = if let Some((id, token, capture, epoch, digest, entries, generation, root)) =
            existing
        {
            if token == lease.token {
                if capture != before.capture_id()
                    || epoch != before.epoch()
                    || digest != before.digest()
                    || entries != before.entries()
                    || generation != before.generation()
                    || root != root_generation
                {
                    return Err(WorkStoreError::InvalidRequest(
                        "retained-source recheck already started",
                    ));
                }
                id
            } else {
                let id = Uuid::now_v7();
                sqlx::query("UPDATE inbound_retained_recheck_sessions SET session_id=$2,lease_token=$3,capture_id=$4,before_epoch=$5,before_digest=$6,before_entries=$7,source_generation=$8,source_root_generation=$9,created_at=clock_timestamp() WHERE work_item_id=$1")
                    .bind(lease.item_id).bind(id).bind(lease.token).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).bind(before.entries()).bind(before.generation()).bind(root_generation).execute(&mut *tx).await?;
                id
            }
        } else {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO inbound_retained_recheck_sessions(work_item_id,session_id,lease_token,capture_id,before_epoch,before_digest,before_entries,source_generation,source_root_generation,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,clock_timestamp())")
                .bind(lease.item_id).bind(id).bind(lease.token).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).bind(before.entries()).bind(before.generation()).bind(root_generation).execute(&mut *tx).await?;
            id
        };
        ensure_retained_source_lease_live(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(RetainedRecheckSession {
            id,
            item_id: lease.item_id,
            lease_token: lease.token,
            capture_id: before.capture_id(),
            before_epoch: before.epoch(),
            digest: before.digest().to_owned(),
            entries: before.entries(),
            generation: before.generation().to_owned(),
            root_generation: root_generation.to_owned(),
        })
    }

    /// Complete two ordered observations and atomically hand a changed source
    /// back to discovery. The caller must separately verify that its guarded
    /// root and source descriptors still match `root_generation` before calling.
    /// This method never removes, renames, or modifies media files.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish_retained_recheck(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        session: &RetainedRecheckSession,
        after: &SealedCustodyHash,
        root_generation: &str,
        next_check: Duration,
    ) -> Result<RetainedRecheckOutcome, WorkStoreError> {
        let interval = duration_millis(next_check, MAX_RECHECK_MILLIS)?;
        validate_retained_seal_binding(lease, after)?;
        if session.item_id != lease.item_id
            || session.lease_token != lease.token
            || session.capture_id != after.capture_id()
            || session.before_epoch == after.epoch()
            || session.digest != after.digest()
            || session.entries != after.entries()
            || session.generation != after.generation()
            || session.root_generation != root_generation
        {
            return Err(WorkStoreError::InvalidRequest(
                "retained-source observations are unstable",
            ));
        }
        let mut tx = pool.begin().await?;
        // Discovery takes this transaction-scoped lock before touching an
        // occurrence. Matching it makes supersession and insertion indivisible
        // with respect to normal discovery in every process.
        sqlx::query("SELECT lock_inbound_occurrence($1,$2)")
            .bind(lease.source_library_id)
            .bind(&lease.source_path)
            .execute(&mut *tx)
            .await?;
        let completed: Option<CompletionRow> = sqlx::query_as("SELECT source_library_id,source_path,source_path_format,source_generation,content_digest,completed_lease_token,completed_session_id,completed_epoch,completed_digest,completed_generation,completed_outcome,completed_work_item_id,completed_created FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE")
            .bind(lease.item_id).fetch_optional(&mut *tx).await?;
        let Some(completed) = completed else {
            return Err(WorkStoreError::LeaseExpired(lease.item_id));
        };
        if completed.source_library_id != lease.source_library_id
            || completed.source_path != lease.source_path
            || completed.source_path_format != lease.source_path_format
            || completed.source_generation != lease.baseline_generation
            || completed.content_digest != lease.baseline_digest
        {
            return Err(WorkStoreError::LeaseExpired(lease.item_id));
        }
        // A retry after a commit acknowledgement was lost returns the exact
        // prior disposition without creating another occurrence.
        if completed.completed_lease_token == Some(lease.token)
            && completed.completed_session_id == Some(session.id)
            && completed.completed_epoch == Some(after.epoch())
            && completed.completed_digest.as_deref() == Some(after.digest())
            && completed.completed_generation.as_deref() == Some(after.generation())
        {
            let outcome = match (
                completed.completed_outcome.as_deref(),
                completed.completed_work_item_id,
                completed.completed_created,
            ) {
                (Some("unchanged"), None, None) => RetainedRecheckOutcome::Unchanged,
                (Some("superseded"), Some(work_item_id), Some(created)) => {
                    RetainedRecheckOutcome::Superseded {
                        work_item_id,
                        created,
                    }
                }
                _ => {
                    return Err(WorkStoreError::CorruptState(
                        "invalid retained-source completion".to_owned(),
                    ));
                }
            };
            tx.commit().await?;
            return Ok(outcome);
        }
        let original_expiry = lock_retained_source_lease(&mut tx, lease).await?;
        let session_exists: Option<bool> = sqlx::query_scalar("SELECT true FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND session_id=$2 AND lease_token=$3 AND capture_id=$4 AND before_epoch=$5 AND before_digest=$6 AND before_entries=$7 AND source_generation=$8 AND source_root_generation=$9 FOR UPDATE")
            .bind(lease.item_id).bind(session.id).bind(lease.token).bind(session.capture_id).bind(session.before_epoch).bind(&session.digest).bind(session.entries).bind(&session.generation).bind(&session.root_generation).fetch_optional(&mut *tx).await?;
        if session_exists.is_none() {
            return Err(WorkStoreError::InvalidRequest(
                "retained-source recheck is stale",
            ));
        }
        lock_retained_seal(&mut tx, lease, after).await?;

        let prior: Option<(Option<Uuid>,String,i16,bool)> = sqlx::query_as("SELECT source_library_id,source_path,discovery_path_format,inbound_active FROM release_work_items WHERE id=$1 FOR UPDATE")
            .bind(lease.item_id).fetch_optional(&mut *tx).await?;
        if let Some((library, path, format, _)) = &prior
            && (*library != lease.source_library_id
                || *path != lease.source_path
                || *format != lease.source_path_format)
        {
            return Err(WorkStoreError::InvalidRequest(
                "retained-source work item binding changed",
            ));
        }
        let prior_active = prior.is_some_and(|(_, _, _, active)| active);
        let unchanged = prior_active
            && session.generation == lease.baseline_generation
            && session.digest == lease.baseline_digest;
        if !unchanged {
            // Ordinary ingestion is permitted to rename a source. Until a
            // copy-only retained-origin intake exists, retain the obligation
            // and its source and persist only a request for fresh intake.
            // The pending request and its durable reason are one atomic
            // transition: observers may stop, restart, or crash between this
            // commit and the scheduler's retry deferral, and the recorded
            // disposition must not lag the sticky signal.
            sqlx::query("UPDATE inbound_retained_sources SET baseline_change_pending=true,last_retry_reason='awaiting_intake',updated_at=clock_timestamp() WHERE work_item_id=$1")
                .bind(lease.item_id).execute(&mut *tx).await?;
            ensure_original_expiry_live(&mut tx, lease.item_id, original_expiry).await?;
            tx.commit().await?;
            return Err(WorkStoreError::RetainedFreshIntakeRequired);
        }
        let outcome = RetainedRecheckOutcome::Unchanged;
        let disposition = "unchanged";
        let completed_item: Option<Uuid> = None;
        let completed_created: Option<bool> = None;
        sqlx::query("UPDATE inbound_retained_sources SET status=$2,superseded_by=$3,next_reobserve_at=clock_timestamp()+($4::bigint*interval '1 millisecond'),retry_count=0,last_retry_reason=NULL,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,completed_lease_token=$5,completed_session_id=$6,completed_epoch=$7,completed_digest=$8,completed_generation=$9,completed_outcome=$11,completed_work_item_id=$3,completed_created=$10,updated_at=clock_timestamp() WHERE work_item_id=$1")
            .bind(lease.item_id).bind(if unchanged { "active" } else { "superseded" }).bind(completed_item).bind(interval).bind(lease.token).bind(session.id).bind(after.epoch()).bind(after.digest()).bind(after.generation()).bind(completed_created).bind(disposition).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND session_id=$2",
        )
        .bind(lease.item_id)
        .bind(session.id)
        .execute(&mut *tx)
        .await?;
        ensure_original_expiry_live(&mut tx, lease.item_id, original_expiry).await?;
        tx.commit().await?;
        Ok(outcome)
    }
}
