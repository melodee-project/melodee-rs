//! Provisional, fenced v4 chunk accumulators. These are change-detection hints,
//! never proof of a current snapshot or authority to remove source media.
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{RetainedSourceLease, WorkItemStore, WorkStoreError};

const CHUNK_BYTES: u64 = 65_536;
const MAX_PEAKS_BYTES: usize = 8192;
const RESERVED_BYTES: i64 = 272_000;

/// An opaque, provisional accumulator. Earlier chunks may have changed since
/// observation; resumption does not establish current source integrity.
#[derive(Clone, Debug)]
pub struct RetainedChunkCheckpoint {
    capture_id: Uuid,
    epoch: Uuid,
    path: String,
    root_generation: String,
    candidate_generation: String,
    file_length: u64,
    next_index: u64,
    revision: i64,
    peaks: Vec<(u8, String)>,
    complete: bool,
}
impl RetainedChunkCheckpoint {
    pub fn capture_id(&self) -> Uuid {
        self.capture_id
    }
    pub fn epoch(&self) -> Uuid {
        self.epoch
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn root_generation(&self) -> &str {
        &self.root_generation
    }
    pub fn candidate_generation(&self) -> &str {
        &self.candidate_generation
    }
    pub fn file_length(&self) -> u64 {
        self.file_length
    }
    pub fn next_index(&self) -> u64 {
        self.next_index
    }
    pub fn revision(&self) -> i64 {
        self.revision
    }
    pub fn peaks(&self) -> &[(u8, String)] {
        &self.peaks
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
}

#[derive(FromRow)]
struct Row {
    capture_id: Uuid,
    epoch: Uuid,
    lease_owner: String,
    lease_token: Uuid,
    format_version: i16,
    algorithm: String,
    chunk_bytes: i32,
    path: String,
    root_generation: String,
    candidate_generation: String,
    file_length: i64,
    next_index: i64,
    revision: i64,
    peaks: String,
    complete: bool,
}
const SELECT_ROW: &str = "SELECT capture_id,epoch,lease_owner,lease_token,format_version,algorithm,chunk_bytes,path,root_generation,candidate_generation,file_length,next_index,revision,peaks::text AS peaks,complete FROM inbound_retained_chunk_checkpoints WHERE work_item_id=$1 FOR UPDATE";
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid retained chunk checkpoint")
}
fn decode_hash(value: &str) -> Result<[u8; 32], WorkStoreError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let mut out = [0; 32];
    for (slot, pair) in out.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let s = std::str::from_utf8(pair).map_err(|_| invalid())?;
        *slot = u8::from_str_radix(s, 16).map_err(|_| invalid())?;
    }
    Ok(out)
}
fn checkpoint(row: Row) -> Result<RetainedChunkCheckpoint, WorkStoreError> {
    if row.format_version != 4
        || row.algorithm != "blake3-binary-carry-v4"
        || row.chunk_bytes != 65_536
    {
        return Err(invalid());
    }
    if row.peaks.len() > MAX_PEAKS_BYTES
        || row.file_length < 0
        || row.next_index < 0
        || row.revision < 0
    {
        return Err(invalid());
    }
    let file_length = u64::try_from(row.file_length).map_err(|_| invalid())?;
    let next_index = u64::try_from(row.next_index).map_err(|_| invalid())?;
    let peaks: Vec<(u8, String)> = serde_json::from_str(&row.peaks).map_err(|_| invalid())?;
    if peaks.len() > 64 || next_index > file_length.div_ceil(CHUNK_BYTES) {
        return Err(invalid());
    }
    let mut bits = 0_u64;
    for (level, hex) in &peaks {
        if *level >= 64 || bits & (1_u64 << level) != 0 {
            return Err(invalid());
        }
        decode_hash(hex)?;
        bits |= 1_u64 << level;
    }
    if bits != next_index || (row.complete && next_index != file_length.div_ceil(CHUNK_BYTES)) {
        return Err(invalid());
    }
    Ok(RetainedChunkCheckpoint {
        capture_id: row.capture_id,
        epoch: row.epoch,
        path: row.path,
        root_generation: row.root_generation,
        candidate_generation: row.candidate_generation,
        file_length,
        next_index,
        revision: row.revision,
        peaks,
        complete: row.complete,
    })
}
async fn fence<'a>(
    tx: &mut Transaction<'a, Postgres>,
    lease: &RetainedSourceLease,
) -> Result<OffsetDateTime, WorkStoreError> {
    sqlx::query("SELECT singleton FROM inbound_retained_chunk_budget WHERE singleton FOR UPDATE")
        .execute(&mut **tx)
        .await?;
    crate::retained_sources::lock_retained_source_lease(tx, lease).await
}
async fn end_fence(
    tx: &mut Transaction<'_, Postgres>,
    lease: &RetainedSourceLease,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz > clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(WorkStoreError::LeaseExpired(lease.work_item_id()));
    }
    Ok(())
}
fn bound_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
impl WorkItemStore {
    /// Start or adopt an interrupted provisional capture under a new retained
    /// lease. Matching bindings never make older observations current.
    pub async fn begin_or_adopt_retained_chunk_checkpoint(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        path: &str,
        root_generation: &str,
        candidate_generation: &str,
        file_length: u64,
    ) -> Result<RetainedChunkCheckpoint, WorkStoreError> {
        if !bound_text(path, 262_200)
            || !bound_text(root_generation, 512)
            || !bound_text(candidate_generation, 512)
        {
            return Err(invalid());
        }
        let length = i64::try_from(file_length).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        let expiry = fence(&mut tx, lease).await?;
        let row: Option<Row> = sqlx::query_as(SELECT_ROW)
            .bind(lease.work_item_id())
            .fetch_optional(&mut *tx)
            .await?;
        let row = if let Some(row) = row {
            if row.path != path
                || row.root_generation != root_generation
                || row.candidate_generation != candidate_generation
                || row.file_length != length
            {
                return Err(invalid());
            }
            sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET lease_owner=$2,lease_token=$3,updated_at=clock_timestamp() WHERE work_item_id=$1").bind(lease.work_item_id()).bind(lease.owner()).bind(lease.token()).execute(&mut *tx).await?;
            row
        } else {
            let inserted=sqlx::query("UPDATE inbound_retained_chunk_budget SET captures=captures+1,bytes=bytes+$1 WHERE singleton AND captures<max_captures AND $1<=max_bytes-bytes").bind(RESERVED_BYTES).execute(&mut *tx).await?;
            if inserted.rows_affected() != 1 {
                return Err(WorkStoreError::RetainedSourceBudgetExhausted);
            }
            sqlx::query("INSERT INTO inbound_retained_chunk_checkpoints(work_item_id,capture_id,epoch,lease_owner,lease_token,path,root_generation,candidate_generation,file_length) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(lease.work_item_id()).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(lease.owner()).bind(lease.token()).bind(path).bind(root_generation).bind(candidate_generation).bind(length).execute(&mut *tx).await?;
            sqlx::query_as(SELECT_ROW)
                .bind(lease.work_item_id())
                .fetch_one(&mut *tx)
                .await?
        };
        let result = checkpoint(row)?;
        end_fence(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Advance exactly one indexed chunk from its already domain-separated digest.
    pub async fn advance_retained_chunk_checkpoint(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        previous: &RetainedChunkCheckpoint,
        index: u64,
        chunk_length: u64,
        leaf_digest: &str,
    ) -> Result<RetainedChunkCheckpoint, WorkStoreError> {
        let mut digest = decode_hash(leaf_digest)?;
        let offset = index.checked_mul(CHUNK_BYTES).ok_or_else(invalid)?;
        let expected = previous
            .file_length
            .checked_sub(offset)
            .ok_or_else(invalid)?
            .min(CHUNK_BYTES);
        if previous.complete
            || index != previous.next_index
            || expected == 0
            || chunk_length != expected
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = fence(&mut tx, lease).await?;
        let row: Row = sqlx::query_as(SELECT_ROW)
            .bind(lease.work_item_id())
            .fetch_one(&mut *tx)
            .await?;
        if row.lease_owner != lease.owner() || row.lease_token != lease.token() {
            return Err(WorkStoreError::LeaseExpired(lease.work_item_id()));
        }
        let current = checkpoint(row)?;
        if current.capture_id != previous.capture_id
            || current.revision != previous.revision
            || current.next_index != index
        {
            return Err(invalid());
        }
        let mut peaks = current.peaks;
        let mut level = 0_u8;
        while let Some(pos) = peaks.iter().position(|(l, _)| *l == level) {
            let (_, left) = peaks.remove(pos);
            let left = decode_hash(&left)?;
            let mut h = blake3::Hasher::new();
            h.update(b"melodee-retained-file-node-v4\0");
            h.update(&u64::from(level).to_le_bytes());
            h.update(&left);
            h.update(&digest);
            digest = *h.finalize().as_bytes();
            level = level.checked_add(1).ok_or_else(invalid)?;
        }
        if level >= 64 {
            return Err(invalid());
        }
        peaks.push((level, blake3::Hash::from_bytes(digest).to_hex().to_string()));
        peaks.sort_unstable_by_key(|p| p.0);
        let encoded = serde_json::to_string(&peaks).map_err(|_| invalid())?;
        if encoded.len() > MAX_PEAKS_BYTES {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET next_index=next_index+1,revision=revision+1,peaks=$2::jsonb,updated_at=clock_timestamp() WHERE work_item_id=$1 AND lease_owner=$3 AND lease_token=$4").bind(lease.work_item_id()).bind(encoded).bind(lease.owner()).bind(lease.token()).execute(&mut *tx).await?;
        let row: Row = sqlx::query_as(SELECT_ROW)
            .bind(lease.work_item_id())
            .fetch_one(&mut *tx)
            .await?;
        let result = checkpoint(row)?;
        end_fence(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Mark a complete hint; this does not establish that the file is unchanged.
    pub async fn finish_retained_chunk_checkpoint(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        previous: &RetainedChunkCheckpoint,
    ) -> Result<RetainedChunkCheckpoint, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fence(&mut tx, lease).await?;
        let row: Row = sqlx::query_as(SELECT_ROW)
            .bind(lease.work_item_id())
            .fetch_one(&mut *tx)
            .await?;
        if row.lease_owner != lease.owner() || row.lease_token != lease.token() {
            return Err(WorkStoreError::LeaseExpired(lease.work_item_id()));
        }
        let current = checkpoint(row)?;
        if current.capture_id != previous.capture_id
            || current.revision != previous.revision
            || current.next_index != current.file_length.div_ceil(CHUNK_BYTES)
        {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_retained_chunk_checkpoints SET complete=true,revision=revision+1,updated_at=clock_timestamp() WHERE work_item_id=$1 AND lease_owner=$2 AND lease_token=$3").bind(lease.work_item_id()).bind(lease.owner()).bind(lease.token()).execute(&mut *tx).await?;
        let row: Row = sqlx::query_as(SELECT_ROW)
            .bind(lease.work_item_id())
            .fetch_one(&mut *tx)
            .await?;
        let result = checkpoint(row)?;
        end_fence(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Drop only provisional metadata; media and retained obligations remain.
    pub async fn discard_retained_chunk_checkpoint(
        &self,
        pool: &PgPool,
        lease: &RetainedSourceLease,
        capture: Uuid,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = fence(&mut tx, lease).await?;
        let deleted=sqlx::query("DELETE FROM inbound_retained_chunk_checkpoints WHERE work_item_id=$1 AND capture_id=$2 AND lease_owner=$3 AND lease_token=$4").bind(lease.work_item_id()).bind(capture).bind(lease.owner()).bind(lease.token()).execute(&mut *tx).await?;
        if deleted.rows_affected() != 1 {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_retained_chunk_budget SET captures=captures-1,bytes=bytes-$1 WHERE singleton").bind(RESERVED_BYTES).execute(&mut *tx).await?;
        end_fence(&mut tx, lease, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Reclaim one abandoned hint after at least one hour; never reclaims a
    /// live lease, media, a retained obligation, or a verified receipt.
    pub async fn cleanup_retained_chunk_checkpoint(
        &self,
        pool: &PgPool,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM inbound_retained_chunk_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let candidate: Option<Uuid> = sqlx::query_scalar("SELECT c.work_item_id FROM inbound_retained_chunk_checkpoints c JOIN inbound_retained_sources r ON r.work_item_id=c.work_item_id WHERE c.updated_at < clock_timestamp()-interval '1 hour' AND (r.status<>'active' OR r.lease_token IS NULL OR r.lease_expires_at<=clock_timestamp()) ORDER BY c.updated_at,c.work_item_id LIMIT 1").fetch_optional(&mut *tx).await?;
        let Some(item) = candidate else {
            tx.commit().await?;
            return Ok(false);
        };
        sqlx::query(
            "SELECT work_item_id FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(item)
        .execute(&mut *tx)
        .await?;
        let deleted = sqlx::query("DELETE FROM inbound_retained_chunk_checkpoints c USING inbound_retained_sources r WHERE c.work_item_id=$1 AND r.work_item_id=c.work_item_id AND c.updated_at<clock_timestamp()-interval '1 hour' AND (r.status<>'active' OR r.lease_token IS NULL OR r.lease_expires_at<=clock_timestamp())").bind(item).execute(&mut *tx).await?;
        if deleted.rows_affected() == 1 {
            sqlx::query("UPDATE inbound_retained_chunk_budget SET captures=captures-1,bytes=bytes-$1 WHERE singleton").bind(RESERVED_BYTES).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(deleted.rows_affected() == 1)
    }
}
