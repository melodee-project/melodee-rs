//! Read-only database evidence for first-run onboarding (PRD-SETUP-001/002).
//!
//! The application layer decides which checks are required and how uncertain
//! evidence affects onboarding. This store preserves database errors for that
//! decision instead of converting them into a successful preflight result.

use sqlx::PgPool;

use crate::KNOWN_SETTINGS;

/// Queries the independent database evidence used by onboarding preflight.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnboardingPreflightStore;

impl OnboardingPreflightStore {
    /// Whether an enabled, unlocked administrator exists.
    pub async fn has_enabled_administrator(&self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM native_users \
             WHERE is_admin AND enabled AND NOT locked)",
        )
        .fetch_one(pool)
        .await
    }

    /// Whether catalog identities provide evidence of an existing installation.
    pub async fn has_catalog_entities(&self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM releases) OR EXISTS (SELECT 1 FROM artists)",
        )
        .fetch_one(pool)
        .await
    }

    /// Whether at least one persisted setting belongs to the known registry.
    pub async fn has_known_server_setting(&self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        let known_keys: Vec<&str> = KNOWN_SETTINGS.iter().map(|known| known.key).collect();
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM server_settings WHERE key = ANY($1))")
            .bind(&known_keys)
            .fetch_one(pool)
            .await
    }
}
