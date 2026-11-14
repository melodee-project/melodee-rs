//! Target selection and cleanup safety of PostgreSQL test fixtures.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use sqlx::{ConnectOptions, PgPool};

#[test]
fn options_replace_only_database_and_preserve_connection_parameters() {
    let name = common::database_name("unusually_long_test_name_that_must_not_truncate_uuid");
    assert_eq!(name.len(), 63);
    let options = common::database_options(
        "postgres://test:synthetic@localhost:5433/test_admin?sslmode=disable",
        &name,
    );
    assert_eq!(options.get_database(), Some(name.as_str()));
    assert_eq!(options.get_port(), 5433);
    assert_eq!(options.get_username(), "test");
    assert!(options.to_url_lossy().as_str().contains(&name));
    assert_ne!(
        name,
        common::database_name("unusually_long_test_name_that_must_not_truncate_uuid")
    );
}

#[test]
fn ordinary_database_names_are_rejected_before_connection() {
    assert!(
        std::panic::catch_unwind(|| common::database_options(
            "postgres://localhost/postgres",
            "production"
        ))
        .is_err()
    );
}

#[tokio::test]
async fn unwinding_drops_only_the_database_created_by_the_fixture() {
    let guard = common::DatabaseGuard::create("panic_cleanup").await;
    let name = guard.name.clone();
    let admin = PgPool::connect(&common::admin_url()).await.expect("admin");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = guard;
        panic!("intentional fixture unwind");
    }));
    assert!(result.is_err());
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&name)
            .fetch_one(&admin)
            .await
            .expect("database existence");
    assert!(!exists);
    let admin_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&admin)
        .await
        .expect("administrator survives");
    assert_ne!(admin_name, name);
}

#[tokio::test]
async fn forward_migration_fixture_starts_empty_even_with_a_template() {
    let db = common::TestDb::create_unmigrated("fresh_migration").await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = 'public')")
            .fetch_one(&db.pool)
            .await
            .expect("table inventory");
    assert!(!exists);
    db.drop().await;
}

#[tokio::test]
async fn returned_fixture_owns_database_across_pool_restarts_until_drop() {
    struct Scenario {
        database: common::TestDb,
    }
    async fn scenario() -> Scenario {
        Scenario {
            database: common::TestDb::create("returned_owner").await,
        }
    }

    let fixture = scenario().await;
    let name = fixture.database.name.clone();
    // A process restart closes application connections, not the fixture's
    // database ownership. Reconnecting must still see the migrated database.
    fixture.database.pool.close().await;
    let pool = PgPool::connect(&common::database_url(&name))
        .await
        .expect("reconnect after returning fixture");
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("returned fixture database survives");
    assert_eq!(actual, name);
    let migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration state survives connection restart");
    assert!(migrations > 0);
    pool.close().await;
    drop(fixture);

    let admin = PgPool::connect(&common::admin_url()).await.expect("admin");
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&name)
            .fetch_one(&admin)
            .await
            .expect("database existence after owner drop");
    assert!(!exists, "cleanup waits for the returned fixture owner");
}
