//! Bounded provisional custody digest evidence. This is not a staging receipt
//! and does not prove that filesystem enumeration was complete or stable.

use std::collections::BTreeMap;

use sqlx::{Acquire, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ProtectedClaimEpoch, WorkItemStore, WorkStoreError, WorkerId, work_items};

pub const CUSTODY_HASH_PAGE_SIZE: usize = 128;
pub const CUSTODY_HASH_PAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyHashPurpose {
    Source,
    Staging,
    Retained,
}

impl CustodyHashPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Staging => "staging",
            Self::Retained => "retained",
        }
    }
}

/// Exact reversible occurrence path and its content observation. The caller
/// must use a canonical, platform-specific path encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyHashObservation {
    pub path_key: String,
    pub parent_key: String,
    pub name_key: String,
    pub legacy_path_hex: String,
    pub is_directory: bool,
    pub legacy_length: i64,
    pub digest: String,
}

impl CustodyHashObservation {
    pub fn logical_bytes(&self) -> usize {
        self.path_key
            .len()
            .saturating_add(self.parent_key.len())
            .saturating_add(self.name_key.len())
            .saturating_add(self.legacy_path_hex.len())
            .saturating_add(384)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyHashEntry {
    pub ordinal: i64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyHashCursor {
    pub digest: String,
    pub ordinal: i64,
}

#[derive(Debug, Clone)]
pub struct CustodyLegacyEntry {
    pub ordinal: i64,
    pub path_key: String,
    pub name_key: String,
    pub legacy_path_hex: String,
    pub is_directory: bool,
    pub legacy_length: i64,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct CustodyLegacyCursor {
    pub name_key: String,
    pub ordinal: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyHashSpool {
    pub id: Uuid,
    pub source_generation: String,
    pub target_binding: String,
    pub state: String,
    pub retained_lease_token: Option<Uuid>,
}

/// Store-computed digest of frozen provisional observations. Its fields are
/// private so an arbitrary caller cannot fabricate a binding token.
#[derive(Debug, Clone)]
pub struct SealedCustodyHash {
    capture_id: Uuid,
    item_id: Uuid,
    owner: String,
    purpose: CustodyHashPurpose,
    retained_lease_token: Option<Uuid>,
    protected_claim_epoch: Option<Uuid>,
    generation: String,
    target_binding: String,
    epoch: Uuid,
    digest: String,
    entries: i64,
}

impl SealedCustodyHash {
    pub fn capture_id(&self) -> Uuid {
        self.capture_id
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn entries(&self) -> i64 {
        self.entries
    }
    pub fn item_id(&self) -> Uuid {
        self.item_id
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn purpose(&self) -> CustodyHashPurpose {
        self.purpose
    }
    pub fn retained_lease_token(&self) -> Option<Uuid> {
        self.retained_lease_token
    }
    pub(crate) fn protected_claim_epoch(&self) -> Option<Uuid> {
        self.protected_claim_epoch
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn target_binding(&self) -> &str {
        &self.target_binding
    }
    pub fn epoch(&self) -> Uuid {
        self.epoch
    }
}

fn database_error(error: sqlx::Error) -> WorkStoreError {
    if matches!(&error, sqlx::Error::Database(db) if db.code().as_deref() == Some("53400")) {
        WorkStoreError::CustodyHashBudgetExhausted
    } else {
        WorkStoreError::Database(error)
    }
}

// All API mutations lock the singleton before work item and capture rows.
// The accounting triggers use the same order, preventing quota races.
async fn lock_budget(tx: &mut Transaction<'_, Postgres>) -> Result<(), WorkStoreError> {
    sqlx::query("SELECT singleton FROM custody_hash_spool_budget WHERE singleton FOR UPDATE")
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_item(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    worker: &WorkerId,
    now: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let item = work_items::lock_for_update(tx, item).await?;
    work_items::ensure_active_lease(&item, worker, now)?;
    ensure_db_lease_live(tx, item.id, worker).await
}

async fn ensure_db_lease_live(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    worker: &WorkerId,
) -> Result<(), WorkStoreError> {
    let live: bool = sqlx::query_scalar("SELECT COALESCE(lease_owner=$2 AND lease_expires_at > clock_timestamp(),false) FROM release_work_items WHERE id=$1")
        .bind(item).bind(worker.as_str()).fetch_one(&mut **tx).await?;
    if !live {
        return Err(WorkStoreError::LeaseExpired(item));
    }
    Ok(())
}

// Retained observations use an independent lease so completed work items do not
// contend with processing claims. The row lock serializes takeover with writes.
async fn lock_spool_lease(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    worker: &WorkerId,
    now: OffsetDateTime,
    retained_token: Option<Uuid>,
    protected: Option<&ProtectedClaimEpoch>,
) -> Result<(), WorkStoreError> {
    if let Some(claim) = protected {
        if retained_token.is_some() || claim.item_id != item || claim.owner != worker.as_str() {
            return Err(WorkStoreError::LeaseExpired(item));
        }
        let live: Option<bool> = sqlx::query_scalar("SELECT true FROM release_work_items WHERE id=$1 AND lease_owner=$2 AND protected_attempt_lease_epoch=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
            .bind(item).bind(worker.as_str()).bind(claim.epoch).fetch_optional(&mut **tx).await?;
        return live.map(|_| ()).ok_or(WorkStoreError::LeaseExpired(item));
    }
    if let Some(token) = retained_token {
        let live: Option<bool> = sqlx::query_scalar("SELECT true FROM inbound_retained_sources WHERE work_item_id=$1 AND status='active' AND lease_owner=$2 AND lease_token=$3 AND lease_expires_at>clock_timestamp() FOR UPDATE")
            .bind(item).bind(worker.as_str()).bind(token).fetch_optional(&mut **tx).await?;
        if live.is_none() {
            return Err(WorkStoreError::LeaseExpired(item));
        }
        Ok(())
    } else {
        lock_item(tx, item, worker, now).await
    }
}

async fn ensure_spool_lease_live(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    worker: &WorkerId,
    retained_token: Option<Uuid>,
    protected: Option<&ProtectedClaimEpoch>,
) -> Result<(), WorkStoreError> {
    if let Some(claim) = protected {
        if retained_token.is_some() || claim.item_id != item || claim.owner != worker.as_str() {
            return Err(WorkStoreError::LeaseExpired(item));
        }
        let live: Option<bool> = sqlx::query_scalar("SELECT true FROM release_work_items WHERE id=$1 AND lease_owner=$2 AND protected_attempt_lease_epoch=$3 AND lease_expires_at>clock_timestamp()")
            .bind(item).bind(worker.as_str()).bind(claim.epoch).fetch_optional(&mut **tx).await?;
        return live.map(|_| ()).ok_or(WorkStoreError::LeaseExpired(item));
    }
    if let Some(token) = retained_token {
        let live: Option<bool> = sqlx::query_scalar("SELECT true FROM inbound_retained_sources WHERE work_item_id=$1 AND status='active' AND lease_owner=$2 AND lease_token=$3 AND lease_expires_at>clock_timestamp()")
            .bind(item).bind(worker.as_str()).bind(token).fetch_optional(&mut **tx).await?;
        if live.is_none() {
            return Err(WorkStoreError::LeaseExpired(item));
        }
        Ok(())
    } else {
        ensure_db_lease_live(tx, item, worker).await
    }
}

async fn lock_spool_token(
    tx: &mut Transaction<'_, Postgres>,
    capture: Uuid,
    item: Uuid,
    retained_token: Option<Uuid>,
    protected: Option<&ProtectedClaimEpoch>,
) -> Result<(), WorkStoreError> {
    let found: Option<bool> = sqlx::query_scalar("SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND retained_lease_token IS NOT DISTINCT FROM $3 AND protected_claim_epoch IS NOT DISTINCT FROM $4 FOR UPDATE")
        .bind(capture).bind(item).bind(retained_token).bind(protected.map(|claim| claim.epoch)).fetch_optional(&mut **tx).await?;
    if found.is_none() {
        return Err(WorkStoreError::InvalidRequest(
            "custody hash capture lease changed",
        ));
    }
    Ok(())
}

async fn lock_capture(
    tx: &mut Transaction<'_, Postgres>,
    capture: Uuid,
    item: Uuid,
    worker: &WorkerId,
    state: &str,
) -> Result<(), WorkStoreError> {
    let found: Option<bool> = sqlx::query_scalar(
        "SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND state=$4 FOR UPDATE",
    ).bind(capture).bind(item).bind(worker.as_str()).bind(state)
        .fetch_optional(&mut **tx).await?;
    if found.is_none() {
        return Err(WorkStoreError::InvalidRequest(
            "custody hash capture binding changed",
        ));
    }
    Ok(())
}

impl WorkItemStore {
    /// Renew using the database clock after acquiring the work-item lock, so a
    /// queued heartbeat cannot resurrect a lease which expired while waiting.
    pub async fn renew_custody_hash_lease(
        &self,
        pool: &PgPool,
        item: Uuid,
        worker: &WorkerId,
        duration: time::Duration,
    ) -> Result<(), WorkStoreError> {
        if duration <= time::Duration::ZERO {
            return Err(WorkStoreError::InvalidRequest(
                "custody lease duration must be positive",
            ));
        }
        let acquired = std::time::Instant::now();
        let mut connection = pool.acquire().await?;
        crate::metrics::observe(crate::metrics::Metric::HeartbeatAcquire, acquired.elapsed());
        let _hold = crate::metrics::HoldTimer::new(crate::metrics::Metric::HeartbeatHold);
        let mut tx = (&mut *connection).begin().await?;
        let item = work_items::lock_for_update(&mut tx, item).await?;
        work_items::try_lock_unguarded_occurrence(&mut tx, &item).await?;
        let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        work_items::ensure_active_lease(&item, worker, db_now)?;
        work_items::ensure_ordinary_custody(&mut tx, item.id).await?;
        let original_expiry = item
            .lease_expires_at
            .ok_or(WorkStoreError::LeaseExpired(item.id))?;
        let expiry = db_now
            .checked_add(duration)
            .ok_or(WorkStoreError::InvalidRequest(
                "custody lease duration overflows",
            ))?;
        sqlx::query("UPDATE release_work_items SET lease_expires_at=$2,updated_at=$3 WHERE id=$1")
            .bind(item.id)
            .bind(expiry)
            .bind(db_now)
            .execute(&mut *tx)
            .await?;
        work_items::insert_event(
            &mut tx,
            item.id,
            melodee_domain::WorkEventType::LeaseRenewed,
            None,
            None,
            None,
            Some(worker.as_str()),
            item.correlation_id,
            db_now,
            Some(serde_json::json!({"lease_expires_at":expiry.to_string()})),
        )
        .await?;
        let original_is_live: bool = sqlx::query_scalar("SELECT now_at_check < $1 AND now_at_check < $2 FROM (SELECT clock_timestamp() AS now_at_check) AS clock")
            .bind(original_expiry)
            .bind(expiry)
            .fetch_one(&mut *tx)
            .await?;
        if !original_is_live {
            return Err(WorkStoreError::LeaseExpired(item.id));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Locate provisional evidence for restart; it is never a receipt.
    pub async fn custody_hash_spool_for_item(
        &self,
        pool: &PgPool,
        item: Uuid,
        purpose: CustodyHashPurpose,
    ) -> Result<Option<CustodyHashSpool>, WorkStoreError> {
        let row: Option<(Uuid, String, String, String, Option<Uuid>)> = sqlx::query_as("SELECT id,source_generation,target_binding,state,retained_lease_token FROM custody_hash_spool_captures WHERE work_item_id=$1 AND purpose=$2")
            .bind(item).bind(purpose.as_str()).fetch_optional(pool).await?;
        Ok(row.map(
            |(id, source_generation, target_binding, state, retained_lease_token)| {
                CustodyHashSpool {
                    id,
                    source_generation,
                    target_binding,
                    state,
                    retained_lease_token,
                }
            },
        ))
    }

    /// Create provisional observations bound to the exact processing claim.
    pub async fn begin_protected_custody_hash_spool(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        worker: &WorkerId,
        purpose: CustodyHashPurpose,
        generation: &str,
        binding: &str,
        now: OffsetDateTime,
    ) -> Result<Uuid, WorkStoreError> {
        self.begin_custody_hash_spool_fenced(
            pool,
            claim.item_id,
            worker,
            purpose,
            generation,
            binding,
            now,
            None,
            Some(claim),
        )
        .await
    }

    pub async fn append_protected_custody_hash_spool(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        capture: Uuid,
        worker: &WorkerId,
        entries: &[CustodyHashObservation],
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.append_custody_hash_spool_fenced(
            pool,
            capture,
            claim.item_id,
            worker,
            entries,
            now,
            None,
            Some(claim),
        )
        .await
    }

    pub async fn revalidate_protected_custody_hash_spool(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        capture: Uuid,
        worker: &WorkerId,
        generation: &str,
        observation_epoch: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.revalidate_custody_hash_spool_fenced(
            pool,
            capture,
            claim.item_id,
            worker,
            generation,
            observation_epoch,
            now,
            None,
            Some(claim),
        )
        .await
    }

    pub async fn freeze_protected_custody_hash_spool(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        capture: Uuid,
        worker: &WorkerId,
        count: i64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.freeze_custody_hash_spool_fenced(
            pool,
            capture,
            claim.item_id,
            worker,
            count,
            now,
            None,
            Some(claim),
        )
        .await
    }

    pub async fn seal_protected_custody_hash_spool(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        capture: Uuid,
        worker: &WorkerId,
    ) -> Result<SealedCustodyHash, WorkStoreError> {
        self.seal_custody_hash_spool_fenced(pool, capture, claim.item_id, worker, None, Some(claim))
            .await
    }

    /// Create one provisional capture. A resumed worker must explicitly
    /// reconcile an existing capture; this method never replaces its evidence.
    pub async fn begin_custody_hash_spool(
        &self,
        pool: &PgPool,
        item: Uuid,
        worker: &WorkerId,
        purpose: CustodyHashPurpose,
        source_generation: &str,
        target_binding: &str,
        now: OffsetDateTime,
    ) -> Result<Uuid, WorkStoreError> {
        self.begin_custody_hash_spool_fenced(
            pool,
            item,
            worker,
            purpose,
            source_generation,
            target_binding,
            now,
            None,
            None,
        )
        .await
    }

    pub async fn begin_custody_hash_spool_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        item: Uuid,
        worker: &WorkerId,
        purpose: CustodyHashPurpose,
        source_generation: &str,
        target_binding: &str,
        now: OffsetDateTime,
    ) -> Result<Uuid, WorkStoreError> {
        self.begin_custody_hash_spool_fenced(
            pool,
            item,
            worker,
            purpose,
            source_generation,
            target_binding,
            now,
            Some(retained_token),
            None,
        )
        .await
    }

    async fn begin_custody_hash_spool_fenced(
        &self,
        pool: &PgPool,
        item: Uuid,
        worker: &WorkerId,
        purpose: CustodyHashPurpose,
        source_generation: &str,
        target_binding: &str,
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<Uuid, WorkStoreError> {
        if (purpose == CustodyHashPurpose::Retained) != retained_token.is_some()
            || (protected.is_some() && retained_token.is_some())
        {
            return Err(WorkStoreError::InvalidRequest(
                "custody purpose and lease differ",
            ));
        }
        if source_generation.is_empty()
            || source_generation.len() > 512
            || worker.as_str().len() > 512
            || target_binding.is_empty()
            || target_binding.len() > 262_200
            || target_binding.contains('\0')
        {
            return Err(WorkStoreError::InvalidRequest(
                "invalid custody source generation or worker",
            ));
        }
        let mut tx = pool.begin().await?;
        lock_budget(&mut tx).await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, protected).await?;
        if retained_token.is_some() {
            let matches: bool = sqlx::query_scalar("SELECT source_path COLLATE \"C\"=$2 COLLATE \"C\" FROM inbound_retained_sources WHERE work_item_id=$1")
                .bind(item).bind(target_binding).fetch_one(&mut *tx).await?;
            if !matches {
                return Err(WorkStoreError::InvalidRequest(
                    "retained source path changed",
                ));
            }
        }
        let capture = Uuid::now_v7();
        sqlx::query("INSERT INTO custody_hash_spool_captures (id,work_item_id,lease_owner,purpose,source_generation,target_binding,state,observation_epoch,created_at,updated_at,retained_lease_token,protected_claim_epoch) VALUES ($1,$2,$3,$4,$5,$6,'capturing',$7,$8,$8,$9,$10)")
            .bind(capture).bind(item).bind(worker.as_str()).bind(purpose.as_str()).bind(source_generation).bind(target_binding).bind(Uuid::now_v7()).bind(now).bind(retained_token).bind(protected.map(|claim| claim.epoch))
            .execute(&mut *tx).await.map_err(database_error)?;
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, protected).await?;
        tx.commit().await?;
        Ok(capture)
    }

    /// A new lease owner may retain staged records only as provisional hints.
    /// Every previous path must be read and verified again in the new epoch
    /// before freezing; a changed path fails instead of replacing evidence.
    pub async fn revalidate_custody_hash_spool(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        source_generation: &str,
        observation_epoch: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.revalidate_custody_hash_spool_fenced(
            pool,
            capture,
            item,
            worker,
            source_generation,
            observation_epoch,
            now,
            None,
            None,
        )
        .await
    }

    pub async fn revalidate_custody_hash_spool_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        source_generation: &str,
        observation_epoch: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.revalidate_custody_hash_spool_fenced(
            pool,
            capture,
            item,
            worker,
            source_generation,
            observation_epoch,
            now,
            Some(retained_token),
            None,
        )
        .await
    }

    async fn revalidate_custody_hash_spool_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        source_generation: &str,
        observation_epoch: Uuid,
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, protected).await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        let updated = sqlx::query("UPDATE custody_hash_spool_captures SET lease_owner=$3,state='revalidating',observation_epoch=$4,sealed_digest=NULL,sealed_epoch=NULL,updated_at=$5 WHERE id=$1 AND work_item_id=$2 AND source_generation=$6 AND state IN ('capturing','revalidating','frozen') AND (observation_epoch<>$4 OR (state='revalidating' AND lease_owner=$3))")
            .bind(capture).bind(item).bind(worker.as_str()).bind(observation_epoch).bind(now).bind(source_generation)
            .execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash revalidation binding changed",
            ));
        }
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, protected).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Append one bounded idempotent page. Exact paths are deduplicated even
    /// when replay order changes; different evidence for a path rolls back.
    pub async fn append_custody_hash_spool(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        entries: &[CustodyHashObservation],
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.append_custody_hash_spool_fenced(pool, capture, item, worker, entries, now, None, None)
            .await
    }

    pub async fn append_custody_hash_spool_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        entries: &[CustodyHashObservation],
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.append_custody_hash_spool_fenced(
            pool,
            capture,
            item,
            worker,
            entries,
            now,
            Some(retained_token),
            None,
        )
        .await
    }

    async fn append_custody_hash_spool_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        entries: &[CustodyHashObservation],
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<(), WorkStoreError> {
        if entries.len() > CUSTODY_HASH_PAGE_SIZE {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash page exceeds 128 entries",
            ));
        }
        if entries.iter().fold(0_usize, |total, entry| {
            total.saturating_add(entry.logical_bytes())
        }) > CUSTODY_HASH_PAGE_BYTES
        {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash page exceeds byte limit",
            ));
        }
        let mut unique = BTreeMap::new();
        for entry in entries {
            if entry.path_key.is_empty()
                || entry.path_key.len() > 262_200
                || entry.path_key.contains('\0')
                || entry.parent_key.len() > 262_200
                || entry.parent_key.contains('\0')
                || entry.name_key.is_empty()
                || entry.name_key.len() > 262_144
                || entry.name_key.len() % 2 != 0
                || !entry
                    .name_key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || entry.legacy_path_hex.is_empty()
                || entry.legacy_path_hex.len() > 1_048_576
                || entry.legacy_path_hex.len() % 2 != 0
                || !entry
                    .legacy_path_hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || entry.legacy_length < 0
                || entry.digest.len() != 64
                || !entry
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(WorkStoreError::InvalidRequest("invalid custody hash entry"));
            }
            if unique
                .insert(entry.path_key.as_str(), entry)
                .is_some_and(|old| old != entry)
            {
                return Err(WorkStoreError::InvalidRequest(
                    "conflicting custody hash path",
                ));
            }
        }
        let paths: Vec<&str> = unique.keys().copied().collect();
        let digests: Vec<&str> = unique.values().map(|entry| entry.digest.as_str()).collect();
        let parents: Vec<&str> = unique
            .values()
            .map(|entry| entry.parent_key.as_str())
            .collect();
        let names: Vec<&str> = unique
            .values()
            .map(|entry| entry.name_key.as_str())
            .collect();
        let legacy_paths: Vec<&str> = unique
            .values()
            .map(|entry| entry.legacy_path_hex.as_str())
            .collect();
        let directories: Vec<bool> = unique.values().map(|entry| entry.is_directory).collect();
        let lengths: Vec<i64> = unique.values().map(|entry| entry.legacy_length).collect();
        let mut tx = pool.begin().await?;
        lock_budget(&mut tx).await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, protected).await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        let epoch: Option<Uuid> = sqlx::query_scalar("SELECT observation_epoch FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND lease_owner=$3 AND state IN ('capturing','revalidating') FOR UPDATE")
            .bind(capture).bind(item).bind(worker.as_str()).fetch_optional(&mut *tx).await?;
        let epoch = epoch.ok_or(WorkStoreError::InvalidRequest(
            "custody hash capture binding changed",
        ))?;
        let mismatch: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM UNNEST($2::text[],$3::text[],$4::text[],$5::text[],$6::boolean[],$7::bigint[],$8::text[]) e(path_key,digest,parent_key,name_key,is_directory,legacy_length,legacy_path_hex) JOIN custody_hash_spool_entries s ON s.capture_id=$1 AND ingest_occurrence_hash(s.path_key)=ingest_occurrence_hash(e.path_key) AND s.path_key COLLATE \"C\"=e.path_key COLLATE \"C\" WHERE s.digest<>e.digest OR s.parent_key COLLATE \"C\"<>e.parent_key COLLATE \"C\" OR s.name_key<>e.name_key OR s.is_directory<>e.is_directory OR s.legacy_length<>e.legacy_length OR s.legacy_path_hex<>e.legacy_path_hex)")
            .bind(capture).bind(&paths).bind(&digests).bind(&parents).bind(&names).bind(&directories).bind(&lengths).bind(&legacy_paths).fetch_one(&mut *tx).await?;
        if mismatch {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash path already has different evidence",
            ));
        }
        sqlx::query("UPDATE custody_hash_spool_entries s SET observed_epoch=$4 FROM UNNEST($2::text[],$3::text[]) e(path_key,digest) WHERE s.capture_id=$1 AND ingest_occurrence_hash(s.path_key)=ingest_occurrence_hash(e.path_key) AND s.path_key COLLATE \"C\"=e.path_key COLLATE \"C\"")
            .bind(capture).bind(&paths).bind(&digests).bind(epoch).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO custody_hash_spool_entries(capture_id,path_key,digest,parent_key,name_key,is_directory,legacy_length,legacy_path_hex,observed_epoch) SELECT $1,e.path_key,e.digest,e.parent_key,e.name_key,e.is_directory,e.legacy_length,e.legacy_path_hex,$9 FROM UNNEST($2::text[],$3::text[],$4::text[],$5::text[],$6::boolean[],$7::bigint[],$8::text[]) e(path_key,digest,parent_key,name_key,is_directory,legacy_length,legacy_path_hex) WHERE NOT EXISTS (SELECT 1 FROM custody_hash_spool_entries s WHERE s.capture_id=$1 AND ingest_occurrence_hash(s.path_key)=ingest_occurrence_hash(e.path_key) AND s.path_key COLLATE \"C\"=e.path_key COLLATE \"C\")")
            .bind(capture).bind(&paths).bind(&digests).bind(&parents).bind(&names).bind(&directories).bind(&lengths).bind(&legacy_paths).bind(epoch).execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query("UPDATE custody_hash_spool_captures SET updated_at=$2 WHERE id=$1")
            .bind(capture)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, protected).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Freeze entries before ordered reading. Freezing alone does not certify
    /// filesystem traversal or authorize a custody transition.
    pub async fn freeze_custody_hash_spool(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        expected_entries: i64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.freeze_custody_hash_spool_fenced(
            pool,
            capture,
            item,
            worker,
            expected_entries,
            now,
            None,
            None,
        )
        .await
    }

    pub async fn freeze_custody_hash_spool_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        expected_entries: i64,
        now: OffsetDateTime,
    ) -> Result<(), WorkStoreError> {
        self.freeze_custody_hash_spool_fenced(
            pool,
            capture,
            item,
            worker,
            expected_entries,
            now,
            Some(retained_token),
            None,
        )
        .await
    }

    async fn freeze_custody_hash_spool_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        expected_entries: i64,
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<(), WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, protected).await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        let updated = sqlx::query("UPDATE custody_hash_spool_captures c SET state='frozen',updated_at=$4 WHERE c.id=$1 AND c.work_item_id=$2 AND c.lease_owner=$3 AND c.entry_count=$5 AND c.state IN ('capturing','revalidating','frozen') AND NOT EXISTS (SELECT 1 FROM custody_hash_spool_entries e WHERE e.capture_id=c.id AND e.observed_epoch<>c.observation_epoch)")
            .bind(capture).bind(item).bind(worker.as_str()).bind(now).bind(expected_entries).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash revalidation is incomplete",
            ));
        }
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, protected).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Read a deterministic bounded page in digest order. Duplicate digests
    /// are retained, with ordinal serving only as a stable pagination key.
    pub async fn custody_hash_spool_page(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        after: Option<&CustodyHashCursor>,
        now: OffsetDateTime,
    ) -> Result<Vec<CustodyHashEntry>, WorkStoreError> {
        self.custody_hash_spool_page_fenced(pool, capture, item, worker, after, now, None, None)
            .await
    }

    pub async fn custody_hash_spool_page_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        after: Option<&CustodyHashCursor>,
        now: OffsetDateTime,
    ) -> Result<Vec<CustodyHashEntry>, WorkStoreError> {
        self.custody_hash_spool_page_fenced(
            pool,
            capture,
            item,
            worker,
            after,
            now,
            Some(retained_token),
            None,
        )
        .await
    }

    async fn custody_hash_spool_page_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        after: Option<&CustodyHashCursor>,
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<Vec<CustodyHashEntry>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, protected).await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        lock_capture(&mut tx, capture, item, worker, "frozen").await?;
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT ordinal,digest FROM custody_hash_spool_entries WHERE capture_id=$1 AND ($2::text IS NULL OR (digest,ordinal)>($2::text COLLATE \"C\",$3::bigint)) ORDER BY digest,ordinal LIMIT 128")
            .bind(capture).bind(after.map(|cursor| &cursor.digest)).bind(after.map(|cursor| cursor.ordinal))
            .fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|(ordinal, digest)| CustodyHashEntry { ordinal, digest })
            .collect())
    }

    /// A bounded, exact-parent sibling page in native filename order. The
    /// indexed prefix narrows work while full keys resolve long common prefixes.
    #[allow(clippy::too_many_arguments)]
    pub async fn custody_legacy_siblings_page(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        parent: &str,
        after: Option<&CustodyLegacyCursor>,
        directories_only: bool,
        descending: bool,
        now: OffsetDateTime,
    ) -> Result<Vec<CustodyLegacyEntry>, WorkStoreError> {
        self.custody_legacy_siblings_page_fenced(
            pool,
            capture,
            item,
            worker,
            parent,
            after,
            directories_only,
            descending,
            now,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn custody_legacy_siblings_page_protected(
        &self,
        pool: &PgPool,
        claim: &ProtectedClaimEpoch,
        capture: Uuid,
        worker: &WorkerId,
        parent: &str,
        after: Option<&CustodyLegacyCursor>,
        directories_only: bool,
        descending: bool,
        now: OffsetDateTime,
    ) -> Result<Vec<CustodyLegacyEntry>, WorkStoreError> {
        self.custody_legacy_siblings_page_fenced(
            pool,
            capture,
            claim.item_id,
            worker,
            parent,
            after,
            directories_only,
            descending,
            now,
            Some(claim),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn custody_legacy_siblings_page_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        parent: &str,
        after: Option<&CustodyLegacyCursor>,
        directories_only: bool,
        descending: bool,
        now: OffsetDateTime,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<Vec<CustodyLegacyEntry>, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_spool_lease(&mut tx, item, worker, now, None, protected).await?;
        lock_spool_token(&mut tx, capture, item, None, protected).await?;
        lock_capture(&mut tx, capture, item, worker, "frozen").await?;
        let rows: Vec<(i64,String,String,String,bool,i64,String)> = sqlx::query_as(
            "SELECT ordinal,path_key,name_key,legacy_path_hex,is_directory,legacy_length,digest FROM custody_hash_spool_entries WHERE capture_id=$1 AND ingest_occurrence_hash(parent_key)=ingest_occurrence_hash($2) AND parent_key COLLATE \"C\"=$2 COLLATE \"C\" AND (NOT $5 OR is_directory) AND ($3::text IS NULL OR (NOT $6 AND (left(name_key,512),name_key,ordinal)>(left($3::text,512) COLLATE \"C\",$3::text COLLATE \"C\",$4::bigint)) OR ($6 AND (left(name_key,512),name_key,ordinal)<(left($3::text,512) COLLATE \"C\",$3::text COLLATE \"C\",$4::bigint))) ORDER BY CASE WHEN NOT $6 THEN left(name_key,512) END ASC, CASE WHEN NOT $6 THEN name_key END ASC, CASE WHEN NOT $6 THEN ordinal END ASC, CASE WHEN $6 THEN left(name_key,512) END DESC, CASE WHEN $6 THEN name_key END DESC, CASE WHEN $6 THEN ordinal END DESC LIMIT 16",
        ).bind(capture).bind(parent).bind(after.map(|value| &value.name_key)).bind(after.map(|value| value.ordinal))
            .bind(directories_only).bind(descending).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    ordinal,
                    path_key,
                    name_key,
                    legacy_path_hex,
                    is_directory,
                    legacy_length,
                    digest,
                )| CustodyLegacyEntry {
                    ordinal,
                    path_key,
                    name_key,
                    legacy_path_hex,
                    is_directory,
                    legacy_length,
                    digest,
                },
            )
            .collect())
    }

    /// Hash every frozen page in deterministic order, then seal only if the
    /// same owner, epoch, purpose and row count still hold under the lease.
    /// This certifies spool contents, not that a filesystem traversal was complete.
    pub async fn seal_custody_hash_spool(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
    ) -> Result<SealedCustodyHash, WorkStoreError> {
        self.seal_custody_hash_spool_fenced(pool, capture, item, worker, None, None)
            .await
    }

    pub async fn seal_custody_hash_spool_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
    ) -> Result<SealedCustodyHash, WorkStoreError> {
        self.seal_custody_hash_spool_fenced(pool, capture, item, worker, Some(retained_token), None)
            .await
    }

    async fn seal_custody_hash_spool_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        retained_token: Option<Uuid>,
        protected: Option<&ProtectedClaimEpoch>,
    ) -> Result<SealedCustodyHash, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_spool_lease(
            &mut tx,
            item,
            worker,
            OffsetDateTime::now_utc(),
            retained_token,
            protected,
        )
        .await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        lock_capture(&mut tx, capture, item, worker, "frozen").await?;
        let (purpose, generation, target_binding, epoch, expected): (String, String, String, Uuid, i64) = sqlx::query_as(
            "SELECT purpose,source_generation,target_binding,observation_epoch,entry_count FROM custody_hash_spool_captures WHERE id=$1",
        ).bind(capture).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        let purpose = match purpose.as_str() {
            "source" => CustodyHashPurpose::Source,
            "staging" => CustodyHashPurpose::Staging,
            "retained" => CustodyHashPurpose::Retained,
            _ => {
                return Err(WorkStoreError::CorruptState(
                    "unknown custody hash purpose".to_owned(),
                ));
            }
        };
        let mut hash = blake3::Hasher::new();
        hash.update(b"melodee-custody-tree-v3\0");
        let mut cursor: Option<CustodyHashCursor> = None;
        let mut count = 0_i64;
        loop {
            let page = self
                .custody_hash_spool_page_fenced(
                    pool,
                    capture,
                    item,
                    worker,
                    cursor.as_ref(),
                    OffsetDateTime::now_utc(),
                    retained_token,
                    protected,
                )
                .await?;
            if page.is_empty() {
                break;
            }
            for entry in &page {
                hash.update(entry.digest.as_bytes());
                count = count.checked_add(1).ok_or(WorkStoreError::InvalidRequest(
                    "custody hash count overflow",
                ))?;
            }
            let last = page
                .last()
                .ok_or(WorkStoreError::InvalidRequest("empty custody hash page"))?;
            cursor = Some(CustodyHashCursor {
                digest: last.digest.clone(),
                ordinal: last.ordinal,
            });
        }
        if count != expected {
            return Err(WorkStoreError::InvalidRequest("custody hash count changed"));
        }
        hash.update(&(count as u64).to_le_bytes());
        let digest = format!("v3:{}", hash.finalize().to_hex());
        let mut tx = pool.begin().await?;
        lock_spool_lease(
            &mut tx,
            item,
            worker,
            OffsetDateTime::now_utc(),
            retained_token,
            protected,
        )
        .await?;
        lock_spool_token(&mut tx, capture, item, retained_token, protected).await?;
        lock_capture(&mut tx, capture, item, worker, "frozen").await?;
        let updated = sqlx::query("UPDATE custody_hash_spool_captures SET sealed_digest=$2,sealed_epoch=$3 WHERE id=$1 AND observation_epoch=$3 AND entry_count=$4 AND purpose=$5 AND source_generation=$6")
            .bind(capture).bind(&digest).bind(epoch).bind(count).bind(purpose.as_str()).bind(&generation).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash seal binding changed",
            ));
        }
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, protected).await?;
        tx.commit().await?;
        Ok(SealedCustodyHash {
            capture_id: capture,
            item_id: item,
            owner: worker.as_str().to_owned(),
            purpose,
            retained_lease_token: retained_token,
            protected_claim_epoch: protected.map(|claim| claim.epoch),
            generation,
            target_binding,
            epoch,
            digest,
            entries: count,
        })
    }

    /// Start or resume bounded disposal under the current work-item lease.
    /// This permits a new owner to clean abandoned provisional evidence but
    /// never adopts it as verified content.
    pub async fn discard_custody_hash_spool_page(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<bool, WorkStoreError> {
        self.discard_custody_hash_spool_page_fenced(pool, capture, item, worker, now, None)
            .await
    }

    pub async fn discard_custody_hash_spool_page_retained(
        &self,
        retained_token: Uuid,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
    ) -> Result<bool, WorkStoreError> {
        self.discard_custody_hash_spool_page_fenced(
            pool,
            capture,
            item,
            worker,
            now,
            Some(retained_token),
        )
        .await
    }

    async fn discard_custody_hash_spool_page_fenced(
        &self,
        pool: &PgPool,
        capture: Uuid,
        item: Uuid,
        worker: &WorkerId,
        now: OffsetDateTime,
        retained_token: Option<Uuid>,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_budget(&mut tx).await?;
        lock_spool_lease(&mut tx, item, worker, now, retained_token, None).await?;
        // A new retained lease may discard an abandoned predecessor's token,
        // but never processing evidence. Adoption requires a fresh capture.
        let exists: Option<bool> = sqlx::query_scalar("SELECT true FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND (($3::uuid IS NULL AND purpose<>'retained') OR ($3::uuid IS NOT NULL AND purpose='retained')) FOR UPDATE")
            .bind(capture).bind(item).bind(retained_token).fetch_optional(&mut *tx).await?;
        if exists.is_none() {
            return Err(WorkStoreError::InvalidRequest(
                "custody hash capture binding changed",
            ));
        }
        sqlx::query("UPDATE custody_hash_spool_captures SET state='discarding',lease_owner=$2,updated_at=$3 WHERE id=$1")
            .bind(capture).bind(worker.as_str()).bind(now).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM custody_hash_spool_entries WHERE (capture_id,ordinal) IN (SELECT capture_id,ordinal FROM custody_hash_spool_entries WHERE capture_id=$1 ORDER BY ordinal LIMIT 128)")
            .bind(capture).execute(&mut *tx).await?;
        let deleted =
            sqlx::query("DELETE FROM custody_hash_spool_captures WHERE id=$1 AND entry_count=0")
                .bind(capture)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
        ensure_spool_lease_live(&mut tx, item, worker, retained_token, None).await?;
        tx.commit().await?;
        Ok(deleted)
    }

    /// Drain one page from an orphaned or expired capture. The work-item row
    /// is locked and its lease rechecked before any evidence is discarded.
    pub async fn cleanup_abandoned_custody_hash_spool_page(
        &self,
        pool: &PgPool,
        now: OffsetDateTime,
    ) -> Result<bool, WorkStoreError> {
        let mut tx = pool.begin().await?;
        lock_budget(&mut tx).await?;
        let candidate: Option<(Uuid, Uuid, String)> = sqlx::query_as("SELECT c.id,c.work_item_id,c.purpose FROM custody_hash_spool_captures c WHERE (c.purpose='retained' AND NOT EXISTS (SELECT 1 FROM inbound_retained_sources r WHERE r.work_item_id=c.work_item_id AND r.status='active' AND r.lease_owner=c.lease_owner AND r.lease_token=c.retained_lease_token AND r.lease_expires_at>clock_timestamp())) OR (c.purpose<>'retained' AND NOT EXISTS (SELECT 1 FROM release_work_items w WHERE w.id=c.work_item_id AND w.lease_owner=c.lease_owner AND w.lease_expires_at>clock_timestamp())) ORDER BY c.updated_at,c.id LIMIT 1")
            .fetch_optional(&mut *tx).await?;
        let Some((capture, item, purpose)) = candidate else {
            tx.commit().await?;
            return Ok(false);
        };
        let lease: Option<(Option<String>, Option<Uuid>, Option<OffsetDateTime>, bool)> = if purpose
            == "retained"
        {
            sqlx::query_as("SELECT lease_owner,lease_token,lease_expires_at,status='active' FROM inbound_retained_sources WHERE work_item_id=$1 FOR UPDATE")
                .bind(item).fetch_optional(&mut *tx).await?
        } else {
            sqlx::query_as("SELECT lease_owner,NULL::uuid,lease_expires_at,true FROM release_work_items WHERE id=$1 FOR UPDATE")
                .bind(item).fetch_optional(&mut *tx).await?
        };
        let owner: Option<(String, Option<Uuid>)> = sqlx::query_as("SELECT lease_owner,retained_lease_token FROM custody_hash_spool_captures WHERE id=$1 AND work_item_id=$2 AND purpose=$3 FOR UPDATE")
            .bind(capture).bind(item).bind(purpose).fetch_optional(&mut *tx).await?;
        let Some((owner, token)) = owner else {
            tx.commit().await?;
            return Ok(false);
        };
        let db_now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        if lease
            .as_ref()
            .is_some_and(|(worker, lease_token, expires, active)| {
                *active
                    && *lease_token == token
                    && worker.as_deref() == Some(owner.as_str())
                    && expires.is_some_and(|time| time > db_now)
            })
        {
            tx.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE custody_hash_spool_captures SET state='discarding',updated_at=$2 WHERE id=$1",
        )
        .bind(capture)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM custody_hash_spool_entries WHERE (capture_id,ordinal) IN (SELECT capture_id,ordinal FROM custody_hash_spool_entries WHERE capture_id=$1 ORDER BY ordinal LIMIT 128)")
            .bind(capture).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM custody_hash_spool_captures WHERE id=$1 AND entry_count=0")
            .bind(capture)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}
