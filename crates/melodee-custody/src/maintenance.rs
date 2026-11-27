//! Database epoch observations, not filesystem or completion authority.

use sqlx::PgPool;
use uuid::Uuid;

/// Bearer credentials for observing one maintenance reservation. This type
/// deliberately has no Debug, Clone, serialization, or token accessors.
pub struct MaintenanceCredentials {
    owner: String,
    reservation_id: Uuid,
    reservation_token: Uuid,
    maintenance_token: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum MaintenanceObservationError {
    #[error("invalid or expired maintenance observation")]
    Invalid,
    #[error("maintenance observation query failed")]
    Database(#[from] sqlx::Error),
}

impl MaintenanceCredentials {
    /// Construct a bearer, without granting authority. The database must
    /// independently validate every value against both live original epochs.
    #[doc(hidden)]
    pub fn from_parts(
        owner: String,
        reservation_id: Uuid,
        reservation_token: Uuid,
        maintenance_token: Uuid,
    ) -> Result<Self, MaintenanceObservationError> {
        if owner.trim().is_empty()
            || owner.len() > 512
            || reservation_id.is_nil()
            || reservation_token.is_nil()
            || maintenance_token.is_nil()
        {
            return Err(MaintenanceObservationError::Invalid);
        }
        Ok(Self {
            owner,
            reservation_id,
            reservation_token,
            maintenance_token,
        })
    }

    /// Observe whether an intent's recorded original tokens still match both
    /// live DB leases and its frozen snapshot/root-set revisions. This read
    /// does not lock rows, verify target membership or authorize filesystem work, and its
    /// result is never accepted as proof of acknowledgement or deletion.
    pub async fn observe_original_epochs(
        &self,
        pool: &PgPool,
        intent_id: Uuid,
    ) -> Result<(), MaintenanceObservationError> {
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM admin_media_page_intents i \
             JOIN admin_media_reservations r ON r.id=i.reservation_id \
             JOIN admin_media_snapshots s ON s.reservation_id=i.reservation_id \
             JOIN admin_media_root_sets roots ON roots.revision=i.root_set_revision AND roots.reservation_id=i.reservation_id \
             JOIN work_item_lifecycle_maintenance m ON m.singleton \
             WHERE i.id=$1 AND i.reservation_id=$2 AND i.state='intent' \
               AND i.reservation_token=$3 AND r.token=$3 AND r.owner=$5 \
               AND r.lease_expires_at>clock_timestamp() \
               AND i.maintenance_token=$4 AND m.token=$4 AND m.owner=$5 \
               AND roots.maintenance_token=$4 AND roots.reservation_token=$3 \
               AND m.quiesced AND m.lease_expires_at>clock_timestamp() \
               AND s.state='frozen' AND s.revision=i.snapshot_revision AND s.root_set_revision=roots.revision \
               AND roots.entry_count=(SELECT count(*) FROM admin_media_root_entries e WHERE e.revision=roots.revision) \
               AND i.entry_count=(SELECT count(*) FROM admin_media_page_intent_entries e WHERE e.intent_id=i.id))"
        ).bind(intent_id).bind(self.reservation_id).bind(self.reservation_token).bind(self.maintenance_token).bind(&self.owner).fetch_one(pool).await?;
        if !current {
            return Err(MaintenanceObservationError::Invalid);
        }
        Ok(())
    }
}
