//! CLI evidence: `melodee worker-pools list|pause|resume` operates on
//! the same durable `worker_pool_state` rows the server workers honor
//! (PRD-CLI-002: the CLI never signals or bypasses a worker; PRD-WORK-005:
//! pause/resume is durable and audited).

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;

fn run_cli(database_url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("run worker-pools CLI command");
    let stdout = String::from_utf8(output.stdout).expect("CLI output is UTF-8");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn run_cli_failing(database_url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("run worker-pools CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

async fn pool_row(
    db: &TestDb,
    pool: &str,
) -> (
    bool,
    Option<String>,
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
) {
    sqlx::query_as(
        "SELECT paused,paused_by,paused_at,resume_requested_at FROM worker_pool_state WHERE pool=$1",
    )
    .bind(pool)
    .fetch_one(&db.pool)
    .await
    .expect("pool row")
}

#[tokio::test]
async fn worker_pool_pause_resume_is_durable_and_audited() {
    let db = TestDb::create("cli_worker_pools").await;
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));

    // The migration seeds exactly the wired pools, all unpaused.
    let stdout = run_cli(&database_url, &["worker-pools", "list"]);
    for pool in ["discovery", "podcast", "policy"] {
        assert!(stdout.contains(pool), "stdout: {stdout}");
    }
    assert!(stdout.contains("-- 3 pool(s)"), "stdout: {stdout}");

    // The operator identity is mandatory.
    let stderr = run_cli_failing(&database_url, &["worker-pools", "pause", "policy"]);
    assert!(stderr.contains("--by"), "stderr: {stderr}");
    // Pool names are validated before any SQL runs.
    run_cli_failing(
        &database_url,
        &["worker-pools", "pause", "Policy", "--by", "ops"],
    );

    let stdout = run_cli(
        &database_url,
        &["worker-pools", "pause", "policy", "--by", "ops-alice"],
    );
    assert!(stdout.contains("paused: policy"), "stdout: {stdout}");
    let (paused, paused_by, paused_at, resumed_at) = pool_row(&db, "policy").await;
    assert!(paused);
    assert_eq!(paused_by.as_deref(), Some("ops-alice"));
    assert!(paused_at.is_some());
    assert!(resumed_at.is_none());
    assert!(
        melodee_persistence::is_pool_paused(&db.pool, melodee_persistence::POOL_POLICY)
            .await
            .expect("pause visible to workers")
    );

    let stdout = run_cli(
        &database_url,
        &["worker-pools", "resume", "policy", "--by", "ops-bob"],
    );
    assert!(stdout.contains("resumed: policy"), "stdout: {stdout}");
    let (paused, paused_by, paused_at_after, resumed_at) = pool_row(&db, "policy").await;
    assert!(!paused);
    assert_eq!(
        (paused_by.as_deref(), paused_at_after),
        (Some("ops-alice"), paused_at),
        "resume retains the last-pause audit"
    );
    assert!(resumed_at.is_some());

    // Pausing an unregistered pool lazily registers it (upsert), so a pool
    // wired in a later binary is paused from its first check.
    let stdout = run_cli(
        &database_url,
        &["worker-pools", "pause", "search", "--by", "ops-carol"],
    );
    assert!(stdout.contains("paused: search"), "stdout: {stdout}");
    let stdout = run_cli(&database_url, &["worker-pools", "list"]);
    assert!(stdout.contains("search"), "stdout: {stdout}");
    assert!(stdout.contains("-- 4 pool(s)"), "stdout: {stdout}");

    db.drop().await;
}
