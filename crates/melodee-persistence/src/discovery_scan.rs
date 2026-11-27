//! Fenced ownership of a durable inbound-source discovery generation.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::WorkStoreError;

/// Identifies one acquired scan lease. Every discovery mutation must hold
/// the corresponding row lock until its transaction commits.
#[derive(Clone, Debug)]
pub struct DiscoveryScanFence {
    pub scope_key: String,
    pub generation: Uuid,
    pub owner: Uuid,
}

impl DiscoveryScanFence {
    /// Locks and validates ownership inside the caller's transaction.
    pub async fn lock(&self, tx: &mut Transaction<'_, Postgres>) -> Result<(), WorkStoreError> {
        lock_fence(tx, Some(self)).await
    }
}

pub(crate) async fn lock_fence(
    tx: &mut Transaction<'_, Postgres>,
    fence: Option<&DiscoveryScanFence>,
) -> Result<(), WorkStoreError> {
    if let Some(fence) = fence {
        let valid = sqlx::query_scalar::<_, bool>(
            "SELECT true FROM ingestion_discovery_scan
             WHERE scope_key = $1 AND generation = $2 AND owner = $3
               AND lease_expires_at > clock_timestamp()
             FOR UPDATE",
        )
        .bind(&fence.scope_key)
        .bind(fence.generation)
        .bind(fence.owner)
        .fetch_optional(&mut **tx)
        .await?;
        if valid.is_none() {
            return Err(WorkStoreError::DiscoveryScanLeaseExpired);
        }
    }
    Ok(())
}
