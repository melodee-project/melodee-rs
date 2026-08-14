//! WP-11 CLI evidence: `melodee scripts list` and `melodee scripts test`
//! (PRD-SCRIPT-001/004, ADR-0135). Test runs evaluate in the real sandbox
//! and record honest `script_runs` audit rows — including error outcomes —
//! instead of failing the command.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;
use melodee_persistence::{ScriptRegistryStore, ScriptUpsert};
use time::OffsetDateTime;

fn run_cli(database_url: &str, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url);
    let output = command.output().expect("run scripts CLI command");
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
    let output = command.output().expect("run scripts CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

fn write_temp(content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "melodee-cli-scripts-test-{}.json",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

#[tokio::test]
async fn scripts_list_shows_registry_and_wired_flags() {
    let db = TestDb::create("cli_scripts_list").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));

    // Empty registry is honest.
    let stdout = run_cli(&database_url, &["scripts", "list"]);
    assert!(stdout.contains("-- 0 script(s)"), "stdout: {stdout}");

    for (event, name) in [
        ("directoryProcessingStart", "min-three-tracks"),
        ("userLoginStart", "future-hook"),
    ] {
        ScriptRegistryStore::new()
            .upsert(
                &db.pool,
                &ScriptUpsert {
                    event,
                    name,
                    source: "true",
                    timeout_ms: 250,
                    memory_bytes: 16 * 1024 * 1024,
                },
                OffsetDateTime::now_utc(),
            )
            .await
            .expect("register");
    }
    let stdout = run_cli(&database_url, &["scripts", "list"]);
    assert!(
        stdout.contains("directoryProcessingStart") && stdout.contains("min-three-tracks"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("wired"), "stdout: {stdout}");
    assert!(stdout.contains("inert"), "stdout: {stdout}");
    assert!(stdout.contains("-- 2 script(s)"), "stdout: {stdout}");

    db.drop().await;
}

#[tokio::test]
async fn scripts_test_evaluates_ad_hoc_source_and_records_the_run() {
    let db = TestDb::create("cli_scripts_test_adhoc").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));
    let ctx = write_temp(r#"{ "mediaFilesCount": 2 }"#);
    let source = write_temp("ctx.mediaFilesCount >= 3");

    let stdout = run_cli(
        &database_url,
        &[
            "scripts",
            "test",
            "--event",
            "directoryProcessingStart",
            "--ctx",
            ctx.to_str().expect("utf-8 path"),
            "--source",
            source.to_str().expect("utf-8 path"),
        ],
    );
    assert!(stdout.contains("outcome:     deny"), "stdout: {stdout}");
    assert!(stdout.contains("run_id:"), "stdout: {stdout}");

    // The run is honestly recorded (ad-hoc: no registry link).
    let runs = melodee_persistence::ScriptRunStore::new()
        .list_recent(&db.pool, None, 10, 0)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "deny");
    assert_eq!(runs[0].script_id, None);
    db.drop().await;
}

#[tokio::test]
async fn scripts_test_without_source_evaluates_stored_scripts_in_order() {
    let db = TestDb::create("cli_scripts_test_stored").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));

    ScriptRegistryStore::new()
        .upsert(
            &db.pool,
            &ScriptUpsert {
                event: "directoryProcessingStart",
                name: "min-three-tracks",
                source: "function check(ctx, scriptConfig) { if (ctx.mediaFilesCount < 3) { return { result: false, message: \"too few\" }; } return true; }",
                timeout_ms: 250,
                memory_bytes: 16 * 1024 * 1024,
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("register");

    // No enabled scripts for a different event: honest default verdict.
    let ctx = write_temp(r#"{ "mediaFilesCount": 1 }"#);
    let stdout = run_cli(
        &database_url,
        &[
            "scripts",
            "test",
            "--event",
            "podcastChannelAddStart",
            "--ctx",
            ctx.to_str().expect("utf-8 path"),
        ],
    );
    assert!(stdout.contains("no enabled scripts"), "stdout: {stdout}");
    assert!(stdout.contains("verdict: allow"), "stdout: {stdout}");

    // Stored script denies with its message; the run is recorded and linked.
    let stdout = run_cli(
        &database_url,
        &[
            "scripts",
            "test",
            "--event",
            "directoryProcessingStart",
            "--ctx",
            ctx.to_str().expect("utf-8 path"),
        ],
    );
    assert!(stdout.contains("min-three-tracks"), "stdout: {stdout}");
    assert!(stdout.contains("outcome:     deny"), "stdout: {stdout}");
    assert!(stdout.contains("verdict: deny"), "stdout: {stdout}");
    let runs = melodee_persistence::ScriptRunStore::new()
        .list_recent(&db.pool, None, 10, 0)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "deny");
    assert!(runs[0].script_id.is_some());
    db.drop().await;
}

#[tokio::test]
async fn scripts_test_rejects_invalid_inputs() {
    let db = TestDb::create("cli_scripts_test_invalid").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));

    // Missing ctx file.
    let ctx = write_temp(r#"{}"#);
    let stderr = run_cli_failing(
        &database_url,
        &[
            "scripts",
            "test",
            "--event",
            "directoryProcessingStart",
            "--ctx",
            "/nonexistent/ctx.json",
        ],
    );
    assert!(stderr.contains("cannot read"), "stderr: {stderr}");

    // Invalid JSON ctx.
    let bad = write_temp("not json");
    let stderr = run_cli_failing(
        &database_url,
        &[
            "scripts",
            "test",
            "--event",
            "directoryProcessingStart",
            "--ctx",
            bad.to_str().expect("utf-8 path"),
        ],
    );
    assert!(stderr.contains("not valid JSON"), "stderr: {stderr}");
    drop(ctx);
    db.drop().await;
}
