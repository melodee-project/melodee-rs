//! scripting persistence acceptance (ADR-0134 §6, ADR-0135,
//! PRD-SCRIPT-001..004): migration shape (nothing seeded — scripts are
//! operator-installed), registry CRUD with version bump on source change
//! only, bounds enforcement with the SQL CHECKs as backstop, the append-only
//! execution audit with ON DELETE SET NULL survival, bounded retention, the
//! circuit-breaker failure-window query, and transactional trip+disable.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    NewScriptRun, ScriptRegistryStore, ScriptRunOutcome, ScriptRunStore, ScriptStoreError,
    ScriptUpsert,
};
use time::{Duration, OffsetDateTime};

const EVENT: &str = "directoryProcessingStart";

fn upsert<'a>(event: &'a str, name: &'a str, source: &'a str) -> ScriptUpsert<'a> {
    ScriptUpsert {
        event,
        name,
        source,
        timeout_ms: 250,
        memory_bytes: 16 * 1024 * 1024,
    }
}

fn run<'a>(script_id: Option<uuid::Uuid>, outcome: ScriptRunOutcome) -> NewScriptRun<'a> {
    NewScriptRun {
        script_id,
        event: EVENT,
        outcome,
        duration_ms: 12,
        error: None,
    }
}

#[tokio::test]
async fn migration_seeds_nothing_scripts_are_operator_installed() {
    let db = TestDb::create("scripts_seed").await;
    let registry = ScriptRegistryStore::new();

    let listed = registry.list(&db.pool, 50, 0).await.expect("list");
    assert!(listed.is_empty(), "no seeded scripts (ADR-0135)");

    let runs = ScriptRunStore::new()
        .list_recent(&db.pool, None, 50, 0)
        .await
        .expect("list runs");
    assert!(runs.is_empty());
    db.drop().await;
}

#[tokio::test]
async fn registry_crud_and_version_bumps_only_on_source_change() {
    let db = TestDb::create("scripts_crud").await;
    let registry = ScriptRegistryStore::new();
    let now = OffsetDateTime::now_utc();

    let created = registry
        .upsert(
            &db.pool,
            &upsert(EVENT, "min-three-tracks", "ctx.mediaFilesCount >= 3"),
            now,
        )
        .await
        .expect("create");
    assert_eq!(created.version, 1);
    assert!(created.enabled, "new scripts default to enabled");
    assert_eq!(created.timeout_ms, 250);
    assert_eq!(created.memory_bytes, 16 * 1024 * 1024);

    // Re-saving identical source churns updated_at but not the version.
    let resaved = registry
        .upsert(
            &db.pool,
            &upsert(EVENT, "min-three-tracks", "ctx.mediaFilesCount >= 3"),
            now + Duration::seconds(1),
        )
        .await
        .expect("resave");
    assert_eq!(resaved.version, 1, "unchanged source does not bump version");
    assert_eq!(resaved.id, created.id, "upsert keeps the stable id");
    assert!(resaved.updated_at > created.updated_at);

    // A source change bumps the version.
    let changed = registry
        .upsert(
            &db.pool,
            &upsert(EVENT, "min-three-tracks", "ctx.mediaFilesCount >= 4"),
            now + Duration::seconds(2),
        )
        .await
        .expect("source change");
    assert_eq!(changed.version, 2);

    // Enable/disable is explicit and survives upserts.
    let disabled = registry
        .set_enabled(&db.pool, created.id, false, now + Duration::seconds(3))
        .await
        .expect("disable");
    assert!(!disabled.enabled);
    let reupserted = registry
        .upsert(
            &db.pool,
            &upsert(EVENT, "min-three-tracks", "ctx.mediaFilesCount >= 5"),
            now + Duration::seconds(4),
        )
        .await
        .expect("upsert while disabled");
    assert_eq!(reupserted.version, 3);
    assert!(
        !reupserted.enabled,
        "upsert must not silently re-enable (circuit breaker safety)"
    );

    // Enabled-for-event lookup skips disabled rows.
    assert!(
        registry
            .list_enabled_for_event(&db.pool, EVENT)
            .await
            .expect("enabled lookup")
            .is_empty()
    );
    registry
        .set_enabled(&db.pool, created.id, true, now + Duration::seconds(5))
        .await
        .expect("re-enable");
    let enabled = registry
        .list_enabled_for_event(&db.pool, EVENT)
        .await
        .expect("enabled lookup");
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].source, "ctx.mediaFilesCount >= 5");

    // get / get_by_name / delete.
    let by_name = registry
        .get_by_name(&db.pool, EVENT, "min-three-tracks")
        .await
        .expect("get by name");
    assert_eq!(by_name.id, created.id);
    registry.delete(&db.pool, created.id).await.expect("delete");
    assert!(matches!(
        registry.get(&db.pool, created.id).await,
        Err(ScriptStoreError::ScriptNotFound)
    ));
    assert!(matches!(
        registry.delete(&db.pool, created.id).await,
        Err(ScriptStoreError::ScriptNotFound)
    ));
    db.drop().await;
}

#[tokio::test]
async fn bounds_are_enforced_in_code_and_by_sql_backstop() {
    let db = TestDb::create("scripts_bounds").await;
    let registry = ScriptRegistryStore::new();
    let now = OffsetDateTime::now_utc();

    for bad_event in ["", "   ", &"e".repeat(101)] {
        let result = registry
            .upsert(&db.pool, &upsert(bad_event, "n", "true"), now)
            .await;
        assert!(
            matches!(result, Err(ScriptStoreError::InvalidScript(_))),
            "event {bad_event:?}"
        );
    }
    for bad_name in ["", "  ", &"n".repeat(256), "has/slash", "ctl\nchar"] {
        let result = registry
            .upsert(&db.pool, &upsert(EVENT, bad_name, "true"), now)
            .await;
        assert!(
            matches!(result, Err(ScriptStoreError::InvalidScript(_))),
            "name {bad_name:?}"
        );
    }
    for bad_source in ["", "   ", &"s".repeat(256 * 1024 + 1)] {
        let result = registry
            .upsert(&db.pool, &upsert(EVENT, "n", bad_source), now)
            .await;
        assert!(
            matches!(result, Err(ScriptStoreError::InvalidScript(_))),
            "source length {}",
            bad_source.len()
        );
    }
    for bad_timeout in [0, -1, 10_001] {
        let mut payload = upsert(EVENT, "n", "true");
        payload.timeout_ms = bad_timeout;
        let result = registry.upsert(&db.pool, &payload, now).await;
        assert!(
            matches!(result, Err(ScriptStoreError::InvalidScript(_))),
            "timeout {bad_timeout}"
        );
    }
    for bad_memory in [4095, 64 * 1024 * 1024 + 1] {
        let mut payload = upsert(EVENT, "n", "true");
        payload.memory_bytes = bad_memory;
        let result = registry.upsert(&db.pool, &payload, now).await;
        assert!(
            matches!(result, Err(ScriptStoreError::InvalidScript(_))),
            "memory {bad_memory}"
        );
    }

    // The migration CHECKs are the fail-closed backstop against raw SQL.
    for bad_sql in [
        "INSERT INTO script_registry (id, event, name, source) VALUES (gen_random_uuid(), '', 'n', 'true')",
        "INSERT INTO script_registry (id, event, name, source) VALUES (gen_random_uuid(), 'e', 'has/slash', 'true')",
        "INSERT INTO script_registry (id, event, name, source, timeout_ms) VALUES (gen_random_uuid(), 'e', 'n', 'true', 10001)",
        "INSERT INTO script_registry (id, event, name, source, memory_bytes) VALUES (gen_random_uuid(), 'e', 'n', 'true', 1024)",
        "INSERT INTO script_registry (id, event, name, source) VALUES (gen_random_uuid(), 'e', 'n', repeat('s', 262145))",
    ] {
        assert!(
            sqlx::query(bad_sql).execute(&db.pool).await.is_err(),
            "{bad_sql}"
        );
    }
    db.drop().await;
}

#[tokio::test]
async fn run_audit_is_append_only_survives_delete_and_is_retained() {
    let db = TestDb::create("scripts_runs").await;
    let registry = ScriptRegistryStore::new();
    let runs = ScriptRunStore::new();
    let now = OffsetDateTime::now_utc();

    let script = registry
        .upsert(&db.pool, &upsert(EVENT, "gate", "true"), now)
        .await
        .expect("create");
    for outcome in [
        ScriptRunOutcome::Allow,
        ScriptRunOutcome::Deny,
        ScriptRunOutcome::Error,
    ] {
        runs.record_run(&db.pool, &run(Some(script.id), outcome), now)
            .await
            .expect("record");
    }
    // Ad-hoc test-run rows carry no script reference.
    runs.record_run(&db.pool, &run(None, ScriptRunOutcome::Allow), now)
        .await
        .expect("record ad-hoc");

    let listed = runs.list_recent(&db.pool, None, 50, 0).await.expect("list");
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0].outcome, "allow", "newest first");
    assert_eq!(listed[0].script_id, None);

    // Scoped listing.
    let scoped = runs
        .list_recent(&db.pool, Some(script.id), 50, 0)
        .await
        .expect("scoped list");
    assert_eq!(scoped.len(), 3);

    // Long errors are truncated to the audit bound on a char boundary.
    let long_error = "e".repeat(5000);
    let mut failing = run(Some(script.id), ScriptRunOutcome::Error);
    failing.error = Some(&long_error);
    let truncated = runs
        .record_run(&db.pool, &failing, now)
        .await
        .expect("record truncated error");
    assert!(
        truncated
            .error
            .as_deref()
            .is_some_and(|error| error.chars().count() <= 2000 && error.ends_with('…'))
    );

    // Deleting the script must not delete its audit rows (ON DELETE SET NULL).
    registry.delete(&db.pool, script.id).await.expect("delete");
    let surviving = runs
        .list_recent(&db.pool, None, 50, 0)
        .await
        .expect("list after delete");
    assert_eq!(surviving.len(), 5);
    assert!(surviving.iter().all(|row| row.script_id.is_none()));

    // The outcome CHECK is the fail-closed backstop for the audit vocabulary.
    let raw = sqlx::query("INSERT INTO script_runs (id, event, outcome) VALUES ($1, 'e', 'bogus')")
        .bind(uuid::Uuid::now_v7())
        .execute(&db.pool)
        .await;
    assert!(raw.is_err(), "unknown outcome must violate the CHECK");

    // Retention purge removes only rows older than the cutoff.
    sqlx::query(
        "INSERT INTO script_runs (id, event, outcome, created_at) \
         VALUES ($1, 'e', 'allow', $2)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(now - Duration::days(120))
    .execute(&db.pool)
    .await
    .expect("insert old row");
    let purged = runs
        .purge_older_than(&db.pool, now - Duration::days(90))
        .await
        .expect("purge");
    assert_eq!(purged, 1);
    assert_eq!(
        runs.list_recent(&db.pool, None, 50, 0)
            .await
            .expect("list")
            .len(),
        5
    );
    db.drop().await;
}

#[tokio::test]
async fn failure_window_counts_only_recent_failures() {
    let db = TestDb::create("scripts_breaker_window").await;
    let registry = ScriptRegistryStore::new();
    let runs = ScriptRunStore::new();
    let now = OffsetDateTime::now_utc();

    let script = registry
        .upsert(&db.pool, &upsert(EVENT, "flaky", "true"), now)
        .await
        .expect("create");

    // timeout, timeout, allow, timeout (oldest→newest): the 3-run window
    // holds [allow, timeout,...] -> not a full-failure window.
    for (index, outcome) in [
        ScriptRunOutcome::Timeout,
        ScriptRunOutcome::Timeout,
        ScriptRunOutcome::Allow,
        ScriptRunOutcome::Timeout,
    ]
    .into_iter()
    .enumerate()
    {
        runs.record_run(
            &db.pool,
            &run(Some(script.id), outcome),
            now + Duration::seconds(index as i64),
        )
        .await
        .expect("record");
    }
    assert_eq!(
        runs.count_recent_failures(&db.pool, script.id, 3)
            .await
            .expect("count"),
        2,
        "an allow inside the window breaks the consecutive-failure streak"
    );

    // Two more timeouts fill the window with failures only.
    for index in 4..6 {
        runs.record_run(
            &db.pool,
            &run(Some(script.id), ScriptRunOutcome::Timeout),
            now + Duration::seconds(index),
        )
        .await
        .expect("record");
    }
    assert_eq!(
        runs.count_recent_failures(&db.pool, script.id, 3)
            .await
            .expect("count"),
        3,
        "three consecutive failures trip the threshold"
    );

    // Failures of another script never leak into the window.
    let other = registry
        .upsert(&db.pool, &upsert(EVENT, "other", "true"), now)
        .await
        .expect("create other");
    assert_eq!(
        runs.count_recent_failures(&db.pool, other.id, 3)
            .await
            .expect("count"),
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn circuit_breaker_trip_is_transactional_audit_plus_disable() {
    let db = TestDb::create("scripts_trip").await;
    let registry = ScriptRegistryStore::new();
    let runs = ScriptRunStore::new();
    let now = OffsetDateTime::now_utc();

    let script = registry
        .upsert(&db.pool, &upsert(EVENT, "runaway", "while(true){}"), now)
        .await
        .expect("create");

    let mut trip = run(Some(script.id), ScriptRunOutcome::Tripped);
    trip.error = Some("3 consecutive timeouts; script disabled");
    let row = runs
        .record_trip_and_disable(&db.pool, &trip, now)
        .await
        .expect("trip");
    assert_eq!(row.outcome, "tripped");

    let after = registry.get(&db.pool, script.id).await.expect("get");
    assert!(!after.enabled, "trip disables the script in the registry");
    let audit = runs
        .list_recent(&db.pool, Some(script.id), 50, 0)
        .await
        .expect("audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].outcome, "tripped");

    // A tripped row without a script reference is rejected.
    assert!(matches!(
        runs.record_trip_and_disable(&db.pool, &run(None, ScriptRunOutcome::Tripped), now)
            .await,
        Err(ScriptStoreError::InvalidRun(_))
    ));
    // So is a non-tripped outcome through the trip path.
    assert!(matches!(
        runs.record_trip_and_disable(
            &db.pool,
            &run(Some(script.id), ScriptRunOutcome::Error),
            now
        )
        .await,
        Err(ScriptStoreError::InvalidRun(_))
    ));
    db.drop().await;
}
