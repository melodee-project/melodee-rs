//! Quota-bounded provisional protected-stage creation provenance.
//!
//! A reserved row is not proof that a directory was created. A crash before
//! the created acknowledgement quarantines that namespace: it cannot be
//! adopted, deleted, published, or silently reclaimed by these APIs.
use crate::{ProtectedClaimEpoch, WorkItemStore, WorkStoreError};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
const MAX_PAGE: i64 = 16;
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("invalid protected stage attempt")
}
fn valid(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
#[derive(Debug, Clone, FromRow)]
pub struct ProtectedStageAttempt {
    id: Uuid,
    work_item_id: Uuid,
    owner: String,
    claim_epoch: Uuid,
    source_library_id: Option<Uuid>,
    source_path: String,
    source_path_format: i16,
    source_generation: String,
    root_generation: String,
    final_binding: String,
    namespace_name: String,
    state: String,
    namespace_generation: Option<String>,
    marker_generation: Option<String>,
    lock_generation: Option<String>,
    promotion_content_generation: Option<String>,
    promotion_parent_generation: Option<String>,
    promoted_generation: Option<String>,
}
impl ProtectedStageAttempt {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn work_item_id(&self) -> Uuid {
        self.work_item_id
    }
    pub fn namespace_name(&self) -> &str {
        &self.namespace_name
    }
    pub fn final_binding(&self) -> &str {
        &self.final_binding
    }
    pub fn root_generation(&self) -> &str {
        &self.root_generation
    }
    pub fn source_generation(&self) -> &str {
        &self.source_generation
    }
    pub fn source_path(&self) -> &str {
        &self.source_path
    }
    pub fn source_library_id(&self) -> Option<Uuid> {
        self.source_library_id
    }
    pub fn source_path_format(&self) -> i16 {
        self.source_path_format
    }
    pub fn namespace_generation(&self) -> Option<&str> {
        self.namespace_generation.as_deref()
    }
    pub fn marker_generation(&self) -> Option<&str> {
        self.marker_generation.as_deref()
    }
    pub fn lock_generation(&self) -> Option<&str> {
        self.lock_generation.as_deref()
    }
    pub fn lock_name(&self) -> String {
        format!("{}.lock", self.namespace_name)
    }
    pub fn is_created(&self) -> bool {
        self.state == "created"
    }
    pub fn state(&self) -> &str {
        &self.state
    }
    pub fn promotion_content_generation(&self) -> Option<&str> {
        self.promotion_content_generation.as_deref()
    }
    pub fn promotion_parent_generation(&self) -> Option<&str> {
        self.promotion_parent_generation.as_deref()
    }
    pub fn promoted_generation(&self) -> Option<&str> {
        self.promoted_generation.as_deref()
    }
}
/// Filesystem identities independently verified by the private stage capability.
#[derive(Debug, Clone, Copy)]
pub struct ProtectedStageIdentity<'a> {
    pub root_generation: &'a str,
    pub namespace_generation: &'a str,
    pub marker_generation: &'a str,
    pub lock_generation: &'a str,
}
/// Read-only evidence for independently verifying an already committed
/// receipt after reclaim. This is not a filesystem proof or cleanup authority.
#[derive(Debug, Clone, FromRow)]
pub struct ProtectedReceiptRecovery {
    pub attempt_id: Uuid,
    pub work_item_id: Uuid,
    pub baseline_id: Uuid,
    pub session_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub source_generation: String,
    pub source_digest: String,
    pub stage_binding: String,
    pub stage_generation: String,
    pub legacy_hash: String,
    pub root_generation: String,
    pub namespace_name: String,
    pub namespace_generation: String,
    pub marker_generation: String,
    pub lock_generation: String,
    pub promotion_parent_generation: String,
    pub lifecycle_lock_name: String,
    pub lifecycle_root_generation: String,
    pub lifecycle_lock_generation: String,
    pub attempt_state: String,
}
/// Persisted receipt identities after attempt cleanup. This is a read-only
/// lookup result, not a filesystem proof or authorization to mutate media.
#[derive(Debug, Clone, FromRow)]
pub struct ProtectedCleanedReceiptRecovery {
    pub attempt_id: Uuid,
    pub work_item_id: Uuid,
    pub baseline_id: Uuid,
    pub session_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub source_path_format: i16,
    pub source_generation: String,
    pub source_digest: String,
    pub stage_binding: String,
    pub stage_generation: String,
    pub legacy_hash: String,
    pub root_generation: String,
    pub final_parent_generation: String,
    pub lifecycle_lock_name: String,
    pub lifecycle_root_generation: String,
    pub lifecycle_lock_generation: String,
    pub cleaned_at: OffsetDateTime,
}
/// Historical database receipt identity. No variant grants filesystem access.
#[derive(Debug, Clone)]
pub enum ClaimedStageReceipt {
    NoReceipt,
    Legacy,
    ProtectedPending(ProtectedReceiptRecovery),
    ProtectedCleaned(ProtectedCleanedReceiptRecovery),
}
/// One exact stored directory identity for bounded stage revalidation. The
/// synthetic root has ordinal 0 and stores the tree digest without its `v3:`
/// prefix; other rows store their directory-entry digest and native path key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedBaselineDirectoryEntry {
    pub ordinal: i64,
    pub path_key: String,
    pub parent_key: String,
    pub stage_digest: String,
}
/// One immutable baseline file observation for a bounded remeasurement pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedBaselineVerityEntry {
    pub ordinal: i64,
    pub path_key: String,
    pub evidence: crate::RetainedFileVerity,
}
const SELECT_ATTEMPT: &str = "SELECT id,work_item_id,owner,claim_epoch,source_library_id,source_path,source_path_format,source_generation,root_generation,final_binding,namespace_name,state,namespace_generation,marker_generation,lock_generation,promotion_content_generation,promotion_parent_generation,promoted_generation FROM inbound_protected_stage_attempts WHERE id=$1 FOR UPDATE";
struct ItemBinding {
    library: Option<Uuid>,
    path: String,
    format: i16,
    expiry: OffsetDateTime,
}
async fn lock_claim(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ProtectedClaimEpoch,
) -> Result<ItemBinding, WorkStoreError> {
    sqlx::query(
        "SELECT singleton FROM inbound_protected_attempt_budget WHERE singleton FOR UPDATE",
    )
    .execute(&mut **tx)
    .await?;
    let row:Option<(Option<Uuid>,String,i16,bool,Option<String>,Option<Uuid>,Option<OffsetDateTime>)>=sqlx::query_as("SELECT source_library_id,source_path,discovery_path_format,inbound_active,lease_owner,protected_attempt_lease_epoch,lease_expires_at FROM release_work_items WHERE id=$1 FOR UPDATE").bind(claim.item_id).fetch_optional(&mut **tx).await?;
    let Some((library, path, format, active, owner, epoch, expiry)) = row else {
        return Err(WorkStoreError::LeaseExpired(claim.item_id));
    };
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if !active
        || owner.as_deref() != Some(claim.owner.as_str())
        || epoch != Some(claim.epoch)
        || expiry.is_none_or(|e| e <= now)
        || (format == 0 && path.contains('\u{fffd}'))
    {
        return Err(WorkStoreError::LeaseExpired(claim.item_id));
    }
    crate::work_items::ensure_item_not_admin_reserved(tx, claim.item_id).await?;
    Ok(ItemBinding {
        library,
        path,
        format,
        expiry: expiry.ok_or(WorkStoreError::LeaseExpired(claim.item_id))?,
    })
}
async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ProtectedClaimEpoch,
    expiry: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
        .bind(expiry)
        .fetch_one(&mut **tx)
        .await?;
    if !live {
        return Err(WorkStoreError::LeaseExpired(claim.item_id));
    }
    Ok(())
}
async fn lock_intent(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    source_generation: &str,
    final_binding: &str,
) -> Result<(), WorkStoreError> {
    let intent:Option<(String,String,String)>=sqlx::query_as("SELECT source_generation,staging_relative_path,status FROM release_work_item_staging_handoffs WHERE work_item_id=$1 FOR UPDATE").bind(item).fetch_optional(&mut **tx).await?;
    if intent
        != Some((
            source_generation.to_owned(),
            final_binding.to_owned(),
            "intent".to_owned(),
        ))
    {
        return Err(invalid());
    }
    Ok(())
}
fn check_binding(
    attempt: &ProtectedStageAttempt,
    claim: &ProtectedClaimEpoch,
    item: &ItemBinding,
) -> bool {
    attempt.work_item_id == claim.item_id
        && attempt.source_library_id == item.library
        && attempt.source_path == item.path
        && attempt.source_path_format == item.format
}
async fn pending_receipt(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ProtectedClaimEpoch,
    item: &ItemBinding,
    attempt_id: Uuid,
) -> Result<Option<ProtectedReceiptRecovery>, WorkStoreError> {
    let row:Option<ProtectedReceiptRecovery>=sqlx::query_as("SELECT a.id AS attempt_id,a.work_item_id,a.consumed_baseline_id AS baseline_id,a.consumed_session_id AS session_id,a.source_library_id,a.source_path,a.source_path_format,a.source_generation,a.consumed_source_digest AS source_digest,a.final_binding AS stage_binding,a.promoted_generation AS stage_generation,a.consumed_legacy_hash AS legacy_hash,a.root_generation,a.namespace_name,a.namespace_generation,a.marker_generation,a.lock_generation,a.promotion_parent_generation,b.lifecycle_lock_name,b.lifecycle_root_generation,b.lifecycle_lock_generation,a.state AS attempt_state FROM inbound_protected_stage_attempts a JOIN inbound_retained_baselines b ON b.id=a.consumed_baseline_id JOIN inbound_retained_sources r ON r.work_item_id=a.work_item_id JOIN release_work_item_staging_handoffs h ON h.work_item_id=a.work_item_id WHERE a.id=$1 AND a.work_item_id=$2 AND a.state IN ('consumed','cleanup_intent') AND a.source_library_id IS NOT DISTINCT FROM $3 AND a.source_path COLLATE \"C\"=$4 COLLATE \"C\" AND a.source_path_format=$5 AND b.work_item_id=a.work_item_id AND b.state='authoritative' AND b.protected_coverage AND b.lifecycle_lock_name IS NOT NULL AND b.lifecycle_root_generation=a.promotion_parent_generation AND b.lifecycle_lock_generation IS NOT NULL AND b.stage_binding COLLATE \"C\"=a.final_binding COLLATE \"C\" AND b.stage_generation=a.promoted_generation AND b.stage_digest=a.consumed_source_digest AND r.baseline_id=b.id AND r.status='active' AND r.source_library_id IS NOT DISTINCT FROM a.source_library_id AND r.source_path COLLATE \"C\"=a.source_path COLLATE \"C\" AND r.source_path_format=a.source_path_format AND r.source_generation=a.source_generation AND r.content_digest=a.consumed_source_digest AND h.status='staged' AND h.retained_source AND h.source_generation=a.source_generation AND h.staging_relative_path COLLATE \"C\"=a.final_binding COLLATE \"C\" AND h.tree_hash=a.consumed_legacy_hash FOR SHARE OF a,b,r,h").bind(attempt_id).bind(claim.item_id).bind(item.library).bind(&item.path).bind(item.format).fetch_optional(&mut **tx).await?;
    Ok(row)
}
async fn cleaned_receipt(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ProtectedClaimEpoch,
    item: &ItemBinding,
    attempt_id: Uuid,
) -> Result<Option<ProtectedCleanedReceiptRecovery>, WorkStoreError> {
    Ok(sqlx::query_as("SELECT b.cleaned_attempt_id AS attempt_id,b.work_item_id,b.id AS baseline_id,b.cleaned_session_id AS session_id,r.source_library_id,r.source_path,r.source_path_format,r.source_generation,r.content_digest AS source_digest,b.stage_binding,b.stage_generation,h.tree_hash AS legacy_hash,b.cleaned_root_generation AS root_generation,b.cleaned_final_parent_generation AS final_parent_generation,b.lifecycle_lock_name,b.lifecycle_root_generation,b.lifecycle_lock_generation,b.cleaned_attempt_at AS cleaned_at FROM inbound_retained_baselines b JOIN inbound_retained_sources r ON r.work_item_id=b.work_item_id JOIN release_work_item_staging_handoffs h ON h.work_item_id=b.work_item_id WHERE b.work_item_id=$1 AND b.cleaned_attempt_id=$2 AND b.cleaned_attempt_at IS NOT NULL AND b.state='authoritative' AND b.protected_coverage AND b.lifecycle_lock_name IS NOT NULL AND b.lifecycle_root_generation=b.cleaned_final_parent_generation AND b.lifecycle_lock_generation IS NOT NULL AND r.baseline_id=b.id AND r.status='active' AND r.source_library_id IS NOT DISTINCT FROM $3 AND r.source_path COLLATE \"C\"=$4 COLLATE \"C\" AND r.source_path_format=$5 AND r.content_digest=b.stage_digest AND h.status='staged' AND h.retained_source AND h.source_generation=r.source_generation AND h.staging_relative_path COLLATE \"C\"=b.stage_binding COLLATE \"C\" AND h.tree_hash IS NOT NULL FOR SHARE OF b,r,h").bind(claim.item_id).bind(attempt_id).bind(item.library).bind(&item.path).bind(item.format).fetch_optional(&mut **tx).await?)
}
fn decode_verity(
    device: &str,
    inode: &str,
    digest: &str,
) -> Result<crate::RetainedFileVerity, WorkStoreError> {
    if device.len() != 16
        || inode.len() != 16
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let device = u64::from_str_radix(device, 16).map_err(|_| invalid())?;
    let inode = u64::from_str_radix(inode, 16).map_err(|_| invalid())?;
    let mut bytes = [0_u8; 32];
    for (slot, pair) in bytes.iter_mut().zip(digest.as_bytes().chunks_exact(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| invalid())?, 16)
            .map_err(|_| invalid())?;
    }
    Ok(crate::RetainedFileVerity {
        device,
        inode,
        digest: bytes,
    })
}
impl WorkItemStore {
    /// Renew only the epoch captured by the original claim. The old expiry
    /// is checked again after the update, so a blocked statement cannot revive
    /// a stale lease or renew a same-owner successor.
    pub async fn renew_protected_claim_epoch(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        duration: Duration,
    ) -> Result<(), WorkStoreError> {
        if duration <= Duration::ZERO || duration > Duration::hours(24) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let row:Option<(Option<String>,Option<Uuid>,Option<OffsetDateTime>,Uuid)>=sqlx::query_as("SELECT lease_owner,protected_attempt_lease_epoch,lease_expires_at,correlation_id FROM release_work_items WHERE id=$1 FOR UPDATE").bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let Some((owner, epoch, old_expiry, correlation)) = row else {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        };
        let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let old_expiry = old_expiry.ok_or(WorkStoreError::LeaseExpired(claim.item_id))?;
        if owner.as_deref() != Some(claim.owner.as_str())
            || epoch != Some(claim.epoch)
            || old_expiry <= now
        {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        }
        let new_expiry = now.checked_add(duration).ok_or_else(invalid)?;
        sqlx::query("UPDATE release_work_items SET lease_expires_at=$2,updated_at=$3 WHERE id=$1")
            .bind(claim.item_id)
            .bind(new_expiry)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        crate::work_items::insert_event(
            &mut tx,
            claim.item_id,
            melodee_domain::WorkEventType::LeaseRenewed,
            None,
            None,
            None,
            Some(claim.owner.as_str()),
            correlation,
            now,
            Some(serde_json::json!({"lease_expires_at":new_expiry.to_string()})),
        )
        .await?;
        let live: bool =
            sqlx::query_scalar("SELECT clock_timestamp() < $1 AND clock_timestamp() < $2")
                .bind(old_expiry)
                .bind(new_expiry)
                .fetch_one(&mut *tx)
                .await?;
        if !live {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Reserve a bounded unique namespace before any filesystem creation.
    /// A lost acknowledgement is intentionally not a created-directory proof.
    pub async fn reserve_protected_stage_attempt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        root_generation: &str,
        source_generation: &str,
        final_binding: &str,
    ) -> Result<ProtectedStageAttempt, WorkStoreError> {
        if !valid(root_generation, 512)
            || !valid(source_generation, 512)
            || !valid(final_binding, 262_200)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let protected: bool = sqlx::query_scalar(
            "SELECT protected_custody_required FROM release_work_items WHERE id=$1",
        )
        .bind(claim.item_id)
        .fetch_one(&mut *tx)
        .await?;
        if !protected {
            return Err(invalid());
        }
        lock_intent(&mut tx, claim.item_id, source_generation, final_binding).await?;
        let id: Uuid = sqlx::query_scalar("SELECT gen_random_uuid()")
            .fetch_one(&mut *tx)
            .await?;
        let name = format!("protected-{}", id.simple());
        let bytes = i64::try_from(
            item.path.len()
                + root_generation.len()
                + source_generation.len()
                + final_binding.len()
                + name.len()
                + 4096,
        )
        .map_err(|_| invalid())?;
        let updated=sqlx::query("UPDATE inbound_protected_attempt_budget SET attempts=attempts+1,bytes=bytes+$1 WHERE singleton AND attempts<max_attempts AND $1<=max_bytes-bytes").bind(bytes).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(WorkStoreError::RetainedSourceBudgetExhausted);
        }
        sqlx::query("INSERT INTO inbound_protected_stage_attempts(id,work_item_id,owner,claim_epoch,source_library_id,source_path,source_path_format,source_generation,root_generation,final_binding,namespace_name,state,logical_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'reserved',$12)").bind(id).bind(claim.item_id).bind(&claim.owner).bind(claim.epoch).bind(item.library).bind(&item.path).bind(item.format).bind(source_generation).bind(root_generation).bind(final_binding).bind(name).bind(bytes).execute(&mut *tx).await?;
        let result: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Revalidate a reservation after taking its external filesystem lock and
    /// before opening/creating the attempt namespace. This is a DB fence only.
    pub async fn validate_reserved_protected_stage_attempt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt: &ProtectedStageAttempt,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        lock_intent(
            &mut tx,
            claim.item_id,
            &attempt.source_generation,
            &attempt.final_binding,
        )
        .await?;
        let row: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(attempt.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(invalid)?;
        if !check_binding(&row, claim, &item)
            || row.owner != claim.owner
            || row.claim_epoch != claim.epoch
            || row.state != "reserved"
        {
            return Err(invalid());
        }
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record the identity only after exclusive marker creation, namespace
    /// revalidation and filesystem durability have succeeded in the caller.
    pub async fn acknowledge_protected_stage_created(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt: &ProtectedStageAttempt,
        identity: &ProtectedStageIdentity<'_>,
    ) -> Result<ProtectedStageAttempt, WorkStoreError> {
        let ProtectedStageIdentity {
            root_generation,
            namespace_generation,
            marker_generation,
            lock_generation,
        } = *identity;
        if !valid(root_generation, 512)
            || !valid(namespace_generation, 512)
            || !valid(marker_generation, 512)
            || !valid(lock_generation, 512)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        lock_intent(
            &mut tx,
            claim.item_id,
            &attempt.source_generation,
            &attempt.final_binding,
        )
        .await?;
        let row: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(attempt.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(invalid)?;
        if !check_binding(&row, claim, &item)
            || row.owner != claim.owner
            || row.claim_epoch != claim.epoch
            || row.root_generation != root_generation
        {
            return Err(invalid());
        }
        if row.state == "reserved" {
            sqlx::query("UPDATE inbound_protected_stage_attempts SET state='created',namespace_generation=$2,marker_generation=$3,lock_generation=$4,updated_at=clock_timestamp() WHERE id=$1").bind(row.id).bind(namespace_generation).bind(marker_generation).bind(lock_generation).execute(&mut *tx).await?;
        } else if row.state != "created"
            || row.namespace_generation.as_deref() != Some(namespace_generation)
            || row.marker_generation.as_deref() != Some(marker_generation)
            || row.lock_generation.as_deref() != Some(lock_generation)
        {
            return Err(invalid());
        }
        let result: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Durably fence a no-replace promotion before touching the filesystem.
    /// The caller must validate the private parents and source-child inode.
    pub async fn begin_protected_stage_promotion(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt: &ProtectedStageAttempt,
        content_generation: &str,
        final_parent_generation: &str,
    ) -> Result<ProtectedStageAttempt, WorkStoreError> {
        if !valid(content_generation, 512) || !valid(final_parent_generation, 512) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        lock_intent(
            &mut tx,
            claim.item_id,
            &attempt.source_generation,
            &attempt.final_binding,
        )
        .await?;
        let row: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(attempt.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(invalid)?;
        if !check_binding(&row, claim, &item)
            || row.owner != claim.owner
            || row.claim_epoch != claim.epoch
        {
            return Err(invalid());
        }
        if row.state == "created" {
            sqlx::query("UPDATE inbound_protected_stage_attempts SET state='promotion_intent',promotion_content_generation=$2,promotion_parent_generation=$3,updated_at=clock_timestamp() WHERE id=$1").bind(row.id).bind(content_generation).bind(final_parent_generation).execute(&mut *tx).await?;
        } else if !matches!(row.state.as_str(), "promotion_intent" | "promoted")
            || row.promotion_content_generation.as_deref() != Some(content_generation)
            || row.promotion_parent_generation.as_deref() != Some(final_parent_generation)
        {
            return Err(invalid());
        }
        let result: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Acknowledge a verified no-replace promotion. The caller must reopen both
    /// parents, confirm exact identity and detect ambiguous crash states first.
    pub async fn acknowledge_protected_stage_promotion(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt: &ProtectedStageAttempt,
        final_generation: &str,
        final_parent_generation: &str,
    ) -> Result<ProtectedStageAttempt, WorkStoreError> {
        if !valid(final_generation, 512) || !valid(final_parent_generation, 512) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        lock_intent(
            &mut tx,
            claim.item_id,
            &attempt.source_generation,
            &attempt.final_binding,
        )
        .await?;
        let row: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(attempt.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(invalid)?;
        if !check_binding(&row, claim, &item)
            || row.owner != claim.owner
            || row.claim_epoch != claim.epoch
            || row.promotion_content_generation.as_deref() != Some(final_generation)
            || row.promotion_parent_generation.as_deref() != Some(final_parent_generation)
        {
            return Err(invalid());
        }
        if row.state == "promotion_intent" {
            sqlx::query("UPDATE inbound_protected_stage_attempts SET state='promoted',promoted_generation=$2,updated_at=clock_timestamp() WHERE id=$1").bind(row.id).bind(final_generation).execute(&mut *tx).await?;
        } else if row.state != "promoted"
            || row.promoted_generation.as_deref() != Some(final_generation)
        {
            return Err(invalid());
        }
        let result: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// List acknowledged, possibly mid-promotion candidates, bounded by keyset.
    /// Listing does not authorize adoption: re-open and independently check
    /// the marker, private parent and exact inode identities first.
    pub async fn list_created_protected_stage_attempts(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<ProtectedStageAttempt>, WorkStoreError> {
        if limit == 0 || i64::try_from(limit).map_err(|_| invalid())? > MAX_PAGE {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let rows:Vec<ProtectedStageAttempt>=sqlx::query_as("SELECT id,work_item_id,owner,claim_epoch,source_library_id,source_path,source_path_format,source_generation,root_generation,final_binding,namespace_name,state,namespace_generation,marker_generation,lock_generation,promotion_content_generation,promotion_parent_generation,promoted_generation FROM inbound_protected_stage_attempts WHERE work_item_id=$1 AND state IN ('created','promotion_intent','promoted') AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3").bind(claim.item_id).bind(after).bind(i64::try_from(limit).map_err(|_|invalid())?).fetch_all(&mut *tx).await?;
        if rows.iter().any(|r| !check_binding(r, claim, &item)) {
            return Err(invalid());
        }
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Adopt an acknowledged attempt only after protected-marker, private
    /// parent and promotion-state revalidation. Reserved attempts are excluded.
    pub async fn adopt_created_protected_stage_attempt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        id: Uuid,
        identity: &ProtectedStageIdentity<'_>,
    ) -> Result<ProtectedStageAttempt, WorkStoreError> {
        let ProtectedStageIdentity {
            root_generation,
            namespace_generation,
            marker_generation,
            lock_generation,
        } = *identity;
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let hint: Option<(String,String)> = sqlx::query_as("SELECT source_generation,final_binding FROM inbound_protected_stage_attempts WHERE id=$1 AND work_item_id=$2").bind(id).bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let (source_generation, final_binding) = hint.ok_or_else(invalid)?;
        lock_intent(&mut tx, claim.item_id, &source_generation, &final_binding).await?;
        let row: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(invalid)?;
        if !check_binding(&row, claim, &item)
            || row.source_generation != source_generation
            || row.final_binding != final_binding
            || !matches!(
                row.state.as_str(),
                "created" | "promotion_intent" | "promoted"
            )
            || row.root_generation != root_generation
            || row.namespace_generation.as_deref() != Some(namespace_generation)
            || row.marker_generation.as_deref() != Some(marker_generation)
            || row.lock_generation.as_deref() != Some(lock_generation)
        {
            return Err(invalid());
        }
        sqlx::query("UPDATE inbound_protected_stage_attempts SET owner=$2,claim_epoch=$3,updated_at=clock_timestamp() WHERE id=$1").bind(id).bind(&claim.owner).bind(claim.epoch).execute(&mut *tx).await?;
        let result: ProtectedStageAttempt = sqlx::query_as(SELECT_ATTEMPT)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Look up an already committed receipt under a fresh claim. Callers must
    /// acquire/revalidate the private writer lock and independently verify
    /// filesystem identities and source bytes before resuming any work.
    pub async fn find_committed_protected_receipt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt_id: Uuid,
    ) -> Result<Option<ProtectedReceiptRecovery>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let row = pending_receipt(&mut tx, claim, &item, attempt_id).await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(row)
    }
    /// Identify a staged receipt under the current claim. Results are historical
    /// metadata only; the caller must independently fence and verify the stage.
    pub async fn claimed_stage_receipt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
    ) -> Result<ClaimedStageReceipt, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let protected: bool = sqlx::query_scalar(
            "SELECT protected_custody_required FROM release_work_items WHERE id=$1",
        )
        .bind(claim.item_id)
        .fetch_one(&mut *tx)
        .await?;
        let handoff: Option<String> = sqlx::query_scalar(
            "SELECT status FROM release_work_item_staging_handoffs WHERE work_item_id=$1 FOR SHARE",
        )
        .bind(claim.item_id)
        .fetch_optional(&mut *tx)
        .await?;
        let baseline: Option<(String,Option<Uuid>)> = sqlx::query_as("SELECT state,cleaned_attempt_id FROM inbound_retained_baselines WHERE work_item_id=$1 FOR SHARE")
            .bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let attempts: Vec<(Uuid,String)> = sqlx::query_as("SELECT id,state FROM inbound_protected_stage_attempts WHERE work_item_id=$1 ORDER BY id LIMIT 2 FOR SHARE")
            .bind(claim.item_id).fetch_all(&mut *tx).await?;
        let result = match (handoff.as_deref(), baseline, attempts.as_slice()) {
            (None | Some("intent"), None, []) if !protected => ClaimedStageReceipt::NoReceipt,
            (Some("staged" | "inbound_removed"), None, []) if !protected => {
                ClaimedStageReceipt::Legacy
            }
            (Some("staged"), Some((state, None)), [(attempt, attempt_state)])
                if protected
                    && state == "authoritative"
                    && matches!(attempt_state.as_str(), "consumed" | "cleanup_intent") =>
            {
                ClaimedStageReceipt::ProtectedPending(
                    pending_receipt(&mut tx, claim, &item, *attempt)
                        .await?
                        .ok_or_else(invalid)?,
                )
            }
            (Some("staged"), Some((state, Some(attempt))), [])
                if protected && state == "authoritative" =>
            {
                ClaimedStageReceipt::ProtectedCleaned(
                    cleaned_receipt(&mut tx, claim, &item, attempt)
                        .await?
                        .ok_or_else(invalid)?,
                )
            }
            _ => return Err(invalid()),
        };
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Read a cleaned receipt using a newly claimed epoch. A private caller
    /// must independently revalidate the final protected stage and source.
    pub async fn find_cleaned_protected_receipt(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt_id: Uuid,
    ) -> Result<Option<ProtectedCleanedReceiptRecovery>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let result = cleaned_receipt(&mut tx, claim, &item, attempt_id).await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Return at most 16 per-file cryptographic metadata records for a private
    /// remeasurement pass. Exact stored paths are returned without decoding.
    pub async fn cleaned_protected_verity_page(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt_id: Uuid,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ProtectedBaselineVerityEntry>, WorkStoreError> {
        if limit == 0 || limit > 16 || after.is_some_and(|n| n < 0) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let receipt = cleaned_receipt(&mut tx, claim, &item, attempt_id)
            .await?
            .ok_or_else(invalid)?;
        let rows:Vec<(i64,String,String,String,String)>=sqlx::query_as("SELECT stage_ordinal,path_key,stage_verity_device,stage_verity_inode,stage_verity_sha256 FROM inbound_retained_baseline_files WHERE baseline_id=$1 AND ($2::bigint IS NULL OR stage_ordinal>$2) AND finished AND stage_verity_device IS NOT NULL AND stage_verity_inode IS NOT NULL AND stage_verity_sha256 IS NOT NULL ORDER BY stage_ordinal LIMIT $3").bind(receipt.baseline_id).bind(after).bind(i64::try_from(limit).map_err(|_|invalid())?).fetch_all(&mut *tx).await?;
        let mut result = Vec::with_capacity(rows.len());
        for (ordinal, path, device, inode, digest) in rows {
            result.push(ProtectedBaselineVerityEntry {
                ordinal,
                path_key: path,
                evidence: decode_verity(&device, &inode, &digest)?,
            });
        }
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Return at most 16 exact directory identities after a cleaned receipt.
    /// Pagination is not an assertion of filesystem completeness or safety.
    pub async fn cleaned_protected_directory_page(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt_id: Uuid,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ProtectedBaselineDirectoryEntry>, WorkStoreError> {
        if limit == 0 || limit > 16 || after.is_some_and(|n| n < 0) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let receipt = cleaned_receipt(&mut tx, claim, &item, attempt_id)
            .await?
            .ok_or_else(invalid)?;
        let rows:Vec<(i64,String,String,String)>=sqlx::query_as("SELECT stage_ordinal,path_key,parent_key,stage_digest FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND ($2::bigint IS NULL OR stage_ordinal>$2) ORDER BY stage_ordinal LIMIT $3").bind(receipt.baseline_id).bind(after).bind(i64::try_from(limit).map_err(|_|invalid())?).fetch_all(&mut *tx).await?;
        let result = rows
            .into_iter()
            .map(
                |(ordinal, path_key, parent_key, stage_digest)| ProtectedBaselineDirectoryEntry {
                    ordinal,
                    path_key,
                    parent_key,
                    stage_digest,
                },
            )
            .collect();
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Compare at most 64 observed exact child paths/types against a cleaned
    /// authoritative baseline under a fresh claim. False only requests fresh
    /// verification; a partial page cannot establish absence or completeness.
    pub async fn cleaned_protected_directory_membership(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        attempt_id: Uuid,
        parent_key: &str,
        members: &[crate::RetainedDirectoryMember],
    ) -> Result<Vec<bool>, WorkStoreError> {
        if parent_key.len() > 262_200
            || parent_key.contains('\0')
            || members.len() > 64
            || members.iter().map(|m| m.path_key.len()).sum::<usize>() > 1_048_576
            || members.iter().any(|m| {
                m.path_key.is_empty() || m.path_key.len() > 262_200 || m.path_key.contains('\0')
            })
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let item = lock_claim(&mut tx, claim).await?;
        let receipt = cleaned_receipt(&mut tx, claim, &item, attempt_id)
            .await?
            .ok_or_else(invalid)?;
        let parent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baseline_directories WHERE baseline_id=$1 AND ingest_occurrence_hash(path_key)=ingest_occurrence_hash($2) AND path_key COLLATE \"C\"=$2 COLLATE \"C\")").bind(receipt.baseline_id).bind(parent_key).fetch_one(&mut *tx).await?;
        if !parent {
            return Err(invalid());
        }
        let paths: Vec<&str> = members.iter().map(|m| m.path_key.as_str()).collect();
        let dirs: Vec<bool> = members.iter().map(|m| m.is_directory).collect();
        let result:Vec<bool>=sqlx::query_scalar("SELECT CASE WHEN x.is_dir THEN EXISTS(SELECT 1 FROM inbound_retained_baseline_directories d WHERE d.baseline_id=$1 AND ingest_occurrence_hash(d.path_key)=ingest_occurrence_hash(x.path) AND d.path_key COLLATE \"C\"=x.path COLLATE \"C\" AND d.parent_key COLLATE \"C\"=$2 COLLATE \"C\") ELSE EXISTS(SELECT 1 FROM inbound_retained_baseline_files f WHERE f.baseline_id=$1 AND ingest_occurrence_hash(f.path_key)=ingest_occurrence_hash(x.path) AND f.path_key COLLATE \"C\"=x.path COLLATE \"C\" AND f.parent_key COLLATE \"C\"=$2 COLLATE \"C\") END FROM unnest($3::text[],$4::boolean[]) WITH ORDINALITY AS x(path,is_dir,ord) ORDER BY x.ord").bind(receipt.baseline_id).bind(parent_key).bind(&paths).bind(&dirs).fetch_all(&mut *tx).await?;
        finish(&mut tx, claim, item.expiry).await?;
        tx.commit().await?;
        Ok(result)
    }
}
