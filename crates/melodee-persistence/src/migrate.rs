//! Migration runner. Migrations are explicit, source-controlled SQL under
//! `crates/melodee-persistence/migrations/`, embedded at compile time and
//! runnable from an empty database. Phase-specific additive indexes are kept
//! here as migrations as well, including serving projections and predicate-
//! aligned indexes, so an upgraded catalog receives the same query-plan
//! guarantees as a freshly created database.

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

/// Migration-currency evidence for the onboarding preflight (PRD-SETUP-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Highest applied migration version (0 when the tracking table is
    /// missing or empty).
    pub applied: i64,
    /// Highest migration version compiled into this binary.
    pub latest_available: i64,
    /// No applied migration row recorded a failure.
    pub all_successful: bool,
}

impl MigrationStatus {
    /// Current means: every compiled migration is applied and none failed.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.applied >= self.latest_available && self.all_successful
    }
}

/// Reads the sqlx migration tracking state without applying anything.
///
/// # Errors
/// Returns the database error; a missing `_sqlx_migrations` table is not an
/// error but reports `applied = 0` (never migrated).
pub async fn migration_status(pool: &PgPool) -> Result<MigrationStatus, sqlx::Error> {
    let latest_available = MIGRATOR
        .migrations
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0);
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !table_exists {
        return Ok(MigrationStatus {
            applied: 0,
            latest_available,
            all_successful: false,
        });
    }
    let row: (Option<i64>, i64) = sqlx::query_as(
        "SELECT MAX(version), COUNT(*) FILTER (WHERE NOT success) FROM _sqlx_migrations",
    )
    .fetch_one(pool)
    .await?;
    Ok(MigrationStatus {
        applied: row.0.unwrap_or(0),
        latest_available,
        all_successful: row.1 == 0 && row.0.is_some(),
    })
}
