//! Persistent installation identity shared by Jellyfin discovery and login.

/// Loads the single migration-created identity. A missing row is a broken
/// installation invariant and fails startup, rather than changing client IDs.
pub async fn installation_id(pool: &sqlx::PgPool) -> Result<uuid::Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT installation_id FROM server_installation_identity WHERE singleton = true",
    )
    .fetch_one(pool)
    .await
}
