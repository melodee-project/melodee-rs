//! Shared integration-test support: per-test PostgreSQL databases.
//!
//! Each test gets a fresh database (`melodee_test_<name>_<uuid>`) created from
//! the admin connection, migrated from empty, and dropped on completion.
//! Connection: `MELODEE_TEST_DATABASE_URL` (admin database), defaulting to the
//! documented dev Postgres (`postgres://melodee:melodee-dev@127.0.0.1:5433/postgres`).

// Test-support module: shared helpers are compiled per test binary (some
// items unused in some binaries), and setup failures must abort the test
// loudly. Neither relaxation touches production code.
#![allow(dead_code, clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use melodee_persistence::{WorkEngineConfig, WorkItemStore, migrate};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub const DEFAULT_ADMIN_URL: &str = "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres";

/// Fixed epoch for deterministic time control in tests (no real sleeping).
pub const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

pub fn admin_url() -> String {
    std::env::var("MELODEE_TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_string())
}

pub struct TestDb {
    pub name: String,
    pub pool: PgPool,
    admin_pool: PgPool,
}

impl TestDb {
    pub async fn create(test_name: &str) -> Self {
        let admin_pool = PgPool::connect(&admin_url()).await.unwrap_or_else(|e| {
            panic!("connect to admin database (is dev Postgres running on :5433?): {e}")
        });
        let sanitized: String = test_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let name = format!("melodee_test_{}_{}", sanitized, Uuid::now_v7().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin_pool)
            .await
            .unwrap_or_else(|e| panic!("create test database: {e}"));

        let db_url = admin_url().replace("/postgres", &format!("/{name}"));
        let pool = PgPool::connect(&db_url)
            .await
            .unwrap_or_else(|e| panic!("connect to test database: {e}"));
        migrate(&pool)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        Self {
            name,
            pool,
            admin_pool,
        }
    }

    pub async fn drop(self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop test database: {e}"));
    }
}

/// Seeds a work item in `ready` state through the legal transition path
/// (discovered -> settling -> ready).
pub async fn seed_ready_item(store: &WorkItemStore, pool: &PgPool, path: &str) -> Uuid {
    let item =
        melodee_domain::ReleaseWorkItem::discovered(path.to_string(), None, Uuid::now_v7(), T0);
    let id = item.id;
    store.create(pool, &item).await.expect("create");
    store
        .system_transition(
            pool,
            id,
            melodee_domain::IngestionState::Settling,
            Some("test"),
            T0,
        )
        .await
        .expect("settling");
    store
        .system_transition(
            pool,
            id,
            melodee_domain::IngestionState::Ready,
            Some("test"),
            T0,
        )
        .await
        .expect("ready");
    id
}

pub fn test_config() -> WorkEngineConfig {
    WorkEngineConfig::default()
}
