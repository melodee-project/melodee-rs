//! scripting service integration tests (PRD-SCRIPT-004,
//! ADR-0135 §3/§4): the audited evaluation pipeline over PostgreSQL — empty
//! registry fast path, allow/deny verdicts with audit rows, fail-degraded
//! error recording, the circuit breaker's transactional trip+disable, the
//! ingestion gate's deny→skip mapping, and gate-level fail-degradation when
//! the store itself is gone.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use common::TestDb;
use melodee_ingestion::{CandidateSnapshot, DirectoryGateOutcome, DirectoryProcessingGate};
use melodee_persistence::{
    ScriptRegistryStore, ScriptRunOutcome, ScriptRunStore, ScriptUpsert, SettingsStore,
};
use melodee_server::scripting::{
    CIRCUIT_BREAKER_WINDOW, EVENT_DIRECTORY_PROCESSING_START, ScriptingDirectoryGate,
    ScriptingService,
};
use time::OffsetDateTime;

const EVENT: &str = EVENT_DIRECTORY_PROCESSING_START;

fn upsert<'a>(name: &'a str, source: &'a str, timeout_ms: i32) -> ScriptUpsert<'a> {
    ScriptUpsert {
        event: EVENT,
        name,
        source,
        timeout_ms,
        memory_bytes: 16 * 1024 * 1024,
    }
}

/// Enables the legacy master switch (`scripting.enabled`, default `false`)
/// for tests that exercise the ingestion gate.
async fn enable_scripting(db: &TestDb) {
    SettingsStore::new()
        .set(
            &db.pool,
            "scripting.enabled",
            &serde_json::json!(true),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("enable scripting");
}

async fn register(db: &TestDb, name: &str, source: &str, timeout_ms: i32) -> uuid::Uuid {
    ScriptRegistryStore::new()
        .upsert(
            &db.pool,
            &upsert(name, source, timeout_ms),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("register script")
        .id
}

#[tokio::test]
async fn empty_registry_allows_without_audit_rows() {
    let db = TestDb::create("scripting_empty").await;
    let service = ScriptingService::new(db.pool.clone());

    let verdict = service
        .evaluate_event(EVENT, &serde_json::json!({ "mediaFilesCount": 1 }))
        .await
        .expect("evaluate");
    assert!(verdict.allow);
    assert!(
        ScriptRunStore::new()
            .list_recent(&db.pool, None, 50, 0)
            .await
            .expect("runs")
            .is_empty(),
        "no scripts: no audit rows (legacy missing-setting default)"
    );
    db.drop().await;
}

#[tokio::test]
async fn deny_and_allow_verdicts_are_audited() {
    let db = TestDb::create("scripting_verdicts").await;
    let service = ScriptingService::new(db.pool.clone());
    let deny_id = register(
        &db,
        "min-three",
        "function check(ctx, scriptConfig) { if (ctx.mediaFilesCount < 3) { return { result: false, message: \"too few\" }; } return true; }",
        250,
    )
    .await;
    register(&db, "always-allow", "true", 250).await;

    let verdict = service
        .evaluate_event(EVENT, &serde_json::json!({ "mediaFilesCount": 2 }))
        .await
        .expect("evaluate");
    assert!(!verdict.allow);
    assert_eq!(verdict.message.as_deref(), Some("too few"));
    // Evaluation stops at the first deny (name order: always-allow first).
    let runs = ScriptRunStore::new()
        .list_recent(&db.pool, None, 50, 0)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].outcome, "deny");
    assert_eq!(runs[0].script_id, Some(deny_id));
    assert_eq!(runs[1].outcome, "allow");

    let verdict = service
        .evaluate_event(EVENT, &serde_json::json!({ "mediaFilesCount": 9 }))
        .await
        .expect("evaluate");
    assert!(verdict.allow);
    db.drop().await;
}

#[tokio::test]
async fn script_errors_fail_degraded_to_allow_and_are_audited() {
    let db = TestDb::create("scripting_failopen").await;
    let service = ScriptingService::new(db.pool.clone());
    register(
        &db,
        "thrower",
        "function check(ctx, scriptConfig) { throw new Error('broken'); }",
        250,
    )
    .await;

    let verdict = service
        .evaluate_event(EVENT, &serde_json::json!({}))
        .await
        .expect("evaluate");
    assert!(
        verdict.allow,
        "throwing script must not block the operation"
    );
    let runs = ScriptRunStore::new()
        .list_recent(&db.pool, None, 50, 0)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "error");
    assert!(
        runs[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("broken"))
    );
    db.drop().await;
}

#[tokio::test]
async fn circuit_breaker_trips_after_consecutive_timeouts_and_disables() {
    let db = TestDb::create("scripting_breaker").await;
    let service = ScriptingService::new(db.pool.clone());
    let script_id = register(
        &db,
        "runaway",
        "function check(ctx, scriptConfig) { while (true) {} }",
        50,
    )
    .await;

    for attempt in 0..CIRCUIT_BREAKER_WINDOW {
        let verdict = service
            .evaluate_event(EVENT, &serde_json::json!({}))
            .await
            .expect("evaluate");
        assert!(verdict.allow, "attempt {attempt} fail-degrades to allow");
    }

    let script = ScriptRegistryStore::new()
        .get(&db.pool, script_id)
        .await
        .expect("get");
    assert!(!script.enabled, "breaker disables the runaway script");
    assert_eq!(
        service.leaked_thread_count(),
        CIRCUIT_BREAKER_WINDOW as usize,
        "each timed-out execution detached one worker thread"
    );

    let runs = ScriptRunStore::new()
        .list_recent(&db.pool, Some(script_id), 50, 0)
        .await
        .expect("runs");
    let outcomes: Vec<&str> = runs.iter().map(|run| run.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        vec!["tripped", "timeout", "timeout", "timeout"],
        "newest first: the trip row follows the third timeout"
    );
    assert!(
        runs[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("circuit breaker"))
    );

    // Disabled: further evaluations are a no-op fast path.
    let verdict = service
        .evaluate_event(EVENT, &serde_json::json!({}))
        .await
        .expect("evaluate");
    assert!(verdict.allow);
    assert_eq!(
        ScriptRunStore::new()
            .list_recent(&db.pool, None, 50, 0)
            .await
            .expect("runs")
            .len(),
        4,
        "a disabled script records no further runs"
    );
    db.drop().await;
}

fn snapshot_of(files: &[(&str, i64)]) -> CandidateSnapshot {
    CandidateSnapshot {
        signature: "test-signature".to_string(),
        file_count: files.len() as i64,
        total_bytes: files.iter().map(|(_, size)| size).sum(),
        files: files
            .iter()
            .map(|(path, size)| melodee_ingestion::SnapshotFile {
                relative_path: (*path).to_string(),
                file_type: "file".to_string(),
                file_size: *size,
                modified_at: OffsetDateTime::now_utc(),
            })
            .collect(),
        ready_marker_seen: true,
        bypassed_settling: false,
    }
}

#[tokio::test]
async fn gate_maps_deny_to_skip_with_the_script_message() {
    let db = TestDb::create("scripting_gate_deny").await;
    enable_scripting(&db).await;
    let service = Arc::new(ScriptingService::new(db.pool.clone()));
    register(
        &db,
        "min-size",
        "{ result: ctx.totalSizeMegabytes >= 1, message: \"too small\" }",
        250,
    )
    .await;
    let gate = ScriptingDirectoryGate::new(service);
    let root = std::env::temp_dir().join(format!(
        "melodee-gate-test-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    let snapshot = snapshot_of(&[("01 - a.mp3", 1024), ("02 - b.mp3", 1024)]);

    let outcome = gate.evaluate(&root, &snapshot).await;
    assert_eq!(
        outcome,
        DirectoryGateOutcome::Skip {
            reason: "too small".to_string()
        }
    );
    let runs = ScriptRunStore::new()
        .list_recent(&db.pool, None, 50, 0)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, "deny");
    std::fs::remove_dir_all(&root).ok();
    db.drop().await;
}

#[tokio::test]
async fn gate_allows_when_the_ctx_satisfies_the_script() {
    let db = TestDb::create("scripting_gate_allow").await;
    enable_scripting(&db).await;
    let service = Arc::new(ScriptingService::new(db.pool.clone()));
    register(&db, "min-size", "ctx.totalSizeMegabytes >= 1", 250).await;
    let gate = ScriptingDirectoryGate::new(service);
    let root = std::env::temp_dir().join(format!(
        "melodee-gate-test-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    let snapshot = snapshot_of(&[("01 - a.mp3", 2 * 1024 * 1024)]);

    let outcome = gate.evaluate(&root, &snapshot).await;
    assert_eq!(outcome, DirectoryGateOutcome::Allow);
    assert_eq!(
        ScriptRunStore::new()
            .list_recent(&db.pool, None, 50, 0)
            .await
            .expect("runs")[0]
            .outcome,
        "allow"
    );
    std::fs::remove_dir_all(&root).ok();
    db.drop().await;
}

#[tokio::test]
async fn gate_is_inert_while_scripting_is_disabled() {
    // Legacy `scripting.enabled` defaults to false: the gate must not evaluate
    // installed scripts or write audit rows until an operator enables it.
    let db = TestDb::create("scripting_gate_disabled").await;
    let service = Arc::new(ScriptingService::new(db.pool.clone()));
    register(
        &db,
        "always-deny",
        "{ result: false, message: \"disabled\" }",
        250,
    )
    .await;
    let gate = ScriptingDirectoryGate::new(service);
    let root = std::env::temp_dir().join(format!(
        "melodee-gate-test-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    let snapshot = snapshot_of(&[("01 - a.mp3", 2048)]);

    let outcome = gate.evaluate(&root, &snapshot).await;
    assert_eq!(outcome, DirectoryGateOutcome::Allow);
    assert!(
        ScriptRunStore::new()
            .list_recent(&db.pool, None, 50, 0)
            .await
            .expect("runs")
            .is_empty(),
        "disabled scripting must not produce evaluation audit rows"
    );
    std::fs::remove_dir_all(&root).ok();
    db.drop().await;
}

#[tokio::test]
async fn gate_fail_degrades_to_allow_when_the_store_is_gone() {
    let db = TestDb::create("scripting_gate_storegone").await;
    let service = Arc::new(ScriptingService::new(db.pool.clone()));
    let gate = ScriptingDirectoryGate::new(service);
    let root = std::env::temp_dir();
    let snapshot = snapshot_of(&[("a.mp3", 1)]);
    let name = db.name.clone();
    db.drop().await;

    // Every query now fails; the gate must still allow (ADR-0135 §4).
    let outcome = gate.evaluate(&root, &snapshot).await;
    assert_eq!(
        outcome,
        DirectoryGateOutcome::Allow,
        "dropped database {name}: fail-degraded to allow"
    );
}

#[tokio::test]
async fn ad_hoc_test_run_records_a_null_script_row() {
    let db = TestDb::create("scripting_testrun").await;
    let service = ScriptingService::new(db.pool.clone());

    let (evaluation, run) = service
        .test_run(
            EVENT,
            None,
            "ctx.mediaFilesCount >= 3",
            250,
            &serde_json::json!({ "mediaFilesCount": 1 }),
        )
        .await
        .expect("test run");
    assert_eq!(evaluation.outcome, ScriptRunOutcome::Deny);
    assert_eq!(run.outcome, "deny");
    assert_eq!(run.script_id, None, "ad-hoc runs carry no registry link");
    db.drop().await;
}
