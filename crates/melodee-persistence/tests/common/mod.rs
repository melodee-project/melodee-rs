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
use sqlx::{ConnectOptions, PgPool, postgres::PgConnectOptions};
use time::OffsetDateTime;
use uuid::Uuid;

pub const DEFAULT_ADMIN_URL: &str = "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres";
pub const T0: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

pub fn admin_url() -> String {
    std::env::var("MELODEE_TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_string())
}

/// Retain the entire UUID: PostgreSQL silently truncates identifiers after 63 bytes.
pub fn database_name(label: &str) -> String {
    let label: String = label
        .chars()
        .take(17)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("melodee_test_{label}_{}", Uuid::now_v7().simple())
}

pub fn database_options(admin: &str, name: &str) -> PgConnectOptions {
    assert!(is_owned_name(name), "refusing non-test database target");
    admin
        .parse::<PgConnectOptions>()
        .expect("parse test administrator options")
        .database(name)
}

pub fn database_url(name: &str) -> String {
    database_options(&admin_url(), name)
        .to_url_lossy()
        .to_string()
}

fn is_owned_name(name: &str) -> bool {
    let Some(tail) = name.strip_prefix("melodee_test_") else {
        return false;
    };
    let Some((_, suffix)) = tail.rsplit_once('_') else {
        return false;
    };
    name.len() <= 63
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && suffix.len() == 32
        && suffix.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Owns only the fresh UUID-named database created by this test, including during unwinding.
pub struct DatabaseGuard {
    pub name: String,
    admin: PgConnectOptions,
    armed: bool,
}

impl DatabaseGuard {
    pub async fn create(label: &str) -> Self {
        Self::create_from_template(label, None).await
    }

    pub async fn create_from_template(label: &str, template: Option<&str>) -> Self {
        let name = database_name(label);
        let admin = admin_url()
            .parse::<PgConnectOptions>()
            .expect("parse administrator options");
        let pool = PgPool::connect_with(admin.clone())
            .await
            .expect("connect test administrator");
        let suffix = template.map_or_else(String::new, |name| {
            let suffix = name
                .strip_prefix("melodee_template_")
                .expect("owned test template prefix");
            assert!(
                suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid template name"
            );
            format!(" TEMPLATE \"{name}\"")
        });
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{name}\"{suffix}"
        )))
        .execute(&pool)
        .await
        .expect("create owned test database");
        pool.close().await;
        Self {
            name,
            admin,
            armed: true,
        }
    }

    pub fn url(&self) -> String {
        self.admin
            .clone()
            .database(&self.name)
            .to_url_lossy()
            .to_string()
    }

    pub async fn connect(&self) -> PgPool {
        let pool = PgPool::connect_with(self.admin.clone().database(&self.name))
            .await
            .expect("connect owned test database");
        let actual: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .expect("verify test database");
        assert_eq!(
            actual, self.name,
            "test database selection mismatch; refusing migrations"
        );
        pool
    }

    pub async fn cleanup(&mut self) {
        if self.armed {
            drop_database(&self.admin, &self.name)
                .await
                .expect("drop owned test database");
            self.armed = false;
        }
    }
}

async fn drop_database(admin: &PgConnectOptions, name: &str) -> Result<(), sqlx::Error> {
    assert!(is_owned_name(name), "refusing non-test database cleanup");
    let pool = PgPool::connect_with(admin.clone()).await?;
    // IF EXISTS makes explicit cleanup followed by unwinding harmless.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&pool)
    .await?;
    pool.close().await;
    Ok(())
}

pub async fn drop_owned_database(name: &str) {
    let admin = admin_url()
        .parse::<PgConnectOptions>()
        .expect("parse administrator options");
    drop_database(&admin, name)
        .await
        .expect("drop owned test database");
}

impl Drop for DatabaseGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let admin = self.admin.clone();
        let name = self.name.clone();
        // Tokio test runtimes may already be unwinding. A dedicated runtime makes
        // cleanup finish before the process exits without nesting block_on.
        let result = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?
                .block_on(drop_database(&admin, &name))
                .map_err(|error| error.to_string())
        })
        .join();
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("failed to clean up owned test database {}", self.name);
        }
    }
}

#[must_use = "keep the fixture alive until every database operation has completed"]
pub struct TestDb {
    pub name: String,
    pub pool: PgPool,
    guard: DatabaseGuard,
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Implementing Drop prevents partial moves of `pool`/`name` from silently
        // leaving the ownership guard behind in a fixture constructor. The fields
        // still drop automatically, and DatabaseGuard performs synchronous cleanup.
    }
}

impl TestDb {
    pub async fn create(test_name: &str) -> Self {
        let template = std::env::var("MELODEE_TEST_TEMPLATE_DATABASE").ok();
        let guard = DatabaseGuard::create_from_template(test_name, template.as_deref()).await;
        let pool = guard.connect().await;
        let db = Self {
            name: guard.name.clone(),
            pool,
            guard,
        };
        // Recheck checksums and apply any new migrations even for a cloned template.
        migrate(&db.pool)
            .await
            .expect("migrate owned test database");
        db
    }

    /// Forward migration tests always receive an empty database.
    pub async fn create_unmigrated(test_name: &str) -> Self {
        let guard = DatabaseGuard::create(test_name).await;
        let pool = guard.connect().await;
        Self {
            name: guard.name.clone(),
            pool,
            guard,
        }
    }

    pub async fn drop(mut self) {
        self.pool.close().await;
        self.guard.cleanup().await;
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
