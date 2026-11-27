//! Durable operator authorization for deleting a Release before catalog import.
use melodee_domain::{IngestionState, ReleaseWorkItem, TransitionKind, WorkEventType};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    AdminMediaReservation, WorkItemStore, WorkStoreError,
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
        crate::work_items::ensure_ordinary_custody(&mut tx, id).await?;
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
            "SELECT staging_path, canonical_path FROM review_deletion_authorizations WHERE work_item_id = $1 AND admin_reservation_id IS NULL",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(paths)
    }

    /// Bind a single-item administrative reservation to the exact stored
    /// review paths. This DB intent is not permission to touch files; the
    /// private caller must hold and revalidate the item lifecycle lock.
    #[allow(clippy::too_many_arguments)]
    pub async fn authorize_unpublished_delete_reserved(
        &self,
        pool: &PgPool,
        id: Uuid,
        reservation: &AdminMediaReservation,
        actor: Uuid,
        role: &str,
        note: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ReviewDeletionAuthorization, WorkStoreError> {
        let mut tx = pool.begin().await?;
        let expiry =
            crate::admin_media_reservation::lock_review_reservation(&mut tx, reservation, id)
                .await?;
        let item = lock_for_update(&mut tx, id).await?;
        let allowed:bool=sqlx::query_scalar("SELECT NOT protected_custody_required AND NOT COALESCE(lease_expires_at>clock_timestamp(),false) AND NOT EXISTS(SELECT 1 FROM catalog_imports WHERE work_item_id=$1) AND NOT EXISTS(SELECT 1 FROM inbound_retained_sources WHERE work_item_id=$1 AND status='active') FROM release_work_items WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
        if !allowed || item.state != IngestionState::NeedsReview {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        sqlx::query("INSERT INTO review_deletion_authorizations(work_item_id,actor_id,actor_role,note,staging_path,canonical_path,authorized_at,admin_reservation_id,admin_reservation_token,admin_reservation_owner) VALUES($1,$2,$3,$4,(SELECT staging_relative_path FROM release_work_item_staging_handoffs WHERE work_item_id=$1),(SELECT p.target_relative_path FROM release_publication_plans p JOIN release_publication_receipts r USING(work_item_id) WHERE p.work_item_id=$1),$5,$6,$7,$8) ON CONFLICT(work_item_id) DO UPDATE SET admin_reservation_token=EXCLUDED.admin_reservation_token,admin_reservation_owner=EXCLUDED.admin_reservation_owner WHERE review_deletion_authorizations.admin_reservation_id=EXCLUDED.admin_reservation_id AND review_deletion_authorizations.completed_at IS NULL")
            .bind(id).bind(actor).bind(role).bind(note).bind(now).bind(reservation.id).bind(reservation.token).bind(&reservation.owner).execute(&mut *tx).await?;
        let paths:Option<ReviewDeletionAuthorization>=sqlx::query_as("SELECT staging_path,canonical_path FROM review_deletion_authorizations WHERE work_item_id=$1 AND admin_reservation_id=$2 AND admin_reservation_token=$3 AND admin_reservation_owner=$4 AND completed_at IS NULL FOR UPDATE")
            .bind(id).bind(reservation.id).bind(reservation.token).bind(&reservation.owner).fetch_optional(&mut *tx).await?;
        let paths = paths.ok_or(WorkStoreError::InvalidRequest(
            "review reservation binding changed",
        ))?;
        crate::admin_media_reservation::check_expiry(&mut tx, expiry).await?;
        tx.commit().await?;
        Ok(paths)
    }

    /// Atomically record the deleted state and consume the matching reservation.
    /// Historical replay is accepted only for the exact original token.
    pub async fn complete_unpublished_delete_reserved(
        &self,
        pool: &PgPool,
        id: Uuid,
        reservation: &AdminMediaReservation,
        now: OffsetDateTime,
    ) -> Result<ReleaseWorkItem, WorkStoreError> {
        let mut tx = pool.begin().await?;
        // Fast historical replay does not authorize filesystem work.
        let replay:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM review_deletion_authorizations a JOIN release_work_items w ON w.id=a.work_item_id WHERE a.work_item_id=$1 AND a.admin_reservation_id=$2 AND a.admin_reservation_token=$3 AND a.admin_reservation_owner=$4 AND a.completed_at IS NOT NULL AND w.state='deleted')")
            .bind(id).bind(reservation.id).bind(reservation.token).bind(&reservation.owner).fetch_one(&mut *tx).await?;
        if replay {
            let item = lock_for_update(&mut tx, id).await?;
            tx.commit().await?;
            return Ok(item);
        }
        let expiry =
            crate::admin_media_reservation::lock_review_reservation(&mut tx, reservation, id)
                .await?;
        let item = lock_for_update(&mut tx, id).await?;
        let allowed:bool=sqlx::query_scalar("SELECT NOT protected_custody_required AND NOT COALESCE(lease_expires_at>clock_timestamp(),false) AND NOT EXISTS(SELECT 1 FROM catalog_imports WHERE work_item_id=$1) AND NOT EXISTS(SELECT 1 FROM inbound_retained_sources WHERE work_item_id=$1 AND status='active') FROM release_work_items WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(WorkStoreError::InvalidState(id, item.state));
        }
        item.state
            .ensure_transition(IngestionState::Deleted, TransitionKind::Operator)?;
        let audit:serde_json::Value=sqlx::query_scalar("UPDATE review_deletion_authorizations AS a SET completed_at=$5 WHERE work_item_id=$1 AND admin_reservation_id=$2 AND admin_reservation_token=$3 AND admin_reservation_owner=$4 AND completed_at IS NULL RETURNING to_jsonb(a)-'admin_reservation_token'")
            .bind(id).bind(reservation.id).bind(reservation.token).bind(&reservation.owner).bind(now).fetch_optional(&mut *tx).await?.ok_or(WorkStoreError::InvalidState(id,item.state))?;
        sqlx::query("UPDATE release_work_items SET state='deleted',state_reason='review.delete_completed',updated_at=$2,completed_at=$2,next_attempt_at=NULL,inbound_active=inbound_active AND EXISTS(SELECT 1 FROM release_work_item_staging_handoffs WHERE work_item_id=$1 AND status='staged' AND inbound_removed_at IS NULL),lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL WHERE id=$1").bind(id).bind(now).execute(&mut *tx).await?;
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
        crate::admin_media_reservation::consume_review_reservation(&mut tx, reservation, expiry)
            .await?;
        tx.commit().await?;
        Ok(updated)
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
        crate::work_items::ensure_ordinary_custody(&mut tx, id).await?;
        if item.state == IngestionState::Deleted {
            return Ok(item);
        }
        item.state
            .ensure_transition(IngestionState::Deleted, TransitionKind::Operator)?;
        let audit: serde_json::Value = sqlx::query_scalar(
            "UPDATE review_deletion_authorizations AS a SET completed_at=$2
            WHERE work_item_id=$1 AND admin_reservation_id IS NULL AND completed_at IS NULL RETURNING to_jsonb(a)",
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
