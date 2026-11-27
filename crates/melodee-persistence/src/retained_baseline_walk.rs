//! Durable, bounded cursor over authoritative chunk metadata. A mismatch is
//! only a request for fresh verification; it is never a receipt or deletion.
use crate::{RetainedBaselineChunk, RetainedSourceLease, WorkItemStore, WorkStoreError};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
const PAGE: i64 = 16;
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("retained baseline cursor changed")
}
#[derive(Debug, Clone)]
pub struct RetainedBaselinePage {
    item: Uuid,
    token: Uuid,
    baseline: Uuid,
    revision: i64,
    old_file: i64,
    old_chunk: i64,
    ordinal: i64,
    path: String,
    length: i64,
    chunks: Vec<RetainedBaselineChunk>,
}
impl RetainedBaselinePage {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn file_length(&self) -> i64 {
        self.length
    }
    pub fn chunks(&self) -> &[RetainedBaselineChunk] {
        &self.chunks
    }
}
#[derive(Debug, Clone)]
pub struct RetainedBaselinePass {
    item: Uuid,
    token: Uuid,
    baseline: Uuid,
    revision: i64,
    old_file: i64,
    old_chunk: i64,
}
/// An inconclusive page observation. It records a need for later complete
/// verification; neither reason establishes source deletion or absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedBaselineUnreadableReason {
    Missing,
    Unreadable,
}
impl RetainedBaselineUnreadableReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
    }
}
#[derive(Debug, Clone)]
pub enum RetainedBaselineRead {
    Page(RetainedBaselinePage),
    PassComplete(RetainedBaselinePass),
}
#[derive(FromRow)]
struct Cursor {
    baseline_id: Option<Uuid>,
    baseline_cursor_file: i64,
    baseline_cursor_chunk: i64,
    baseline_cursor_revision: i64,
    baseline_file_due: bool,
}
async fn lock_cursor(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
) -> Result<(Cursor, OffsetDateTime), WorkStoreError> {
    let expiry = crate::retained_sources::lock_retained_source_lease(tx, lease).await?;
    let cursor:Cursor=sqlx::query_as("SELECT baseline_id,baseline_cursor_file,baseline_cursor_chunk,baseline_cursor_revision,baseline_file_due_at<=clock_timestamp() AS baseline_file_due FROM inbound_retained_sources WHERE work_item_id=$1").bind(lease.work_item_id()).fetch_one(&mut **tx).await?;
    let baseline = cursor.baseline_id.ok_or_else(invalid)?;
    let valid:Option<bool>=sqlx::query_scalar("SELECT true FROM inbound_retained_baselines WHERE id=$1 AND work_item_id=$2 AND state='authoritative'").bind(baseline).bind(lease.work_item_id()).fetch_optional(&mut **tx).await?;
    if valid.is_none() {
        return Err(invalid());
    }
    Ok((cursor, expiry))
}
async fn end(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(WorkStoreError::LeaseExpired(lease.work_item_id()));
    }
    Ok(())
}
fn matches(
    cursor: &Cursor,
    lease: &RetainedSourceLease,
    item: Uuid,
    token: Uuid,
    baseline: Uuid,
    revision: i64,
    file: i64,
    chunk: i64,
) -> bool {
    lease.work_item_id() == item
        && lease.token() == token
        && cursor.baseline_id == Some(baseline)
        && cursor.baseline_cursor_revision == revision
        && cursor.baseline_cursor_file == file
        && cursor.baseline_cursor_chunk == chunk
}
impl WorkItemStore {
    /// Return at most one path and sixteen chunks. The caller must reread the
    /// underlying bytes cryptographically before acknowledging the page.
    pub async fn retained_baseline_next_page(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
    ) -> Result<RetainedBaselineRead, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let (cursor, expiry) = lock_cursor(&mut tx, lease).await?;
        let baseline = cursor.baseline_id.ok_or_else(invalid)?;
        if !cursor.baseline_file_due {
            return Err(invalid());
        }
        let file:Option<(i64,String,i64,i64)>=sqlx::query_as("SELECT stage_ordinal,path_key,file_length,chunk_count FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND (stage_ordinal>$2 OR (stage_ordinal=$2 AND chunk_count>0 AND $3<chunk_count-1)) ORDER BY stage_ordinal LIMIT 1").bind(baseline).bind(cursor.baseline_cursor_file).bind(cursor.baseline_cursor_chunk).fetch_optional(&mut *tx).await?;
        let result = if let Some((ordinal, path, length, count)) = file {
            let after = if ordinal == cursor.baseline_cursor_file {
                cursor.baseline_cursor_chunk
            } else {
                -1
            };
            let rows:Vec<(i64,i32,String)>=sqlx::query_as("SELECT chunk_index,chunk_length,digest FROM inbound_retained_baseline_chunks WHERE baseline_id=$1 AND stage_ordinal=$2 AND chunk_index>$3 ORDER BY chunk_index LIMIT $4").bind(baseline).bind(ordinal).bind(after).bind(PAGE).fetch_all(&mut *tx).await?;
            if (count > 0 && rows.is_empty()) || (count == 0 && !rows.is_empty()) {
                return Err(invalid());
            }
            let chunks = rows
                .into_iter()
                .map(|(index, length, digest)| RetainedBaselineChunk {
                    index,
                    length,
                    digest,
                })
                .collect();
            RetainedBaselineRead::Page(RetainedBaselinePage {
                item: lease.work_item_id(),
                token: lease.token(),
                baseline,
                revision: cursor.baseline_cursor_revision,
                old_file: cursor.baseline_cursor_file,
                old_chunk: cursor.baseline_cursor_chunk,
                ordinal,
                path,
                length,
                chunks,
            })
        } else {
            RetainedBaselineRead::PassComplete(RetainedBaselinePass {
                item: lease.work_item_id(),
                token: lease.token(),
                baseline,
                revision: cursor.baseline_cursor_revision,
                old_file: cursor.baseline_cursor_file,
                old_chunk: cursor.baseline_cursor_chunk,
            })
        };
        end(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Advance after a successful reread. `mismatch` is sticky and only
    /// schedules fresh verification; it cannot authorize source removal.
    pub async fn acknowledge_retained_baseline_page(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        page: &RetainedBaselinePage,
        mismatch: bool,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let (cursor, expiry) = lock_cursor(&mut tx, lease).await?;
        if !matches(
            &cursor,
            lease,
            page.item,
            page.token,
            page.baseline,
            page.revision,
            page.old_file,
            page.old_chunk,
        ) {
            return Err(invalid());
        }
        let chunk = page.chunks.last().map_or(-1, |c| c.index);
        sqlx::query("UPDATE inbound_retained_sources SET baseline_cursor_file=$2,baseline_cursor_chunk=$3,baseline_cursor_revision=baseline_cursor_revision+1,baseline_directory_turn=true,baseline_change_pending=baseline_change_pending OR $4,updated_at=clock_timestamp() WHERE work_item_id=$1").bind(lease.work_item_id()).bind(page.ordinal).bind(chunk).bind(mismatch).execute(&mut *tx).await?;
        end(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Record an inconclusive page and yield fairly. The caller must have
    /// revalidated the guarded root and candidate namespace; missing or
    /// unreadable metadata is not proof that source media may be removed.
    pub async fn unreadable_retained_baseline_page(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        page: &RetainedBaselinePage,
        reason: RetainedBaselineUnreadableReason,
        yield_after: Duration,
    ) -> Result<(), WorkStoreError> {
        let millis = yield_after.whole_milliseconds();
        if !(1..=30_i128 * 24 * 60 * 60 * 1000).contains(&millis) {
            return Err(WorkStoreError::InvalidRequest(
                "invalid retained-source yield duration",
            ));
        }
        let millis = i64::try_from(millis).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let (cursor, expiry) = lock_cursor(&mut tx, lease).await?;
        if !matches(
            &cursor,
            lease,
            page.item,
            page.token,
            page.baseline,
            page.revision,
            page.old_file,
            page.old_chunk,
        ) {
            return Err(invalid());
        }
        let proof_active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_recheck_sessions WHERE work_item_id=$1 AND lease_token=$2)")
            .bind(lease.work_item_id()).bind(lease.token()).fetch_one(&mut *tx).await?;
        if proof_active {
            return Err(WorkStoreError::InvalidRequest(
                "cannot yield an active retained proof session",
            ));
        }
        let chunk = page.chunks.last().map_or(-1, |c| c.index);
        sqlx::query("UPDATE inbound_retained_sources SET baseline_cursor_file=$2,baseline_cursor_chunk=$3,baseline_cursor_revision=baseline_cursor_revision+1,baseline_directory_turn=true,baseline_change_pending=true,baseline_unreadable_reason=$4,baseline_unreadable_count=CASE WHEN baseline_unreadable_count<2147483647 THEN baseline_unreadable_count+1 ELSE baseline_unreadable_count END,next_reobserve_at=GREATEST(clock_timestamp()+($5::bigint*interval '1 millisecond'),LEAST(baseline_file_due_at,GREATEST(COALESCE(baseline_directory_blocked_until,'-infinity'::timestamptz),COALESCE((SELECT d.next_scan_at FROM inbound_retained_baseline_directories d WHERE d.baseline_id=inbound_retained_sources.baseline_id ORDER BY d.next_scan_at,d.stage_ordinal LIMIT 1),'infinity'::timestamptz)))),lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp() WHERE work_item_id=$1")
            .bind(lease.work_item_id()).bind(page.ordinal).bind(chunk).bind(reason.as_str()).bind(millis).execute(&mut *tx).await?;
        end(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Wrap only an observed completed pass, retaining any pending mismatch.
    pub async fn complete_retained_baseline_pass(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        pass: &RetainedBaselinePass,
        mismatch: bool,
        recheck_after: Duration,
    ) -> Result<(), WorkStoreError> {
        let millis = recheck_after.whole_milliseconds();
        if !(1..=30_i128 * 24 * 60 * 60 * 1000).contains(&millis) {
            return Err(invalid());
        }
        let millis = i64::try_from(millis).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let (cursor, expiry) = lock_cursor(&mut tx, lease).await?;
        if !matches(
            &cursor,
            lease,
            pass.item,
            pass.token,
            pass.baseline,
            pass.revision,
            pass.old_file,
            pass.old_chunk,
        ) {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_retained_sources SET baseline_cursor_file=0,baseline_cursor_chunk=-1,baseline_cursor_revision=baseline_cursor_revision+1,baseline_completed_passes=baseline_completed_passes+1,baseline_file_due_at=clock_timestamp()+($3::bigint*interval '1 millisecond'),baseline_directory_turn=true,baseline_change_pending=baseline_change_pending OR $2,updated_at=clock_timestamp() WHERE work_item_id=$1").bind(lease.work_item_id()).bind(mismatch).bind(millis).execute(&mut *tx).await?;
        end(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
}
