//! CLI evidence for durable scheduled-job history and explicit cleanup safety.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use common::TestDb;
use melodee_persistence::JobRunHistoryStore;
use time::OffsetDateTime;

fn run_cli(database_url: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("run jobs CLI command")
}

#[tokio::test]
async fn jobs_history_is_machine_readable_and_cleanup_requires_confirmation() {
    let db = TestDb::create("cli_jobs").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));
    let now = OffsetDateTime::now_utc();
    let store = JobRunHistoryStore;
    let run = store
        .start_manual(&db.pool, "fixture", "cli-test", now)
        .await
        .expect("insert fixture job");
    store
        .finish(&db.pool, run.id, now, false, Some("fixture failure"))
        .await
        .expect("finish fixture job");

    let output = run_cli(
        &database_url,
        &["jobs", "history", "--job-name", "fixture", "--json"],
    );
    assert!(output.status.success());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json output");
    assert_eq!(document["total"], 1);
    assert_eq!(document["items"][0]["errorMessage"], "fixture failure");

    let output = run_cli(
        &database_url,
        &[
            "jobs",
            "cleanup",
            "--actor",
            "cli-test",
            "--retention-days",
            "1",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--confirm"));

    let output = run_cli(
        &database_url,
        &[
            "jobs",
            "cleanup",
            "--actor",
            "cli-test",
            "--retention-days",
            "1",
            "--confirm",
        ],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("removed 0"));

    db.drop().await;
}
