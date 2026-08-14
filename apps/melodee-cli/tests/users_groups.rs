//! WP-05 CLI evidence: `melodee users bootstrap-admin|disable|enable` and
//! `melodee groups ...` operator administration (PRD-CLI-001). Bootstrap
//! fails closed once any admin exists; the password comes from the
//! environment, never a CLI argument; group delete fails closed while
//! members or bindings remain.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;

fn run_cli(database_url: &str, args: &[&str], password: Option<&str>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        // Never inherit a host shell's bootstrap password into a test run.
        .env_remove("MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD");
    if let Some(password) = password {
        command.env("MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD", password);
    }
    let output = command.output().expect("run users/groups CLI command");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI output is UTF-8")
}

fn run_cli_failing(database_url: &str, args: &[&str], password: Option<&str>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .env_remove("MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD");
    if let Some(password) = password {
        command.env("MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD", password);
    }
    let output = command.output().expect("run users/groups CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

async fn seed_member(db: &TestDb, name: &str) {
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,$3,upper($3),$4,upper($4),$5,'bcrypt','cli-groups-key',now(),now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(name)
    .bind(format!("{name}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .execute(&db.pool)
    .await
    .expect("insert member");
}

#[tokio::test]
async fn bootstrap_admin_is_fail_closed_and_password_never_a_cli_argument() {
    let db = TestDb::create("cli_users_bootstrap").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));

    // Without the environment password the command refuses to run.
    let stderr = run_cli_failing(
        &database_url,
        &[
            "users",
            "bootstrap-admin",
            "--username",
            "admin",
            "--email",
            "admin@example.test",
        ],
        None,
    );
    assert!(
        stderr.contains("MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD"),
        "stderr: {stderr}"
    );

    let stdout = run_cli(
        &database_url,
        &[
            "users",
            "bootstrap-admin",
            "--username",
            "admin",
            "--email",
            "admin@example.test",
        ],
        Some("Boot-Strap-1!"),
    );
    assert!(
        stdout.contains("bootstrap admin created"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("is_admin:      true"), "stdout: {stdout}");

    // The account authenticates through the normal login path.
    let record = melodee_persistence::AuthStore::new()
        .user_for_auth(&db.pool, melodee_persistence::AuthLookup::Username("ADMIN"))
        .await
        .expect("auth lookup")
        .expect("admin exists");
    assert!(record.user.can_authenticate());
    assert!(
        melodee_auth::verify_password(
            b"Boot-Strap-1!",
            melodee_auth::PasswordHashAlgorithm::Bcrypt,
            record.password_hash.expose_secret(),
        )
        .expect("verify bootstrap password")
    );

    // Fail closed: a second bootstrap is refused.
    run_cli_failing(
        &database_url,
        &[
            "users",
            "bootstrap-admin",
            "--username",
            "second",
            "--email",
            "second@example.test",
        ],
        Some("Other-Password-2!"),
    );

    db.drop().await;
}

#[tokio::test]
async fn disable_enable_roundtrips_account_state() {
    let db = TestDb::create("cli_users_recovery").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));
    seed_member(&db, "recoverable-user").await;

    let stdout = run_cli(
        &database_url,
        &["users", "disable", "Recoverable-User"],
        None,
    );
    assert!(
        stdout.contains("disabled: recoverable-user"),
        "stdout: {stdout}"
    );
    let (enabled,): (bool,) =
        sqlx::query_as("SELECT enabled FROM native_users WHERE username='recoverable-user'")
            .fetch_one(&db.pool)
            .await
            .expect("enabled flag");
    assert!(!enabled);

    let stdout = run_cli(
        &database_url,
        &["users", "enable", "recoverable-user"],
        None,
    );
    assert!(
        stdout.contains("enabled: recoverable-user"),
        "stdout: {stdout}"
    );
    let (enabled,): (bool,) =
        sqlx::query_as("SELECT enabled FROM native_users WHERE username='recoverable-user'")
            .fetch_one(&db.pool)
            .await
            .expect("enabled flag");
    assert!(enabled);

    // Unknown users fail closed.
    run_cli_failing(&database_url, &["users", "disable", "ghost"], None);

    db.drop().await;
}

#[tokio::test]
async fn group_lifecycle_through_the_cli_fails_closed_with_members_and_bindings() {
    let db = TestDb::create("cli_groups_lifecycle").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));
    seed_member(&db, "cli-group-member").await;

    let stdout = run_cli(&database_url, &["groups", "create", "Family"], None);
    assert!(stdout.contains("created: Family"), "stdout: {stdout}");
    // Exact-name conflict fails closed.
    run_cli_failing(&database_url, &["groups", "create", "Family"], None);

    let stdout = run_cli(
        &database_url,
        &[
            "groups",
            "add-user",
            "Family",
            "--username",
            "CLI-Group-Member",
        ],
        None,
    );
    assert!(
        stdout.contains("added CLI-Group-Member"),
        "stdout: {stdout}"
    );
    let stdout = run_cli(&database_url, &["groups", "list"], None);
    assert!(stdout.contains("Family"), "stdout: {stdout}");
    assert!(stdout.contains("members=1"), "stdout: {stdout}");

    // Delete fails closed while a member remains.
    run_cli_failing(&database_url, &["groups", "delete", "Family"], None);

    let library = uuid::Uuid::now_v7().to_string();
    let stdout = run_cli(
        &database_url,
        &["groups", "bind-library", "Family", "--library-id", &library],
        None,
    );
    assert!(stdout.contains("bound library"), "stdout: {stdout}");

    let stdout = run_cli(
        &database_url,
        &[
            "groups",
            "remove-user",
            "Family",
            "--username",
            "cli-group-member",
        ],
        None,
    );
    assert!(
        stdout.contains("removed cli-group-member"),
        "stdout: {stdout}"
    );
    // Members are gone but the binding still blocks deletion.
    run_cli_failing(&database_url, &["groups", "delete", "Family"], None);

    let stdout = run_cli(
        &database_url,
        &[
            "groups",
            "unbind-library",
            "Family",
            "--library-id",
            &library,
        ],
        None,
    );
    assert!(stdout.contains("unbound library"), "stdout: {stdout}");
    let stdout = run_cli(&database_url, &["groups", "delete", "Family"], None);
    assert!(stdout.contains("deleted: Family"), "stdout: {stdout}");

    // Unknown groups and users fail closed.
    run_cli_failing(
        &database_url,
        &[
            "groups",
            "add-user",
            "Ghost",
            "--username",
            "cli-group-member",
        ],
        None,
    );
    run_cli_failing(
        &database_url,
        &["groups", "add-user", "Family2", "--username", "ghost-user"],
        None,
    );

    db.drop().await;
}
