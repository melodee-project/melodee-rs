//! CLI evidence: `melodee notifications templates list` and
//! `melodee notifications test-send` (PRD-NOTIF-003). Test-send records the
//! honest audited outcome — `skipped` when email delivery is disabled —
//! instead of failing the command (PRD-NOTIF-002).

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;

/// Runs the CLI with the email environment scrubbed so a host shell cannot
/// leak MELODEE__EMAIL__* configuration into a test run.
fn run_cli(database_url: &str, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .env_remove("MELODEE__EMAIL__ENABLED")
        .env_remove("MELODEE__EMAIL__FROM_NAME")
        .env_remove("MELODEE__EMAIL__FROM_EMAIL")
        .env_remove("MELODEE__EMAIL__SMTP_HOST")
        .env_remove("MELODEE__EMAIL__SMTP_PORT")
        .env_remove("MELODEE__EMAIL__SMTP_USERNAME")
        .env_remove("MELODEE__EMAIL__SMTP_PASSWORD")
        .env_remove("MELODEE__EMAIL__SMTP_USE_SSL")
        .env_remove("MELODEE__EMAIL__SMTP_USE_START_TLS");
    let output = command.output().expect("run notifications CLI command");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI output is UTF-8")
}

fn run_cli_failing(database_url: &str, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url);
    let output = command.output().expect("run notifications CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

#[tokio::test]
async fn templates_list_shows_seeded_templates() {
    let db = TestDb::create("cli_notifications_list").await;
    let database_url = common::database_url(&db.name);

    let stdout = run_cli(&database_url, &["notifications", "templates", "list"]);
    for key in ["request.fulfilled", "share.created", "user.password_reset"] {
        assert!(stdout.contains(key), "stdout: {stdout}");
    }
    assert!(
        stdout.contains("Reset your Melodee password"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("-- 3 template(s)"), "stdout: {stdout}");

    db.drop().await;
}

#[tokio::test]
async fn test_send_records_honest_audited_outcomes() {
    let db = TestDb::create("cli_notifications_send").await;
    let database_url = common::database_url(&db.name);

    // Email delivery disabled (environment scrubbed): an honest skip, and
    // the command still succeeds.
    let stdout = run_cli(
        &database_url,
        &[
            "notifications",
            "test-send",
            "--template",
            "user.password_reset",
            "--to",
            "ops@example.test",
        ],
    );
    assert!(stdout.contains("status:      skipped"), "stdout: {stdout}");
    assert!(stdout.contains("disabled"), "stdout: {stdout}");

    // Unknown template: also an honest skip, not a command failure.
    let stdout = run_cli(
        &database_url,
        &[
            "notifications",
            "test-send",
            "--template",
            "no.such.event",
            "--to",
            "ops@example.test",
            "--locale",
            "de-DE",
        ],
    );
    assert!(stdout.contains("template not found"), "stdout: {stdout}");

    // Both sends were audited.
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM notification_deliveries")
        .fetch_one(&db.pool)
        .await
        .expect("count deliveries");
    assert_eq!(count, 2);

    // Bad recipient input fails closed without recording.
    let stderr = run_cli_failing(
        &database_url,
        &[
            "notifications",
            "test-send",
            "--template",
            "user.password_reset",
            "--to",
            &"r".repeat(321),
        ],
    );
    assert!(stderr.contains("error:"), "stderr: {stderr}");

    db.drop().await;
}
