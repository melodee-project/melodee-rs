//! DB-atomic *provisional* protected receipt binding. This API cannot verify a
//! filesystem capability; only the private ingestion verifier may call it
//! after re-opening and validating every protected namespace and sealed file.
use crate::{
    CustodyHashPurpose, ProtectedClaimEpoch, ProtectedStageAttempt,
    RetainedProtectedBaselineFrozen, SealedCustodyHash, WorkItemStore, WorkStoreError,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

/// References to independently verified observations. Supplying these values
/// alone is not a filesystem proof or permission to remove source media.
pub struct ProtectedPublication<'a> {
    pub claim: &'a ProtectedClaimEpoch,
    pub attempt: &'a ProtectedStageAttempt,
    pub baseline: &'a RetainedProtectedBaselineFrozen,
    pub source_before: &'a SealedCustodyHash,
    pub stage: &'a SealedCustodyHash,
    pub source_after: &'a SealedCustodyHash,
    pub session_id: Uuid,
    pub legacy_hash: &'a str,
    /// Verified private final-stage parent identity, not filesystem proof by itself.
    pub lifecycle_root_generation: &'a str,
    /// Verified identity of the separately held lifecycle lockfile.
    pub lifecycle_lock_generation: &'a str,
}
fn invalid() -> WorkStoreError {
    WorkStoreError::InvalidRequest("protected publication evidence changed")
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn lock_seal(
    tx: &mut Transaction<'_, Postgres>,
    seal: &SealedCustodyHash,
    purpose: &str,
    claim_epoch: Uuid,
) -> Result<(), WorkStoreError> {
    let present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND purpose=$4 AND source_generation=$5 AND target_binding COLLATE \"C\"=$6 COLLATE \"C\" AND state='frozen' AND observation_epoch=$7 AND sealed_epoch=$7 AND sealed_digest=$8 AND entry_count=$9 AND protected_claim_epoch=$10 FOR UPDATE)").bind(seal.capture_id()).bind(seal.item_id()).bind(seal.owner()).bind(purpose).bind(seal.generation()).bind(seal.target_binding()).bind(seal.epoch()).bind(seal.digest()).bind(seal.entries()).bind(claim_epoch).fetch_one(&mut **tx).await?;
    if !present {
        return Err(invalid());
    }
    Ok(())
}
/// Opaque ordered proof session for one processing claim. It certifies no
/// filesystem operation; the ingestion verifier supplies that evidence.
#[derive(Debug, Clone)]
pub struct ProtectedRetentionSession {
    id: Uuid,
    claim_epoch: Uuid,
    item_id: Uuid,
}
impl ProtectedRetentionSession {
    pub fn id(&self) -> Uuid {
        self.id
    }
}

async fn lock_claim(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ProtectedClaimEpoch,
) -> Result<OffsetDateTime, WorkStoreError> {
    let expiry: Option<OffsetDateTime> = sqlx::query_scalar("SELECT lease_expires_at FROM release_work_items WHERE id=$1 AND lease_owner=$2 AND protected_attempt_lease_epoch=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
        .bind(claim.item_id).bind(&claim.owner).bind(claim.epoch).fetch_optional(&mut **tx).await?.flatten();
    expiry.ok_or(WorkStoreError::LeaseExpired(claim.item_id))
}
async fn check_expiry(
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
impl WorkItemStore {
    /// Begin an ordered protected observation session under the original claim.
    pub async fn begin_protected_retention_session(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        before: &SealedCustodyHash,
    ) -> Result<ProtectedRetentionSession, WorkStoreError> {
        if before.item_id() != claim.item_id
            || before.owner() != claim.owner
            || before.purpose() != CustodyHashPurpose::Source
            || before.protected_claim_epoch() != Some(claim.epoch)
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_claim(&mut tx, claim).await?;
        let source_matches: bool = sqlx::query_scalar(
            "SELECT source_path COLLATE \"C\"=$2 COLLATE \"C\" FROM release_work_items WHERE id=$1",
        )
        .bind(claim.item_id)
        .bind(before.target_binding())
        .fetch_one(&mut *tx)
        .await?;
        if !source_matches {
            return Err(invalid());
        }
        lock_seal(&mut tx, before, "source", claim.epoch).await?;
        let id = Uuid::now_v7();
        let updated=sqlx::query("INSERT INTO inbound_retained_proof_sessions(work_item_id,session_id,lease_owner,source_capture_id,source_before_epoch,source_before_digest,protected_claim_epoch,state) VALUES($1,$2,$3,$4,$5,$6,$7,'before') ON CONFLICT(work_item_id) DO UPDATE SET session_id=EXCLUDED.session_id,lease_owner=EXCLUDED.lease_owner,source_capture_id=EXCLUDED.source_capture_id,source_before_epoch=EXCLUDED.source_before_epoch,source_before_digest=EXCLUDED.source_before_digest,protected_claim_epoch=EXCLUDED.protected_claim_epoch,staging_capture_id=NULL,staging_epoch=NULL,staging_digest=NULL,staging_legacy_hash=NULL,state='before' WHERE inbound_retained_proof_sessions.state<>'bound'")
            .bind(claim.item_id).bind(id).bind(&claim.owner).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).bind(claim.epoch).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(invalid());
        }
        check_expiry(&mut tx, claim, expiry).await?;
        tx.commit().await?;
        Ok(ProtectedRetentionSession {
            id,
            claim_epoch: claim.epoch,
            item_id: claim.item_id,
        })
    }

    /// Register a matching stage while the original source-before seal remains current.
    pub async fn register_protected_retention_stage(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        session: &ProtectedRetentionSession,
        before: &SealedCustodyHash,
        stage: &SealedCustodyHash,
        legacy_hash: &str,
    ) -> Result<(), WorkStoreError> {
        if session.item_id != claim.item_id
            || session.claim_epoch != claim.epoch
            || !valid_hash(legacy_hash)
            || before.item_id() != claim.item_id
            || stage.item_id() != claim.item_id
            || before.owner() != claim.owner
            || stage.owner() != claim.owner
            || before.purpose() != CustodyHashPurpose::Source
            || stage.purpose() != CustodyHashPurpose::Staging
            || before.protected_claim_epoch() != Some(claim.epoch)
            || stage.protected_claim_epoch() != Some(claim.epoch)
            || before.capture_id() == stage.capture_id()
            || before.digest() != stage.digest()
            || before.entries() != stage.entries()
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        let expiry = lock_claim(&mut tx, claim).await?;
        let session_valid: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_proof_sessions WHERE work_item_id=$1 AND session_id=$2 AND lease_owner=$3 AND protected_claim_epoch=$4 AND state='before' AND source_capture_id=$5 AND source_before_epoch=$6 AND source_before_digest=$7 FOR UPDATE)")
            .bind(claim.item_id).bind(session.id).bind(&claim.owner).bind(claim.epoch).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).fetch_one(&mut *tx).await?;
        if !session_valid {
            return Err(invalid());
        }
        lock_seal(&mut tx, before, "source", claim.epoch).await?;
        lock_seal(&mut tx, stage, "staging", claim.epoch).await?;
        sqlx::query("UPDATE inbound_retained_proof_sessions SET state='staging_verified',staging_capture_id=$2,staging_epoch=$3,staging_digest=$4,staging_legacy_hash=$5 WHERE work_item_id=$1")
            .bind(claim.item_id).bind(stage.capture_id()).bind(stage.epoch()).bind(stage.digest()).bind(legacy_hash).execute(&mut *tx).await?;
        check_expiry(&mut tx, claim, expiry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically bind previously verified evidence. No filesystem work may be
    /// performed in this transaction; callers must already hold and revalidate
    /// their protected namespace capability. The retained source is never
    /// renamed or deleted here. Attempt metadata remains cleanup-pending.
    #[doc(hidden)]
    pub async fn publish_protected_retained(
        &self,
        pool: &PgPool,
        p: ProtectedPublication<'_>,
    ) -> Result<(), WorkStoreError> {
        let claim = p.claim;
        let before = p.source_before;
        let stage = p.stage;
        let after = p.source_after;
        let base = p.baseline.baseline();
        if !valid_hash(p.legacy_hash)
            || p.lifecycle_root_generation.is_empty()
            || p.lifecycle_root_generation.len() > 512
            || p.lifecycle_root_generation.contains('\0')
            || p.lifecycle_lock_generation.is_empty()
            || p.lifecycle_lock_generation.len() > 512
            || p.lifecycle_lock_generation.contains('\0')
            || before.item_id() != claim.item_id
            || stage.item_id() != claim.item_id
            || after.item_id() != claim.item_id
            || before.owner() != claim.owner
            || stage.owner() != claim.owner
            || after.owner() != claim.owner
            || before.purpose() != CustodyHashPurpose::Source
            || after.purpose() != CustodyHashPurpose::Source
            || stage.purpose() != CustodyHashPurpose::Staging
            || before.capture_id() != after.capture_id()
            || before.capture_id() == stage.capture_id()
            || before.epoch() == after.epoch()
            || before.generation() != after.generation()
            || before.target_binding() != after.target_binding()
            || before.digest() != after.digest()
            || before.digest() != stage.digest()
            || before.entries() != stage.entries()
            || before.entries() != after.entries()
            || base.stage_capture() != stage.capture_id()
            || base.stage_epoch() != stage.epoch()
            || base.stage_digest() != stage.digest()
            || p.attempt.work_item_id() != claim.item_id
        {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        sqlx::query(
            "SELECT singleton FROM inbound_protected_attempt_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT singleton FROM inbound_retained_baseline_budget WHERE singleton FOR UPDATE",
        )
        .execute(&mut *tx)
        .await?;
        let hint: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "SELECT source_library_id,source_path FROM release_work_items WHERE id=$1",
        )
        .bind(claim.item_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (library, path) = hint.ok_or(WorkStoreError::LeaseExpired(claim.item_id))?;
        sqlx::query("SELECT lock_inbound_occurrence($1,$2)")
            .bind(library)
            .bind(&path)
            .execute(&mut *tx)
            .await?;
        if crate::work_items::active_purge_guard(&mut tx, library, &path).await? {
            return Err(invalid());
        }
        let item:Option<(Option<Uuid>,String,i16,bool,Option<String>,Option<Uuid>,Option<OffsetDateTime>)>=sqlx::query_as("SELECT source_library_id,source_path,discovery_path_format,inbound_active,lease_owner,protected_attempt_lease_epoch,lease_expires_at FROM release_work_items WHERE id=$1 FOR UPDATE").bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let Some((item_library, item_path, format, active, owner, epoch, expiry)) = item else {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        };
        let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let expiry = expiry.ok_or(WorkStoreError::LeaseExpired(claim.item_id))?;
        if owner.as_deref() != Some(claim.owner.as_str())
            || epoch != Some(claim.epoch)
            || expiry <= now
        {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        }
        if !active
            || library != item_library
            || path != item_path
            || path != after.target_binding()
            || (format == 0 && path.contains('\u{fffd}'))
        {
            return Err(invalid());
        }
        let ordered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_proof_sessions WHERE work_item_id=$1 AND session_id=$2 AND lease_owner=$3 AND state IN ('staging_verified','bound') AND source_capture_id=$4 AND source_before_epoch=$5 AND source_before_digest=$6 AND staging_capture_id=$7 AND staging_epoch=$8 AND staging_digest=$9 AND staging_legacy_hash=$10 AND protected_claim_epoch=$11 FOR UPDATE)").bind(claim.item_id).bind(p.session_id).bind(&claim.owner).bind(before.capture_id()).bind(before.epoch()).bind(before.digest()).bind(stage.capture_id()).bind(stage.epoch()).bind(stage.digest()).bind(p.legacy_hash).bind(claim.epoch).fetch_one(&mut *tx).await?;
        if !ordered {
            return Err(invalid());
        }
        lock_seal(&mut tx, after, "source", claim.epoch).await?;
        lock_seal(&mut tx, stage, "staging", claim.epoch).await?;
        let handoff:Option<(String,String,String,Option<String>,bool)>=sqlx::query_as("SELECT status,source_generation,staging_relative_path,tree_hash,retained_source FROM release_work_item_staging_handoffs WHERE work_item_id=$1 FOR UPDATE").bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let Some((status, source_generation, stage_path, old_hash, retained)) = handoff else {
            return Err(invalid());
        };
        if source_generation != after.generation() || stage_path != stage.target_binding() {
            return Err(invalid());
        }
        let attempt:Option<(String,String,Uuid,String,String,Option<String>,Option<String>,Option<Uuid>,Option<Uuid>,Option<String>,Option<String>)>=sqlx::query_as("SELECT state,owner,claim_epoch,source_generation,final_binding,promoted_generation,promotion_parent_generation,consumed_baseline_id,consumed_session_id,consumed_legacy_hash,consumed_source_digest FROM inbound_protected_stage_attempts WHERE id=$1 AND work_item_id=$2 FOR UPDATE").bind(p.attempt.id()).bind(claim.item_id).fetch_optional(&mut *tx).await?;
        let Some((
            attempt_state,
            attempt_owner,
            attempt_epoch,
            attempt_source,
            attempt_final,
            promoted,
            promotion_parent,
            consumed_base,
            consumed_session,
            consumed_legacy,
            consumed_digest,
        )) = attempt
        else {
            return Err(invalid());
        };
        if attempt_owner != claim.owner
            || attempt_epoch != claim.epoch
            || attempt_source != after.generation()
            || attempt_final != stage.target_binding()
            || promoted.as_deref() != Some(stage.generation())
            || promotion_parent.as_deref() != Some(p.lifecycle_root_generation)
        {
            return Err(invalid());
        }
        let baseline:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_baselines WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND state IN ('frozen','authoritative') AND protected_coverage AND stage_capture_id=$4 AND stage_epoch=$5 AND stage_digest=$6 AND stage_generation=$7 AND stage_binding COLLATE \"C\"=$8 COLLATE \"C\" AND file_count=$9 AND chunk_count=$10 AND protected_claim_epoch=$11 FOR UPDATE)").bind(base.id()).bind(claim.item_id).bind(&claim.owner).bind(stage.capture_id()).bind(stage.epoch()).bind(stage.digest()).bind(stage.generation()).bind(stage.target_binding()).bind(base.file_count()).bind(base.chunk_count()).bind(claim.epoch).fetch_one(&mut *tx).await?;
        if !baseline {
            return Err(invalid());
        }
        if status == "intent" && !retained && attempt_state == "promoted" {
            sqlx::query("UPDATE release_work_item_staging_handoffs SET status='staged',method='verified_copy',tree_hash=$2,staged_at=$3,retained_source=true WHERE work_item_id=$1").bind(claim.item_id).bind(p.legacy_hash).bind(now).execute(&mut *tx).await?;
            let updated=sqlx::query("UPDATE inbound_retained_baselines SET state='authoritative',lifecycle_lock_name=$3,lifecycle_root_generation=$4,lifecycle_lock_generation=$5,updated_at=$2 WHERE id=$1 AND state='frozen' AND protected_coverage").bind(base.id()).bind(now).bind(format!("stage-{}.lock",base.id().simple())).bind(p.lifecycle_root_generation).bind(p.lifecycle_lock_generation).execute(&mut *tx).await?;
            if updated.rows_affected() != 1 {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,baseline_id,source_library_id,source_path,source_path_format,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$8,$8)").bind(claim.item_id).bind(base.id()).bind(library).bind(&path).bind(format).bind(after.generation()).bind(after.digest()).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE inbound_retained_proof_sessions SET state='bound' WHERE work_item_id=$1 AND session_id=$2").bind(claim.item_id).bind(p.session_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE inbound_protected_stage_attempts SET state='consumed',consumed_baseline_id=$2,consumed_session_id=$3,consumed_legacy_hash=$4,consumed_source_digest=$5,updated_at=$6 WHERE id=$1").bind(p.attempt.id()).bind(base.id()).bind(p.session_id).bind(p.legacy_hash).bind(after.digest()).bind(now).execute(&mut *tx).await?;
        } else if status == "staged"
            && retained
            && old_hash.as_deref() == Some(p.legacy_hash)
            && attempt_state == "consumed"
            && consumed_base == Some(base.id())
            && consumed_session == Some(p.session_id)
            && consumed_legacy.as_deref() == Some(p.legacy_hash)
            && consumed_digest.as_deref() == Some(after.digest())
        {
            let existing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_retained_sources WHERE work_item_id=$1 AND baseline_id=$2 AND source_library_id IS NOT DISTINCT FROM $3 AND source_path COLLATE \"C\"=$4 COLLATE \"C\" AND source_path_format=$5 AND source_generation=$6 AND content_digest=$7 AND status='active')").bind(claim.item_id).bind(base.id()).bind(library).bind(&path).bind(format).bind(after.generation()).bind(after.digest()).fetch_one(&mut *tx).await?;
            let authoritative:bool=sqlx::query_scalar("SELECT state='authoritative' AND lifecycle_lock_name=$2 AND lifecycle_root_generation=$3 AND lifecycle_lock_generation=$4 FROM inbound_retained_baselines WHERE id=$1").bind(base.id()).bind(format!("stage-{}.lock",base.id().simple())).bind(p.lifecycle_root_generation).bind(p.lifecycle_lock_generation).fetch_one(&mut *tx).await?;
            if !existing || !authoritative {
                return Err(invalid());
            }
        } else {
            return Err(invalid());
        }
        let live: bool = sqlx::query_scalar("SELECT $1::timestamptz>clock_timestamp()")
            .bind(expiry)
            .fetch_one(&mut *tx)
            .await?;
        if !live {
            return Err(WorkStoreError::LeaseExpired(claim.item_id));
        }
        tx.commit().await?;
        Ok(())
    }
}
