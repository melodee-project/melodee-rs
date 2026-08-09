//! Migration runner. Migrations are explicit, source-controlled SQL under
//! `crates/melodee-persistence/migrations/`, embedded at compile time and
//! runnable from an empty database.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies all pending migrations. Idempotent (sqlx tracks applied versions).
///
/// # Errors
/// Returns the migration error describing the failed migration.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
