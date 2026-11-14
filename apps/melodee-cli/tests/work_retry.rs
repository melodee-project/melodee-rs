//! CLI evidence: `melodee work retry` requeues a failed or quarantined
//! work item through the identical operator state-machine arm the Admin UI
//! reprocess endpoint uses (`failed|quarantined -> ready`,
//! `TransitionKind::Operator`, audited reason) — PRD-CLI-002: the CLI never
//! bypasses the durable state machine.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::{TestDb, seed_ready_item, test_config};
use melodee_domain::{FailureClass, IngestionState, TransitionKind, WorkErrorCode};
use melodee_persistence::{WorkItemStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

fn run_cli(database_url: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("run work retry CLI command");
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
        .expect("run work retry CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

/// Parks one work item in `failed` through the legal worker path.
async fn seed_failed_item(db: &TestDb, source_path: &str) -> Uuid {
    let store = WorkItemStore;
    let config = test_config();
    let id = seed_ready_item(&store, &db.pool, source_path).await;
    let worker = WorkerId::new();
    let claimed = store
        .claim(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim failure fixture");
    assert!(claimed.iter().any(|item| item.id == id));
    store
        .transition(
            &db.pool,
            id,
            Some(&worker),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("start inspection");
    store
        .record_failure(
            &db.pool,
            id,
            &worker,
            WorkErrorCode::new(WorkErrorCode::STAGE_FAILED),
            "cli fixture failure",
            FailureClass::Permanent,
            &config.retry_policy,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("record permanent failure");
    id
}

#[tokio::test]
async fn work_retry_requeues_a_failed_item_with_an_audited_reason() {
    let db = TestDb::create("cli_work_retry").await;
    let id = seed_failed_item(&db, "cli/retry/failed-item").await;
    let database_url = common::database_url(&db.name);

    let stdout = run_cli(
        &database_url,
        &[
            "work",
            "retry",
            &id.to_string(),
            "--reason",
            "tags refreshed after rescan",
        ],
    );
    assert!(stdout.contains("requeued"), "{stdout}");
    assert!(stdout.contains("failed -> ready"), "{stdout}");

    let item = WorkItemStore
        .get(&db.pool, id)
        .await
        .expect("reload retried item");
    assert_eq!(item.state, IngestionState::Ready);

    let events = WorkItemStore
        .list_events(&db.pool, id)
        .await
        .expect("event history");
    let retry = events
        .iter()
        .find(|event| {
            event.from_state == Some(IngestionState::Failed)
                && event.to_state == Some(IngestionState::Ready)
        })
        .expect("operator transition event recorded");
    assert_eq!(
        retry.reason_code.as_deref(),
        Some("tags refreshed after rescan")
    );

    db.drop().await;
}

#[tokio::test]
async fn work_retry_rejects_items_that_are_not_failed_or_quarantined() {
    let db = TestDb::create("cli_work_retry_reject").await;
    let id = seed_ready_item(&WorkItemStore, &db.pool, "cli/retry/ready-item").await;

    let database_url = common::database_url(&db.name);
    let stderr = run_cli_failing(&database_url, &["work", "retry", &id.to_string()]);
    assert!(
        stderr.contains("only failed or quarantined items can be reprocessed"),
        "{stderr}"
    );

    // The state machine arm never fired: the item stays ready.
    let item = WorkItemStore
        .get(&db.pool, id)
        .await
        .expect("reload rejected item");
    assert_eq!(item.state, IngestionState::Ready);

    db.drop().await;
}

#[tokio::test]
async fn work_retry_fails_for_an_unknown_id() {
    let db = TestDb::create("cli_work_retry_missing").await;
    let database_url = common::database_url(&db.name);
    let stderr = run_cli_failing(
        &database_url,
        &["work", "retry", &Uuid::now_v7().to_string()],
    );
    assert!(stderr.contains("error:"), "{stderr}");
    db.drop().await;
}
