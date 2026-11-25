//! Durable operator authorization for deleting a Release before catalog import.
use melodee_domain::{IngestionState, ReleaseWorkItem, TransitionKind, WorkEventType};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    WorkItemStore, WorkStoreError,
    work_items::{insert_event, lock_for_update},
};

#[derive(sqlx::FromRow)]
pub struct ReviewDeletionAuthorization {
    pub staging_path: Option<String>,
    pub canonical_path: Option<String>,
}

impl WorkItemStore {
    /// Commit the identity, note and exact managed paths before removing any bytes.
    pub async fn authorize_unpublished_delete(
        &self,
        pool: &PgPool,
        id: Uuid,
        actor: Uuid,
        role: &str,
        note: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReviewDeletionAuthorization, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if item.state != IngestionState::NeedsReview {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        if item.has_active_lease(now) {
            return Err(WorkStoreError::LeaseConflict(id));
        }
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_imports WHERE work_item_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(WorkStoreError::Cataloged(id));
        }
        sqlx::query(
            "INSERT INTO review_deletion_authorizations
                (work_item_id, actor_id, actor_role, note, staging_path, canonical_path, authorized_at)
             VALUES ($1, $2, $3, $4,
                (SELECT staging_relative_path FROM release_work_item_staging_handoffs WHERE work_item_id = $1),
                (SELECT p.target_relative_path FROM release_publication_plans p
                 JOIN release_publication_receipts r USING (work_item_id) WHERE p.work_item_id = $1),
                $5)
             ON CONFLICT (work_item_id) DO NOTHING",
        )
        .bind(id)
        .bind(actor)
        .bind(role)
        .bind(note)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let paths = sqlx::query_as(
            "SELECT staging_path, canonical_path FROM review_deletion_authorizations WHERE work_item_id = $1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(paths)
    }

    /// A repeated completion returns the original tombstone without adding another event.
    pub async fn complete_unpublished_delete(
        &self,
        pool: &PgPool,
        id: Uuid,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let item = lock_for_update(&mut tx, id).await?;
        if item.state == IngestionState::Deleted {
            return Ok(item);
        }
        item.state
            .ensure_transition(IngestionState::Deleted, TransitionKind::Operator)?;
        let audit: serde_json::Value = sqlx::query_scalar(
            "UPDATE review_deletion_authorizations AS a SET completed_at=$2
            WHERE work_item_id=$1 AND completed_at IS NULL RETURNING to_jsonb(a)",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(WorkStoreError::InvalidState(id, item.state))?;
        // Only a staged occurrence still awaiting inbound cleanup needs generation-based
        // suppression. A historical name must be available to future arrivals.
        sqlx::query(
            "UPDATE release_work_items SET state = 'deleted', state_reason = 'review.delete_completed',
                updated_at = $2, completed_at = $2, next_attempt_at = NULL,
                inbound_active = inbound_active AND EXISTS (
                    SELECT 1 FROM release_work_item_staging_handoffs
                    WHERE work_item_id = $1 AND status = 'staged' AND inbound_removed_at IS NULL
                ),
                lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL WHERE id = $1",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_event(
            &mut tx,
            id,
            WorkEventType::Transitioned,
            Some(item.state),
            Some(IngestionState::Deleted),
            Some("review.delete_completed"),
            None,
            item.correlation_id,
            now,
            Some(audit),
        )
        .await?;
        let updated = lock_for_update(&mut tx, id).await?;
        tx.commit().await?;
        Ok(updated)
    }
}
