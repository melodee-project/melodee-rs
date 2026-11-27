//! Fenced durable intent and evidence for inbound custody handoffs.

use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{WorkItemStore, WorkStoreError, WorkerId, work_items};

/// Persisted custody evidence. A row alone never authorizes deleting media.
#[derive(Debug, Clone, FromRow)]
pub struct InboundCustodyClaim {
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: String,
    pub custody_name: String,
    pub source_root_generation: String,
    pub source_generation: String,
    pub preclaim_tree_hash: String,
    pub preclaim_legacy_tree_hash: String,
    pub status: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Evidence captured before a no-replace custody attempt.
pub struct InboundCustodyIntent<'a> {
    pub work_item_id: Uuid,
    pub source_library_id: Option<Uuid>,
    pub source_path: &'a str,
    pub source_root_generation: &'a str,
    pub source_generation: &'a str,
    pub preclaim_tree_hash: &'a str,
    pub preclaim_legacy_tree_hash: &'a str,
    pub now: OffsetDateTime,
}

async fn ensure_db_lease_live(
    tx: &mut Transaction<'_, Postgres>,
    item: &melodee_domain::ReleaseWorkItem,
    worker: &WorkerId,
) -> Result<(), WorkStoreError> {
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    work_items::ensure_active_lease(item, worker, now)
}

impl WorkItemStore {
    /// Read a pending or completed custody record for crash reconciliation.
    pub async fn inbound_custody(
        &self,
        pool: &PgPool,
        work_item_id: Uuid,
    ) -> Result<Option<InboundCustodyClaim>, WorkStoreError> {
        Ok(sqlx::query_as::<_, InboundCustodyClaim>(
            "SELECT * FROM inbound_custody_claims WHERE work_item_id=$1",
        )
        .bind(work_item_id)
        .fetch_optional(pool)
        .await?)
    }

    /// Commit intent before moving any source path. A retry must supply the
    /// same occurrence and content evidence, or fails without changing it.
    pub async fn begin_inbound_custody(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        intent: &InboundCustodyIntent<'_>,
    ) -> Result<InboundCustodyClaim, WorkStoreError> {
        if intent.source_root_generation.is_empty()
            || intent.source_generation.is_empty()
            || intent.preclaim_tree_hash.len() != 67
            || !intent.preclaim_tree_hash.starts_with("v2:")
            || !intent.preclaim_tree_hash.as_bytes()[3..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            || intent.preclaim_legacy_tree_hash.len() != 64
            || !intent
                .preclaim_legacy_tree_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(WorkStoreError::InvalidRequest(
                "invalid inbound custody evidence",
            ));
        }
        let mut tx = pool.begin().await?;
        let item = work_items::lock_for_update(&mut tx, intent.work_item_id).await?;
        work_items::ensure_active_lease(&item, worker, intent.now)?;
        work_items::ensure_ordinary_custody(&mut tx, intent.work_item_id).await?;
        ensure_db_lease_live(&mut tx, &item, worker).await?;
        let inbound_active: bool =
            sqlx::query_scalar("SELECT inbound_active FROM release_work_items WHERE id=$1")
                .bind(item.id)
                .fetch_one(&mut *tx)
                .await?;
        if !inbound_active
            || !matches!(
                item.state,
                melodee_domain::IngestionState::Ready | melodee_domain::IngestionState::Inspecting
            )
        {
            return Err(WorkStoreError::InvalidRequest(
                "work item is not an active inbound occurrence",
            ));
        }
        if !work_items::inbound_path_is_unambiguous(&mut tx, item.id).await? {
            return Err(WorkStoreError::InvalidRequest(
                "legacy inbound path is ambiguous and requires operator review",
            ));
        }
        if item.source_library_id != intent.source_library_id
            || item.source_path != intent.source_path
        {
            return Err(WorkStoreError::InvalidRequest(
                "inbound custody binding changed",
            ));
        }
        let custody_name = format!(".melodee-custody-{}", item.id);
        sqlx::query("INSERT INTO inbound_custody_claims (work_item_id,source_library_id,source_path,custody_name,source_root_generation,source_generation,preclaim_tree_hash,preclaim_legacy_tree_hash,status,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'intent',$9,$9) ON CONFLICT (work_item_id) DO NOTHING")
            .bind(item.id).bind(intent.source_library_id).bind(intent.source_path).bind(&custody_name)
            .bind(intent.source_root_generation).bind(intent.source_generation).bind(intent.preclaim_tree_hash).bind(intent.preclaim_legacy_tree_hash).bind(intent.now).execute(&mut *tx).await?;
        let claim = sqlx::query_as::<_, InboundCustodyClaim>(
            "SELECT * FROM inbound_custody_claims WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(item.id)
        .fetch_one(&mut *tx)
        .await?;
        if claim.source_library_id != intent.source_library_id
            || claim.source_path != intent.source_path
            || claim.custody_name != custody_name
            || claim.source_root_generation != intent.source_root_generation
            || claim.source_generation != intent.source_generation
            || claim.preclaim_tree_hash != intent.preclaim_tree_hash
            || claim.preclaim_legacy_tree_hash != intent.preclaim_legacy_tree_hash
        {
            return Err(WorkStoreError::InvalidRequest(
                "inbound custody intent conflicts with existing evidence",
            ));
        }
        ensure_db_lease_live(&mut tx, &item, worker).await?;
        tx.commit().await?;
        Ok(claim)
    }

    /// Advance one verified custody state while preserving the processing lease.
    pub async fn advance_inbound_custody(
        &self,
        pool: &PgPool,
        worker: &WorkerId,
        work_item_id: Uuid,
        next: &str,
        observed_root_generation: &str,
        observed_generation: &str,
        observed_tree_hash: &str,
        now: OffsetDateTime,
    ) -> Result<InboundCustodyClaim, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = work_items::lock_for_update(&mut tx, work_item_id).await?;
        work_items::ensure_active_lease(&item, worker, now)?;
        work_items::ensure_ordinary_custody(&mut tx, work_item_id).await?;
        ensure_db_lease_live(&mut tx, &item, worker).await?;
        let mut claim = sqlx::query_as::<_, InboundCustodyClaim>(
            "SELECT * FROM inbound_custody_claims WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(work_item_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::NotFound(work_item_id))?;
        if claim.source_library_id != item.source_library_id
            || claim.source_path != item.source_path
        {
            return Err(WorkStoreError::InvalidRequest(
                "inbound custody binding changed",
            ));
        }
        // The recovery protocol does not yet persist an independently
        // verified replacement observation. Keep intent replayable instead of
        // accepting a caller assertion as a terminal filesystem outcome.
        if matches!(next, "replacement_preserved" | "replacement_restored") {
            return Err(WorkStoreError::InvalidRequest(
                "replacement recovery requires filesystem re-observation",
            ));
        }
        if matches!(next, "held" | "staged" | "cleaned")
            && (claim.source_root_generation != observed_root_generation
                || claim.source_generation != observed_generation
                || claim.preclaim_tree_hash != observed_tree_hash)
        {
            return Err(WorkStoreError::InvalidRequest(
                "inbound custody proof mismatch",
            ));
        }
        if next == "staged" {
            let receipt_ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM release_work_item_staging_handoffs WHERE work_item_id=$1 AND status IN ('staged','inbound_removed') AND tree_hash=$2)")
                .bind(work_item_id).bind(&claim.preclaim_legacy_tree_hash).fetch_one(&mut *tx).await?;
            if !receipt_ok {
                return Err(WorkStoreError::InvalidRequest(
                    "inbound custody lacks matching staging receipt",
                ));
            }
        }
        if claim.status == next {
            ensure_db_lease_live(&mut tx, &item, worker).await?;
            tx.commit().await?;
            return Ok(claim);
        }
        let valid = matches!(
            (claim.status.as_str(), next),
            (
                "intent",
                "held" | "replacement_preserved" | "replacement_restored"
            ) | ("held", "staged")
                | ("staged", "cleaned")
        );
        if !valid {
            return Err(WorkStoreError::InvalidRequest(
                "invalid inbound custody transition",
            ));
        }
        sqlx::query(
            "UPDATE inbound_custody_claims SET status=$2,updated_at=$3 WHERE work_item_id=$1",
        )
        .bind(work_item_id)
        .bind(next)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        claim.status = next.to_owned();
        claim.updated_at = now;
        ensure_db_lease_live(&mut tx, &item, worker).await?;
        tx.commit().await?;
        Ok(claim)
    }

    /// Scanner exclusion applies only to an actual pending durable claim.
    pub async fn is_pending_custody_name(
        &self,
        pool: &PgPool,
        source: Option<Uuid>,
        name: &str,
    ) -> Result<bool, WorkStoreError> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inbound_custody_claims WHERE source_library_id IS NOT DISTINCT FROM $1 AND custody_name=$2 AND status<>'cleaned')")
            .bind(source).bind(name).fetch_one(pool).await?)
    }
}
