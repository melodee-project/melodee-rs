//! Bounded provisional chunk evidence for a protected staging observation.
//! These rows do not authorize a receipt or prove that a source is unchanged.
use crate::{CustodyHashPurpose, SealedCustodyHash, WorkItemStore, WorkStoreError, WorkerId};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const PAGE: usize = 128;
const CHUNK_BYTES: i64 = 65_536;
const CHUNK_COST: i64 = 128;
#[derive(Debug, Clone)]
pub struct RetainedBaselineChunk {
    pub index: i64,
    pub length: i32,
    pub digest: String,
}
#[derive(Debug, Clone)]
pub struct RetainedBaselineFile {
    pub ordinal: i64,
    pub length: i64,
}
/// Kernel SHA-256 fs-verity measurement and exact Linux file identity.
/// This is provisional metadata, not evidence that a namespace is protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedFileVerity {
    pub device: u64,
    pub inode: u64,
    pub digest: [u8; 32],
}
/// A complete provisional set of per-file fs-verity records. This token is not
/// a receipt and requires independent protected-namespace and source checks.
#[derive(Debug, Clone)]
pub struct RetainedProtectedBaselineFrozen {
    inner: RetainedBaselineFrozen,
}
impl RetainedProtectedBaselineFrozen {
    pub fn baseline(&self) -> &RetainedBaselineFrozen {
        &self.inner
    }
}
/// Frozen provisional metadata. A separate protected-stage verifier and
/// receipt transaction must authorize publication.
#[derive(Debug, Clone)]
pub struct RetainedBaselineFrozen {
    id: Uuid,
    stage_capture: Uuid,
    stage_epoch: Uuid,
    stage_digest: String,
    file_count: i64,
    chunk_count: i64,
}
impl RetainedBaselineFrozen {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn stage_capture(&self) -> Uuid {
        self.stage_capture
    }
    pub fn stage_epoch(&self) -> Uuid {
        self.stage_epoch
    }
    pub fn stage_digest(&self) -> &str {
        &self.stage_digest
    }
    pub fn file_count(&self) -> i64 {
        self.file_count
    }
    pub fn chunk_count(&self) -> i64 {
        self.chunk_count
    }
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid provisional retained baseline")
}
fn valid_digest(d: &str) -> bool {
    d.len() == 64
        && d.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) async fn locks(
    tx: &mut Transaction<'_, Postgres>,
    worker: &WorkerId,
    seal: &SealedCustodyHash,
) -> Result<OffsetDateTime, WorkStoreError> {
    if seal.purpose() != CustodyHashPurpose::Staging || seal.owner() != worker.as_str() {
        return Err(invalid());
    }
    sqlx::query(
        "SELECT singleton FROM inbound_retained_baseline_budget WHERE singleton FOR UPDATE",
    )
    .execute(&mut **tx)
    .await?;
    let expiry:Option<OffsetDateTime>=sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1 AND lease_owner=$2 AND lease_expires_at>clock_timestamp() AND ($3::uuid IS NULL OR protected_attempt_lease_epoch=$3) FOR UPDATE").bind(seal.item_id()).bind(worker.as_str()).bind(seal.protected_claim_epoch()).fetch_optional(&mut **tx).await?.flatten();
    let expiry = expiry.ok_or(WorkStoreError::LeaseExpired(seal.item_id()))?;
    let valid:Option<bool>=sqlx::query_scalar("SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND purpose='staging' AND source_generation=$4 AND target_binding COLLATE \"C\"=$5 COLLATE \"C\" AND state='frozen' AND observation_epoch=$6 AND sealed_epoch=$6 AND sealed_digest=$7 AND entry_count=$8 AND protected_claim_epoch IS NOT DISTINCT FROM $9 FOR UPDATE").bind(seal.capture_id()).bind(seal.item_id()).bind(worker.as_str()).bind(seal.generation()).bind(seal.target_binding()).bind(seal.epoch()).bind(seal.digest()).bind(seal.entries()).bind(seal.protected_claim_epoch()).fetch_optional(&mut **tx).await?;
    if valid.is_none() {
        return Err(invalid());
    }
    Ok(expiry)
}
pub(crate) async fn end(
    tx: &mut Transaction<'_, Postgres>,
    seal: &SealedCustodyHash,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let ok: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !ok {
        return Err(WorkStoreError::LeaseExpired(seal.item_id()));
    }
    Ok(())
}
pub(crate) async fn lock_baseline(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    worker: &WorkerId,
    seal: &SealedCustodyHash,
    state: &str,
) -> Result<(), WorkStoreError> {
    let ok:Option<bool>=sqlx::query_scalar("SELECT true FROM inbound_retained_baselines WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND stage_capture_id=$4 AND stage_epoch=$5 AND stage_digest=$6 AND stage_generation=$7 AND stage_binding COLLATE \"C\"=$8 COLLATE \"C\" AND state=$9 AND protected_claim_epoch IS NOT DISTINCT FROM $10 FOR UPDATE").bind(id).bind(seal.item_id()).bind(worker.as_str()).bind(seal.capture_id()).bind(seal.epoch()).bind(seal.digest()).bind(seal.generation()).bind(seal.target_binding()).bind(state).bind(seal.protected_claim_epoch()).fetch_optional(&mut **tx).await?;
    if ok.is_none() {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    captures: i64,
    rows: i64,
    bytes: i64,
) -> Result<(), WorkStoreError> {
    let updated=sqlx::query("UPDATE inbound_retained_baseline_budget SET captures=captures+$1,rows=rows+$2,bytes=bytes+$3 WHERE singleton AND $1<=max_captures-captures AND $2<=max_rows-rows AND $3<=max_bytes-bytes").bind(captures).bind(rows).bind(bytes).execute(&mut **tx).await?;
    if updated.rows_affected() != 1 {
        return Err(WorkStoreError::RetainedSourceBudgetExhausted);
    }
    Ok(())
}
impl WorkItemStore {
    pub async fn begin_retained_baseline(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        seal: &SealedCustodyHash,
    ) -> Result<Uuid, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        let prior: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM inbound_retained_baselines WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(seal.item_id())
        .fetch_optional(&mut *tx)
        .await?;
        let id = if let Some(id) = prior {
            let state: String =
                sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE id=$1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            if state != "capturing" && state != "frozen" {
                return Err(invalid());
            }
            lock_baseline(&mut tx, id, worker, seal, &state).await?;
            id
        } else {
            let id = Uuid::now_v7();
            let cost = i64::try_from(seal.target_binding().len() + seal.generation().len() + 512)
                .map_err(|_| invalid())?;
            reserve(&mut tx, 1, 1, cost + 512).await?;
            sqlx::query("INSERT INTO inbound_retained_baselines(id,work_item_id,lease_owner,stage_capture_id,stage_epoch,stage_digest,stage_generation,stage_binding,state,logical_bytes,protected_claim_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'capturing',$9,$10)").bind(id).bind(seal.item_id()).bind(worker.as_str()).bind(seal.capture_id()).bind(seal.epoch()).bind(seal.digest()).bind(seal.generation()).bind(seal.target_binding()).bind(cost).bind(seal.protected_claim_epoch()).execute(&mut *tx).await?;
            let root_digest = seal.digest().strip_prefix("v3:").ok_or_else(invalid)?;
            sqlx::query("INSERT INTO inbound_retained_baseline_directories(baseline_id,stage_ordinal,path_key,parent_key,stage_digest,logical_bytes) VALUES($1,0,'','',$2,512)").bind(id).bind(root_digest).execute(&mut *tx).await?;
            id
        };
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn begin_retained_baseline_file(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        path: &str,
    ) -> Result<RetainedBaselineFile, WorkStoreError> {
        if path.is_empty() || path.len() > 262_200 || path.contains('\0') {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        lock_baseline(&mut tx, id, worker, seal, "capturing").await?;
        let rows:Vec<(i64,i64,String,String)>=sqlx::query_as("SELECT ordinal,legacy_length,digest,parent_key FROM custody_hash_spool_entries WHERE capture_id=$1 AND ingest_occurrence_hash(path_key)=ingest_occurrence_hash($2) AND path_key COLLATE \"C\"=$2 COLLATE \"C\" AND NOT is_directory ORDER BY ordinal LIMIT 2").bind(seal.capture_id()).bind(path).fetch_all(&mut *tx).await?;
        if rows.len() != 1 {
            return Err(invalid());
        }
        let (ordinal, length, digest, parent) = &rows[0];
        let prior:Option<(String,i64,String,String)>=sqlx::query_as("SELECT path_key,file_length,stage_digest,parent_key FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND stage_ordinal=$2 FOR UPDATE").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?;
        if let Some((p, l, d, parent_check)) = prior {
            if p != path || l != *length || d != *digest || parent_check != *parent {
                return Err(invalid());
            }
        } else {
            let cost = i64::try_from(path.len() + parent.len() + 256).map_err(|_| invalid())?;
            reserve(&mut tx, 0, 1, cost).await?;
            sqlx::query("INSERT INTO inbound_retained_baseline_files(baseline_id,stage_ordinal,path_key,parent_key,file_length,stage_digest,logical_bytes) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(ordinal).bind(path).bind(parent).bind(length).bind(digest).bind(cost).execute(&mut *tx).await?;
            sqlx::query("UPDATE inbound_retained_baselines SET file_count=file_count+1,updated_at=clock_timestamp() WHERE id=$1").bind(id).execute(&mut *tx).await?;
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(RetainedBaselineFile {
            ordinal: *ordinal,
            length: *length,
        })
    }
    pub async fn append_retained_baseline_chunks(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        ordinal: i64,
        chunks: &[RetainedBaselineChunk],
    ) -> Result<(), WorkStoreError> {
        if chunks.is_empty() || chunks.len() > PAGE {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        lock_baseline(&mut tx, id, worker, seal, "capturing").await?;
        let file:Option<(i64,i64,bool)>=sqlx::query_as("SELECT file_length,chunk_count,finished FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND stage_ordinal=$2 FOR UPDATE").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?;
        let Some((length, mut count, finished)) = file else {
            return Err(invalid());
        };
        let original_count = count;
        let mut indices = Vec::with_capacity(chunks.len());
        let mut lengths = Vec::with_capacity(chunks.len());
        let mut digests = Vec::with_capacity(chunks.len());
        for c in chunks {
            if c.index < 0 || !valid_digest(&c.digest) {
                return Err(invalid());
            }
            let offset = c.index.checked_mul(CHUNK_BYTES).ok_or_else(invalid)?;
            let remaining = length.checked_sub(offset).ok_or_else(invalid)?;
            if remaining <= 0 || i64::from(c.length) != remaining.min(CHUNK_BYTES) {
                return Err(invalid());
            }
            if indices.last().is_some_and(|previous| *previous >= c.index) {
                return Err(invalid());
            }
            if c.index >= original_count {
                if finished || c.index != count {
                    return Err(invalid());
                }
                count += 1;
            }
            indices.push(c.index);
            lengths.push(c.length);
            digests.push(c.digest.clone());
        }
        let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM unnest($3::bigint[],$4::integer[],$5::text[]) AS input(idx,len,digest) LEFT JOIN inbound_retained_baseline_chunks c ON c.baseline_id=$1 AND c.stage_ordinal=$2 AND c.chunk_index=input.idx WHERE input.idx<$6 AND (c.chunk_index IS NULL OR c.chunk_length<>input.len OR c.digest<>input.digest))").bind(id).bind(ordinal).bind(&indices).bind(&lengths).bind(&digests).bind(original_count).fetch_one(&mut *tx).await?;
        if conflict {
            return Err(invalid());
        }
        let added = count - original_count;
        if added > 0 {
            reserve(&mut tx, 0, added, added * CHUNK_COST).await?;
            sqlx::query("INSERT INTO inbound_retained_baseline_chunks(baseline_id,stage_ordinal,chunk_index,chunk_length,digest) SELECT $1,$2,input.idx,input.len,input.digest FROM unnest($3::bigint[],$4::integer[],$5::text[]) AS input(idx,len,digest) WHERE input.idx >= $6 ORDER BY input.idx").bind(id).bind(ordinal).bind(&indices).bind(&lengths).bind(&digests).bind(original_count).execute(&mut *tx).await?;
            sqlx::query("UPDATE inbound_retained_baselines SET chunk_count=chunk_count+$2,updated_at=clock_timestamp() WHERE id=$1").bind(id).bind(added).execute(&mut *tx).await?;
            sqlx::query("UPDATE inbound_retained_baseline_files SET chunk_count=$3 WHERE baseline_id=$1 AND stage_ordinal=$2").bind(id).bind(ordinal).bind(count).execute(&mut *tx).await?;
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn finish_retained_baseline_file(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        ordinal: i64,
        observed_v3_digest: &str,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        lock_baseline(&mut tx, id, worker, seal, "capturing").await?;
        let updated=sqlx::query("UPDATE inbound_retained_baseline_files SET finished=true WHERE baseline_id=$1 AND stage_ordinal=$2 AND stage_digest=$3 AND chunk_count=file_length/65536 + CASE WHEN file_length%65536=0 THEN 0 ELSE 1 END").bind(id).bind(ordinal).bind(observed_v3_digest).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(invalid());
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Persist the measurement emitted by the protected-stage verifier.
    /// The verifier must derive it from the same freshly created inode, and
    /// independently reopen and remeasure before receipt publication.
    pub async fn record_retained_baseline_file_verity(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        ordinal: i64,
        evidence: &RetainedFileVerity,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        lock_baseline(&mut tx, id, worker, seal, "capturing").await?;
        let row:Option<(bool,Option<String>,Option<String>,Option<String>)>=sqlx::query_as("SELECT finished,stage_verity_device,stage_verity_inode,stage_verity_sha256 FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND stage_ordinal=$2 FOR UPDATE").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?;
        let Some((finished, device, inode, digest)) = row else {
            return Err(invalid());
        };
        if !finished {
            return Err(invalid());
        }
        let expected_device = format!("{:016x}", evidence.device);
        let expected_inode = format!("{:016x}", evidence.inode);
        let expected_digest = blake3::Hash::from_bytes(evidence.digest)
            .to_hex()
            .to_string();
        match (device, inode, digest) {
            (None, None, None) => {
                sqlx::query("UPDATE inbound_retained_baseline_files SET stage_verity_device=$3,stage_verity_inode=$4,stage_verity_sha256=$5 WHERE baseline_id=$1 AND stage_ordinal=$2").bind(id).bind(ordinal).bind(&expected_device).bind(&expected_inode).bind(&expected_digest).execute(&mut *tx).await?;
            }
            (Some(d), Some(i), Some(h))
                if d == expected_device && i == expected_inode && h == expected_digest => {}
            _ => return Err(invalid()),
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Read a bounded per-file record for a fresh kernel remeasurement. This
    /// operation does not establish filesystem identity or grant a receipt.
    pub async fn retained_baseline_file_verity(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        ordinal: i64,
    ) -> Result<RetainedFileVerity, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let state = state.ok_or_else(invalid)?;
        if state != "capturing" && state != "frozen" {
            return Err(invalid());
        }
        lock_baseline(&mut tx, id, worker, seal, &state).await?;
        let row:Option<(String,String,String)>=sqlx::query_as("SELECT stage_verity_device,stage_verity_inode,stage_verity_sha256 FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND stage_ordinal=$2 AND finished AND stage_verity_device IS NOT NULL AND stage_verity_inode IS NOT NULL AND stage_verity_sha256 IS NOT NULL").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?;
        let (device, inode, digest) = row.ok_or_else(invalid)?;
        if !valid_digest(&digest) || device.len() != 16 || inode.len() != 16 {
            return Err(invalid());
        }
        let device = u64::from_str_radix(&device, 16).map_err(|_| invalid())?;
        let inode = u64::from_str_radix(&inode, 16).map_err(|_| invalid())?;
        let mut bytes = [0_u8; 32];
        for (slot, pair) in bytes.iter_mut().zip(digest.as_bytes().chunks_exact(2)) {
            let value = std::str::from_utf8(pair).map_err(|_| invalid())?;
            *slot = u8::from_str_radix(value, 16).map_err(|_| invalid())?;
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(RetainedFileVerity {
            device,
            inode,
            digest: bytes,
        })
    }
    pub async fn freeze_protected_retained_baseline(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
    ) -> Result<RetainedProtectedBaselineFrozen, WorkStoreError> {
        Ok(RetainedProtectedBaselineFrozen {
            inner: self
                .freeze_retained_baseline_inner(pool, worker, id, seal, true)
                .await?,
        })
    }
    pub async fn freeze_retained_baseline(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
    ) -> Result<RetainedBaselineFrozen, WorkStoreError> {
        self.freeze_retained_baseline_inner(pool, worker, id, seal, false)
            .await
    }
    async fn freeze_retained_baseline_inner(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        id: Uuid,
        seal: &SealedCustodyHash,
        require_verity: bool,
    ) -> Result<RetainedBaselineFrozen, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry = locks(&mut tx, worker, seal).await?;
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM inbound_retained_baselines WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let state = state.ok_or_else(invalid)?;
        if state != "capturing" && state != "frozen" {
            return Err(invalid());
        }
        lock_baseline(&mut tx, id, worker, seal, &state).await?;
        let counts:(i64,i64,i64,i64,i64,i64)=sqlx::query_as("SELECT b.file_count,b.chunk_count,(SELECT count(*) FROM custody_hash_spool_entries WHERE capture_id=$2 AND NOT is_directory),(SELECT count(*) FROM inbound_retained_baseline_files WHERE baseline_id=b.id AND finished),(SELECT count(*) FROM custody_hash_spool_entries WHERE capture_id=$2 AND is_directory),(SELECT count(*) FROM inbound_retained_baseline_directories WHERE baseline_id=b.id AND stage_ordinal>0) FROM inbound_retained_baselines b WHERE b.id=$1").bind(id).bind(seal.capture_id()).fetch_one(&mut *tx).await?;
        if counts.0 != counts.2 || counts.0 != counts.3 || counts.4 != counts.5 {
            return Err(invalid());
        }
        if require_verity {
            let missing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND (stage_verity_sha256 IS NULL OR stage_verity_device IS NULL OR stage_verity_inode IS NULL))").bind(id).fetch_one(&mut *tx).await?;
            if missing {
                return Err(invalid());
            }
        }
        if state == "capturing" {
            sqlx::query("UPDATE inbound_retained_baselines SET state='frozen',protected_coverage=$2,updated_at=clock_timestamp() WHERE id=$1").bind(id).bind(require_verity).execute(&mut *tx).await?;
        } else if require_verity {
            sqlx::query("UPDATE inbound_retained_baselines SET protected_coverage=true WHERE id=$1 AND NOT protected_coverage").bind(id).execute(&mut *tx).await?;
        }
        end(&mut tx, seal, expiry).await?;
        tx.commit().await?;
        Ok(RetainedBaselineFrozen {
            id,
            stage_capture: seal.capture_id(),
            stage_epoch: seal.epoch(),
            stage_digest: seal.digest().to_owned(),
            file_count: counts.0,
            chunk_count: counts.1,
        })
    }
    /// Reclaim one bounded page of an abandoned provisional baseline. Live
    /// processing leases and authoritative baselines are never reclaimed.
    pub async fn cleanup_abandoned_retained_baseline_page(
        &self,
        pool: &PgPool,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM inbound_retained_baseline_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let candidate:Option<(Uuid,Uuid)>=sqlx::query_as("SELECT b.id,b.work_item_id FROM inbound_retained_baselines b LEFT JOIN release_work_items w ON w.id=b.work_item_id WHERE b.state<>'authoritative' AND b.updated_at<clock_timestamp()-interval '1 hour' AND (w.id IS NULL OR w.lease_expires_at IS NULL OR w.lease_expires_at<=clock_timestamp()) ORDER BY b.cleanup_after,b.id LIMIT 1").fetch_optional(&mut *tx).await?;
        let Some((id, item)) = candidate else {
            tx.commit().await?;
            return Ok(false);
        };
        let live:Option<bool>=sqlx::query_scalar("SELECT COALESCE(lease_expires_at>clock_timestamp(),false) FROM release_work_items WHERE id=$1 FOR UPDATE").bind(item).fetch_optional(&mut *tx).await?;
        if live == Some(true) {
            tx.commit().await?;
            return Ok(false);
        }
        let eligible:Option<i64>=sqlx::query_scalar("SELECT logical_bytes FROM inbound_retained_baselines WHERE id=$1 AND state<>'authoritative' AND updated_at<clock_timestamp()-interval '1 hour' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let Some(cost) = eligible else {
            tx.commit().await?;
            return Ok(false);
        };
        sqlx::query("UPDATE inbound_retained_baselines SET state='discarding' WHERE id=$1 AND state<>'discarding'").bind(id).execute(&mut *tx).await?;
        let deleted=sqlx::query("WITH doomed AS (SELECT baseline_id,stage_ordinal,chunk_index FROM inbound_retained_baseline_chunks WHERE baseline_id=$1 ORDER BY stage_ordinal,chunk_index LIMIT 128) DELETE FROM inbound_retained_baseline_chunks c USING doomed d WHERE c.baseline_id=d.baseline_id AND c.stage_ordinal=d.stage_ordinal AND c.chunk_index=d.chunk_index").bind(id).execute(&mut *tx).await?.rows_affected();
        let removed: i64 = i64::try_from(deleted).map_err(|_| invalid())?;
        let file_costs:Vec<i64>=sqlx::query_scalar("WITH doomed AS (SELECT f.baseline_id,f.stage_ordinal FROM inbound_retained_baseline_files f WHERE f.baseline_id=$1 AND NOT EXISTS (SELECT 1 FROM inbound_retained_baseline_chunks c WHERE c.baseline_id=f.baseline_id AND c.stage_ordinal=f.stage_ordinal) ORDER BY f.stage_ordinal LIMIT 128) DELETE FROM inbound_retained_baseline_files f USING doomed d WHERE f.baseline_id=d.baseline_id AND f.stage_ordinal=d.stage_ordinal RETURNING f.logical_bytes").bind(id).fetch_all(&mut *tx).await?;
        let file_count = i64::try_from(file_costs.len()).map_err(|_| invalid())?;
        let file_bytes = file_costs.iter().sum::<i64>();
        sqlx::query("UPDATE inbound_retained_baseline_budget SET rows=rows-$1,bytes=bytes-$2 WHERE singleton").bind(removed+file_count).bind(removed*CHUNK_COST+file_bytes).execute(&mut *tx).await?;
        let dir_costs:Vec<i64>=sqlx::query_scalar("WITH doomed AS (SELECT baseline_id,stage_ordinal FROM inbound_retained_baseline_directories WHERE baseline_id=$1 ORDER BY stage_ordinal LIMIT 128) DELETE FROM inbound_retained_baseline_directories d USING doomed x WHERE d.baseline_id=x.baseline_id AND d.stage_ordinal=x.stage_ordinal RETURNING d.logical_bytes").bind(id).fetch_all(&mut *tx).await?;
        let dir_count = i64::try_from(dir_costs.len()).map_err(|_| invalid())?;
        let dir_bytes = dir_costs.iter().sum::<i64>();
        sqlx::query("UPDATE inbound_retained_baseline_budget SET rows=rows-$1,bytes=bytes-$2 WHERE singleton").bind(dir_count).bind(dir_bytes).execute(&mut *tx).await?;
        let remaining: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM inbound_retained_baseline_files WHERE baseline_id=$1) OR EXISTS(SELECT 1 FROM inbound_retained_baseline_directories WHERE baseline_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining {
            sqlx::query(
                "UPDATE inbound_retained_baselines SET cleanup_after=clock_timestamp() WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        if !remaining {
            sqlx::query(
                "DELETE FROM inbound_retained_baselines WHERE id=$1 AND state='discarding'",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE inbound_retained_baseline_budget SET captures=captures-1,bytes=bytes-$1 WHERE singleton").bind(cost).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
