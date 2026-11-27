//! Bounded directory-change hints for an authoritative retained baseline.
//! Directory cookies can skip/repeat entries and never establish absence.
use crate::{
    RetainedBaselineUnreadableReason, RetainedSourceLease, SealedCustodyHash, WorkItemStore,
    WorkStoreError, WorkerId,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
const MAX_MEMBERS: usize = 64;
const MAX_MEMBER_BYTES: usize = 1_048_576;
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid retained directory observation")
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Debug, Clone)]
pub struct RetainedDirectoryMember {
    pub path_key: String,
    pub is_directory: bool,
}
/// Directory cookies are hints. EOF is only a pacing boundary and does not
/// assert a stable complete snapshot or prove absence.
#[derive(Debug, Clone, Copy)]
pub enum RetainedDirectoryContinuation<'a> {
    More(&'a str),
    Eof,
    Invalidated,
}
/// One bounded directory scan decision and its independent pacing intervals.
#[derive(Debug, Clone, Copy)]
pub struct RetainedDirectoryAdvance<'a> {
    pub continuation: RetainedDirectoryContinuation<'a>,
    pub generation: &'a str,
    pub mismatch: bool,
    pub yield_after: Duration,
    pub recheck_after: Duration,
}
#[derive(Debug, Clone)]
pub struct RetainedBaselineDirectory {
    item: Uuid,
    token: Uuid,
    baseline: Uuid,
    ordinal: i64,
    revision: i64,
    path: String,
    cookie: Option<String>,
    generation: Option<String>,
}
impl RetainedBaselineDirectory {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn cookie(&self) -> Option<&str> {
        self.cookie.as_deref()
    }
    pub fn generation(&self) -> Option<&str> {
        self.generation.as_deref()
    }
}
async fn lock_reader(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
) -> Result<(Uuid, OffsetDateTime), WorkStoreError> {
    let expiry = crate::retained_sources::lock_retained_source_lease(tx, lease).await?;
    let baseline: Option<Uuid> = sqlx::query_scalar(
        "SELECT baseline_id FROM inbound_retained_sources WHERE work_item_id=$1",
    )
    .bind(lease.work_item_id())
    .fetch_one(&mut **tx)
    .await?;
    let baseline = baseline.ok_or_else(invalid)?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baselines WHERE id=$1 AND work_item_id=$2 AND state='authoritative')").bind(baseline).bind(lease.work_item_id()).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(invalid());
    }
    Ok((baseline, expiry))
}
async fn end_reader(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let valid: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !valid {
        return Err(WorkStoreError::LeaseExpired(lease.work_item_id()));
    }
    Ok(())
}
async fn lock_page(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
    page: &RetainedBaselineDirectory,
) -> Result<OffsetDateTime, WorkStoreError> {
    let (baseline, expiry) = lock_reader(tx, lease).await?;
    if page.item != lease.work_item_id() || page.token != lease.token() || page.baseline != baseline
    {
        return Err(invalid());
    }
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND stage_ordinal=$2 AND scan_revision=$3 AND path_key COLLATE \"C\"=$4 COLLATE \"C\" AND scan_cookie IS NOT DISTINCT FROM $5 AND scan_generation IS NOT DISTINCT FROM $6)").bind(baseline).bind(page.ordinal).bind(page.revision).bind(&page.path).bind(&page.cookie).bind(&page.generation).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(invalid());
    }
    Ok(expiry)
}
impl WorkItemStore {
    /// Record a same-stream verified non-root directory from the frozen stage.
    /// Root is stored when the baseline begins, bound to the stage seal.
    pub async fn record_retained_baseline_directory(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        path: &str,
        digest: &str,
    ) -> Result<(), WorkStoreError> {
        if path.is_empty() || path.len() > 262_200 || path.contains('\0') || !valid_digest(digest) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = crate::retained_baseline::locks(&mut tx, worker, seal).await?;
        crate::retained_baseline::lock_baseline(&mut tx, id, worker, seal, "capturing").await?;
        let rows:Vec<(i64,String,String)>=sqlx::query_as("SELECT ordinal,parent_key,digest FROM custody_hash_spool_entries WHERE capture_id=$1 AND ingest_occurrence_hash(path_key)=ingest_occurrence_hash($2) AND path_key COLLATE \"C\"=$2 COLLATE \"C\" AND is_directory ORDER BY ordinal LIMIT 2").bind(seal.capture_id()).bind(path).fetch_all(&mut *tx).await?;
        if rows.len() != 1 || rows[0].2 != digest {
            return Err(invalid());
        }
        let (ordinal, parent, stage_digest) = &rows[0];
        let prior:Option<(String,String,String)>=sqlx::query_as("SELECT path_key,parent_key,stage_digest FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND stage_ordinal=$2 FOR UPDATE").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?;
        if let Some((p, parent_check, d)) = prior {
            if p != path || parent_check != *parent || d != *stage_digest {
                return Err(invalid());
            }
        } else {
            let cost = i64::try_from(path.len() + parent.len() + 512).map_err(|_| invalid())?;
            crate::retained_baseline::reserve(&mut tx, 0, 1, cost).await?;
            sqlx::query("INSERT INTO inbound_retained_baseline_directories(baseline_id,stage_ordinal,path_key,parent_key,stage_digest,logical_bytes) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(ordinal).bind(path).bind(parent).bind(digest).bind(cost).execute(&mut *tx).await?;
            sqlx::query(
                "UPDATE inbound_retained_baselines SET updated_at=clock_timestamp() WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        crate::retained_baseline::end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Select one directory fairly; cookies are hints only, including on EOF.
    pub async fn next_retained_baseline_directory(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
    ) -> Result<RetainedBaselineDirectory, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let (baseline, expiry) = lock_reader(&mut tx, lease).await?;
        let due:bool=sqlx::query_scalar("SELECT COALESCE(baseline_directory_blocked_until<=clock_timestamp(),true) FROM inbound_retained_sources WHERE work_item_id=$1").bind(lease.work_item_id()).fetch_one(&mut *tx).await?;
        if !due {
            return Err(invalid());
        }
        #[allow(clippy::type_complexity)]
        let row:Option<(i64,i64,String,Option<String>,Option<String>)>=sqlx::query_as("SELECT stage_ordinal,scan_revision,path_key,scan_cookie,scan_generation FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND next_scan_at<=statement_timestamp() ORDER BY next_scan_at,stage_ordinal LIMIT 1").bind(baseline).fetch_optional(&mut *tx).await?;
        let (ordinal, revision, path, cookie, generation) = row.ok_or_else(invalid)?;
        end_reader(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(RetainedBaselineDirectory {
            item: lease.work_item_id(),
            token: lease.token(),
            baseline,
            ordinal,
            revision,
            path,
            cookie,
            generation,
        })
    }
    /// Check a bounded page of actual observed children against exact parent,
    /// child path and type. `false` requests verification; it is not absence.
    pub async fn retained_directory_membership(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        page: &RetainedBaselineDirectory,
        members: &[RetainedDirectoryMember],
    ) -> Result<Vec<bool>, WorkStoreError> {
        if members.len() > MAX_MEMBERS
            || members.iter().map(|m| m.path_key.len()).sum::<usize>() > MAX_MEMBER_BYTES
            || members.iter().any(|m| {
                m.path_key.is_empty() || m.path_key.len() > 262_200 || m.path_key.contains('\0')
            })
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_page(&mut tx, lease, page).await?;
        let paths: Vec<&str> = members.iter().map(|m| m.path_key.as_str()).collect();
        let dirs: Vec<bool> = members.iter().map(|m| m.is_directory).collect();
        let matched:Vec<bool>=sqlx::query_scalar("SELECT CASE WHEN x.is_dir THEN EXISTS(SELECT 1 FROM inbound_retained_baseline_directories d WHERE d.baseline_id=$1 AND ingest_occurrence_hash(d.path_key)=ingest_occurrence_hash(x.path) AND d.path_key COLLATE \"C\"=x.path COLLATE \"C\" AND d.parent_key COLLATE \"C\"=$2 COLLATE \"C\") ELSE EXISTS(SELECT 1 FROM inbound_retained_baseline_files f WHERE f.baseline_id=$1 AND ingest_occurrence_hash(f.path_key)=ingest_occurrence_hash(x.path) AND f.path_key COLLATE \"C\"=x.path COLLATE \"C\" AND f.parent_key COLLATE \"C\"=$2 COLLATE \"C\") END FROM unnest($3::text[],$4::boolean[]) WITH ORDINALITY AS x(path,is_dir,ord) ORDER BY x.ord").bind(page.baseline).bind(&page.path).bind(&paths).bind(&dirs).fetch_all(&mut *tx).await?;
        end_reader(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(matched)
    }
    /// Rotate one page and yield. A changed directory generation invalidates
    /// the cookie, requests verification and restarts this directory later.
    pub async fn advance_retained_baseline_directory(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        page: &RetainedBaselineDirectory,
        options: RetainedDirectoryAdvance<'_>,
    ) -> Result<(), WorkStoreError> {
        let RetainedDirectoryAdvance {
            continuation,
            generation,
            mismatch,
            yield_after,
            recheck_after,
        } = options;
        let millis = yield_after.whole_milliseconds();
        let recheck_millis = recheck_after.whole_milliseconds();
        let next_cookie = match continuation {
            RetainedDirectoryContinuation::More(cookie) => Some(cookie),
            RetainedDirectoryContinuation::Eof | RetainedDirectoryContinuation::Invalidated => None,
        };
        if !(1..=30_i128 * 24 * 60 * 60 * 1000).contains(&millis)
            || !(1..=30_i128 * 24 * 60 * 60 * 1000).contains(&recheck_millis)
            || generation.is_empty()
            || generation.len() > 512
            || generation.contains('\0')
            || next_cookie.is_some_and(|c| {
                c.len() != 16
                    || !c
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(invalid());
        }
        let millis = i64::try_from(millis).map_err(|_| invalid())?;
        let recheck_millis = i64::try_from(recheck_millis).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let expiry = lock_page(&mut tx, lease, page).await?;
        let proof:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)").bind(lease.work_item_id()).bind(lease.token()).fetch_one(&mut *tx).await?;
        if proof {
            return Err(invalid());
        }
        let changed = page.generation.as_deref().is_some_and(|g| g != generation);
        let invalidated =
            changed || matches!(continuation, RetainedDirectoryContinuation::Invalidated);
        let eof = !invalidated && matches!(continuation, RetainedDirectoryContinuation::Eof);
        let cookie = if invalidated { None } else { next_cookie };
        sqlx::query("UPDATE inbound_retained_baseline_directories SET scan_cookie=$3,scan_generation=$4,scan_revision=scan_revision+1,next_scan_at=CASE WHEN $5 THEN clock_timestamp()+($6::bigint*interval '1 millisecond') ELSE clock_timestamp() END WHERE baseline_id=$1 AND stage_ordinal=$2").bind(page.baseline).bind(page.ordinal).bind(cookie).bind(generation).bind(eof).bind(recheck_millis).execute(&mut *tx).await?;
        sqlx::query("UPDATE inbound_retained_sources r SET baseline_directory_turn=false,baseline_directory_unavailable_reason=NULL,baseline_directory_blocked_until=NULL,baseline_change_pending=baseline_change_pending OR $2,next_reobserve_at=GREATEST(clock_timestamp()+($3::bigint*interval '1 millisecond'),LEAST(r.baseline_file_due_at,GREATEST(COALESCE(r.baseline_directory_blocked_until,'-infinity'::timestamptz),COALESCE((SELECT d.next_scan_at FROM inbound_retained_baseline_directories d WHERE d.baseline_id=r.baseline_id ORDER BY d.next_scan_at,d.stage_ordinal LIMIT 1),'infinity'::timestamptz)))),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE r.work_item_id=$1").bind(lease.work_item_id()).bind(mismatch||invalidated).bind(millis).execute(&mut *tx).await?;
        end_reader(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Yield an inconclusive directory observation. No filesystem absence is
    /// established; the sticky signal requests separate fresh verification.
    pub async fn unreadable_retained_baseline_directory(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        page: &RetainedBaselineDirectory,
        reason: RetainedBaselineUnreadableReason,
        yield_after: Duration,
    ) -> Result<(), WorkStoreError> {
        let millis = yield_after.whole_milliseconds();
        if !(1..=30_i128 * 24 * 60 * 60 * 1000).contains(&millis) {
            return Err(invalid());
        }
        let millis = i64::try_from(millis).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let expiry = lock_page(&mut tx, lease, page).await?;
        let proof:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)").bind(lease.work_item_id()).bind(lease.token()).fetch_one(&mut *tx).await?;
        if proof {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_retained_baseline_directories SET scan_cookie=NULL,scan_generation=NULL,scan_revision=scan_revision+1,next_scan_at=clock_timestamp() WHERE baseline_id=$1 AND stage_ordinal=$2").bind(page.baseline).bind(page.ordinal).execute(&mut *tx).await?;
        sqlx::query("UPDATE inbound_retained_sources SET baseline_directory_turn=false,baseline_change_pending=true,baseline_unreadable_reason=$2,baseline_unreadable_count=CASE WHEN baseline_unreadable_count<2147483647 THEN baseline_unreadable_count+1 ELSE baseline_unreadable_count END,next_reobserve_at=GREATEST(clock_timestamp()+($3::bigint*interval '1 millisecond'),LEAST(baseline_file_due_at,GREATEST(COALESCE(baseline_directory_blocked_until,'-infinity'::timestamptz),COALESCE((SELECT d.next_scan_at FROM inbound_retained_baseline_directories d WHERE d.baseline_id=inbound_retained_sources.baseline_id ORDER BY d.next_scan_at,d.stage_ordinal LIMIT 1),'infinity'::timestamptz)))),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE work_item_id=$1").bind(lease.work_item_id()).bind(reason.as_str()).bind(millis).execute(&mut *tx).await?;
        end_reader(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Defer the unsupported directory class without delaying due file pages.
    /// This is an operator-visible unavailable state, not a successful scan.
    pub async fn defer_unsupported_retained_directories(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        retry_after: Duration,
        minimum_delay: Duration,
    ) -> Result<(), WorkStoreError> {
        let retry = retry_after.whole_milliseconds();
        let minimum = minimum_delay.whole_milliseconds();
        let max = 30_i128 * 24 * 60 * 60 * 1000;
        if !(1..=max).contains(&retry) || !(1..=max).contains(&minimum) {
            return Err(invalid());
        }
        let retry = i64::try_from(retry).map_err(|_| invalid())?;
        let minimum = i64::try_from(minimum).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let (_baseline, expiry) = lock_reader(&mut tx, lease).await?;
        let proof:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)").bind(lease.work_item_id()).bind(lease.token()).fetch_one(&mut *tx).await?;
        if proof {
            return Err(invalid());
        }
        sqlx::query("WITH instant AS (SELECT clock_timestamp() AS now) UPDATE inbound_retained_sources r SET baseline_directory_turn=false,baseline_directory_unavailable_reason='unsupported',baseline_directory_blocked_until=instant.now+($2::bigint*interval '1 millisecond'),next_reobserve_at=GREATEST(instant.now+($3::bigint*interval '1 millisecond'),LEAST(r.baseline_file_due_at,GREATEST(instant.now+($2::bigint*interval '1 millisecond'),COALESCE((SELECT d.next_scan_at FROM inbound_retained_baseline_directories d WHERE d.baseline_id=r.baseline_id ORDER BY d.next_scan_at,d.stage_ordinal LIMIT 1),'infinity'::timestamptz)))),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=instant.now FROM instant WHERE r.work_item_id=$1").bind(lease.work_item_id()).bind(retry).bind(minimum).execute(&mut *tx).await?;
        end_reader(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
}
