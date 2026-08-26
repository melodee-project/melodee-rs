//! Rollback orchestrator: guides the safe revert to the legacy system after a
//! failed v1.0 cutover (ADR-0148 §D.9). Does not modify legacy code or
//! infrastructure — produces audit evidence and documented steps.

use std::env;

use clap::Subcommand;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum RollbackCommands {
    /// Inspect the current rollback state: whether a cutover run exists,
    /// its freeze evidence, and what steps are required to revert safely.
    /// Does not change any system state.
    Status {
        /// Database URL (defaults to MELODEE__DATABASE__URL).
        #[arg(long)]
        database_url: Option<String>,
    },
    /// Generate a rollback runbook: a machine-readable JSON document listing
    /// the exact steps required to revert traffic and restore legacy write
    /// authority. The operator executes the infrastructure steps manually;
    /// this command only documents them with evidence. Idempotent — always
    /// produces the same output for the same input state. Does not change
    /// any system state. Requires --confirm to run (destructive decision).
    Runbook {
        /// Database URL (defaults to MELODEE__DATABASE__URL).
        #[arg(long)]
        database_url: Option<String>,
        /// Acknowledge that this produces a cutover procedure document.
        #[arg(long, default_value_t = false)]
        confirm: bool,
        /// Exact Rust service/container target recorded in the procedure.
        #[arg(long, default_value = "melodee")]
        rust_target: String,
        /// Exact legacy service/container target recorded in the procedure.
        #[arg(long)]
        legacy_target: String,
        /// Exact proxy/load-balancer target name recorded in the procedure.
        #[arg(long)]
        traffic_target: String,
    },
    /// Record that an operator executed rollback after traffic was restored
    /// to the legacy target. This changes only the durable audit record.
    Record {
        /// Database URL (defaults to MELODEE__DATABASE__URL).
        #[arg(long)]
        database_url: Option<String>,
        /// Migration run that reached cutover-ready.
        #[arg(long)]
        run: Uuid,
        /// Exact target returned to service.
        #[arg(long)]
        legacy_target: String,
        /// Stable operator identity.
        #[arg(long)]
        by: String,
        /// Auditable reason for rollback.
        #[arg(long)]
        note: String,
        /// Required acknowledgement of the durable audit mutation.
        #[arg(long, default_value_t = false)]
        confirm: bool,
    },
}

pub(crate) async fn run(command: RollbackCommands) {
    match command {
        RollbackCommands::Status { database_url } => {
            run_status(database_url.as_deref()).await;
        }
        RollbackCommands::Runbook {
            database_url,
            confirm,
            rust_target,
            legacy_target,
            traffic_target,
        } => {
            if !confirm {
                eprintln!("--confirm is required to produce the rollback runbook");
                std::process::exit(2);
            }
            match generate_runbook(
                database_url.as_deref(),
                &rust_target,
                &legacy_target,
                &traffic_target,
            )
            .await
            {
                Ok(runbook) => println!("{runbook}"),
                Err(e) => {
                    eprintln!("rollback runbook error: {e}");
                    std::process::exit(1);
                }
            }
        }
        RollbackCommands::Record {
            database_url,
            run,
            legacy_target,
            by,
            note,
            confirm,
        } => {
            if !confirm {
                eprintln!("--confirm is required to record rollback audit evidence");
                std::process::exit(2);
            }
            if let Err(error) =
                record(database_url.as_deref(), run, &legacy_target, &by, &note).await
            {
                eprintln!("rollback audit error: {error}");
                std::process::exit(1);
            }
        }
    }
}

fn database_url_or_env(value: Option<&str>) -> Result<String, String> {
    value
        .map(str::to_owned)
        .or_else(|| env::var(melodee_persistence::ENV_DATABASE_URL).ok())
        .ok_or_else(|| format!("{} is not set", melodee_persistence::ENV_DATABASE_URL))
}

#[derive(Debug, serde::Serialize)]
struct RollbackStatus {
    has_cutover_run: bool,
    latest_run_id: Option<String>,
    latest_state: String,
    freeze_evidence_present: bool,
    freeze_window_ms: Option<i64>,
    migration_current: bool,
    postgresql_reachable: bool,
}

async fn run_status(database_url: Option<&str>) {
    let url = database_url_or_env(database_url).unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    let pool = PgPool::connect(&url).await.unwrap_or_else(|e| {
        eprintln!("error: cannot connect to database: {e}");
        std::process::exit(2);
    });

    let migration = melodee_persistence::migration_status(&pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("warning: cannot read migration status: {e}");
            melodee_persistence::MigrationStatus {
                applied: 0,
                latest_available: 0,
                all_successful: false,
            }
        });

    let pg_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
        .is_ok_and(|r| r == 1);

    let latest_run: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM legacy_migration.runs ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or_else(|e| {
        eprintln!("warning: cannot query migration runs: {e}");
        None
    });

    let has_cutover_run = matches!(
        latest_run.as_ref().map(|(_, s)| s.as_str()),
        Some("cutover-ready") | Some("complete")
    );

    let (run_id, state) = latest_run
        .clone()
        .map(|(id, s)| (Some(id.to_string()), s))
        .unwrap_or_else(|| (None, "none".to_string()));

    let freeze_evidence: Option<String> = match run_id.as_deref() {
        Some(r) => {
            sqlx::query_scalar(r#"SELECT notes->'freeze' FROM legacy_migration.runs WHERE id = $1"#)
                .bind(uuid::Uuid::parse_str(r).unwrap_or_else(|_| uuid::Uuid::nil()))
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    let freeze_ms = freeze_evidence.as_ref().and_then(|e| {
        serde_json::from_str::<serde_json::Value>(e)
            .ok()
            .and_then(|v| v.get("freeze_window_ms")?.as_i64())
    });

    let status = RollbackStatus {
        has_cutover_run,
        latest_run_id: run_id,
        latest_state: state,
        freeze_evidence_present: freeze_evidence.is_some(),
        freeze_window_ms: freeze_ms,
        migration_current: migration.is_current(),
        postgresql_reachable: pg_ok,
    };

    println!(
        "{}",
        serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string())
    );

    if status.postgresql_reachable && status.migration_current {
        println!("System is ready for rollback operations.");
    } else {
        eprintln!("---");
        eprintln!("Rollback readiness:");
        if !status.postgresql_reachable {
            eprintln!(
                "  X PostgreSQL not reachable - ensure it is running and MELODEE__DATABASE__URL is set"
            );
        }
        if !status.migration_current {
            eprintln!(
                "  X Schema migrations not current - run `melodee-cli migrate` before cutover"
            );
        }
    }

    if let Some(ref id) = status.latest_run_id {
        println!(
            "Latest migration run: {id} (state: {})",
            status.latest_state
        );
    }
    if !status.has_cutover_run {
        eprintln!(
            "
Note: no cutover-ready or complete migration run found."
        );
        eprintln!("A rollback is only meaningful after a freeze+cutover sequence.");
    }
}

fn runbook_json(
    status: &RollbackStatus,
    rust_target: &str,
    legacy_target: &str,
    traffic_target: &str,
) -> String {
    let mut steps = Vec::new();
    steps.push(serde_json::json!({
        "step": 1,
        "title": "Verify rollback prerequisites",
        "target": "Rust PostgreSQL target",
        "expected_impact": "Read-only checks; no traffic or data changes.",
        "confirmation": "Operator confirms the database URL and migration run ID are exact.",
        "actions": [
            "Confirm MELODEE__DATABASE__URL points to the operational database",
            "Run `melodee rollback status` and `melodee doctor --deep`",
            "Verify the recorded rollback window and backups are retained"
        ],
    }));
    steps.push(serde_json::json!({
        "step": 2,
        "title": "Stop the Melodee v1.0 server",
        "target": rust_target,
        "expected_impact": "Rust traffic stops; in-flight requests drain; no Rust target rows are deleted.",
        "confirmation": "Approver confirms the exact Rust service target before stopping it.",
        "actions": [
            format!("Send SIGTERM to the exact service `{rust_target}` using the deployment's reviewed command"),
            format!("Verify `{rust_target}` exited through the deployment control plane; do not use a broad process-name match"),
            "Record the stop timestamp and retain the Rust target for forensics"
        ],
    }));
    steps.push(serde_json::json!({
        "step": 3,
        "title": "Revert reverse proxy / DNS routing",
        "target": traffic_target,
        "expected_impact": format!("Client traffic returns to the legacy target `{legacy_target}`; Rust is no longer authoritative for requests."),
        "confirmation": "Approver confirms the exact proxy/load-balancer target and rollback decision window.",
        "actions": [
            format!("Update the reviewed traffic control `{traffic_target}` to target `{legacy_target}`"),
            "Reload the traffic control through its reviewed, topology-checked deployment command",
            "Verify the legacy health and representative API probes before accepting writes"
        ],
    }));
    steps.push(serde_json::json!({
        "step": 4,
        "title": "Verify legacy server responsiveness",
        "target": legacy_target,
        "expected_impact": "Read-only compatibility probes and one known-client journey; no source mutation.",
        "confirmation": "Operator confirms probes target the restored legacy service and approved state.",
        "actions": [
            "Run the documented native-v1 health and public-ID lookup probes against the exact legacy target",
            "Run the documented OpenSubsonic and Jellyfin read-only probes against the exact legacy target",
            "Verify a known Release loads in an approved client"
        ],
    }));
    if let Some(ref run_id) = status.latest_run_id {
        steps.push(serde_json::json!({
            "step": 5,
            "title": "Record rollback audit evidence",
            "target": format!("Rust migration run {run_id}"),
            "expected_impact": "Append a durable rollback audit record; migration data remains retained.",
            "confirmation": "Operator supplies the exact legacy target, identity, and objective trigger.",
            "actions": [
                format!("Run `melodee rollback record --run {run_id} --legacy-target <exact-target> --by <operator> --note <trigger> --confirm`"),
                "Save this runbook and the post-rollback probes as forensic evidence",
            ],
        }));
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "document_type": "rollback_runbook",
        "version": "1.0",
        "generated_at": time::OffsetDateTime::now_utc().to_string(),
        "source_system": "melodee-rs",
        "legacy_system": "melodee (C#)",
        "prerequisites_met": status.postgresql_reachable && status.migration_current,
        "latest_migration_run": status.latest_run_id.as_deref().unwrap_or("none"),
        "freeze_evidence_present": status.freeze_evidence_present,
        "steps": steps,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

async fn generate_runbook(
    database_url: Option<&str>,
    rust_target: &str,
    legacy_target: &str,
    traffic_target: &str,
) -> Result<String, String> {
    for (label, value) in [
        ("rust target", rust_target),
        ("legacy target", legacy_target),
        ("traffic target", traffic_target),
    ] {
        if value.trim().is_empty() || value.chars().any(char::is_whitespace) {
            return Err(format!(
                "{label} must be a single non-blank topology identifier"
            ));
        }
    }
    let url = database_url_or_env(database_url)?;
    let pool = PgPool::connect(&url)
        .await
        .map_err(|e| format!("cannot connect to database: {e}"))?;

    let migration = melodee_persistence::migration_status(&pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("warning: cannot read migration status: {e}");
            melodee_persistence::MigrationStatus {
                applied: 0,
                latest_available: 0,
                all_successful: false,
            }
        });

    let pg_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
        .is_ok_and(|r| r == 1);

    let latest_run: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM legacy_migration.runs ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or_else(|e| {
        eprintln!("warning: cannot query migration runs: {e}");
        None
    });

    let has_cutover_run = matches!(
        latest_run.as_ref().map(|(_, s)| s.as_str()),
        Some("cutover-ready") | Some("complete")
    );

    let run_id = latest_run.clone().map(|(id, _)| id.to_string());

    let freeze_evidence: Option<String> = match run_id.as_deref() {
        Some(r) => {
            sqlx::query_scalar(r#"SELECT notes->'freeze' FROM legacy_migration.runs WHERE id = $1"#)
                .bind(uuid::Uuid::parse_str(r).unwrap_or_else(|_| uuid::Uuid::nil()))
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    let freeze_ms = freeze_evidence.as_ref().and_then(|e| {
        serde_json::from_str::<serde_json::Value>(e)
            .ok()
            .and_then(|v| v.get("freeze_window_ms")?.as_i64())
    });

    let status = RollbackStatus {
        has_cutover_run,
        latest_run_id: run_id,
        latest_state: latest_run
            .map(|(_, s)| s)
            .unwrap_or_else(|| "none".to_string()),
        freeze_evidence_present: freeze_evidence.is_some(),
        freeze_window_ms: freeze_ms,
        migration_current: migration.is_current(),
        postgresql_reachable: pg_ok,
    };

    Ok(runbook_json(
        &status,
        rust_target,
        legacy_target,
        traffic_target,
    ))
}

async fn record(
    database_url: Option<&str>,
    run_id: Uuid,
    legacy_target: &str,
    operator: &str,
    note: &str,
) -> Result<(), String> {
    if legacy_target.trim().is_empty() || legacy_target.chars().any(char::is_whitespace) {
        return Err("legacy target must be a single non-blank topology identifier".to_string());
    }
    if operator.trim().is_empty() || note.trim().is_empty() {
        return Err("rollback operator and note must not be blank".to_string());
    }
    let url = database_url_or_env(database_url)?;
    let pool = PgPool::connect(&url)
        .await
        .map_err(|e| format!("cannot connect to database: {e}"))?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| format!("cannot inspect migration run {run_id}: {e}"))?;
    if state.as_deref() != Some("cutover-ready") {
        return Err(format!(
            "rollback audit requires run {run_id} in cutover-ready state; complete/failed/unknown runs are not mutated"
        ));
    }
    let record = serde_json::json!({
        "status": "executed",
        "legacy_target": legacy_target,
        "operator": operator.trim(),
        "note": note.trim(),
        "recorded_at": time::OffsetDateTime::now_utc(),
    });
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET notes = COALESCE(notes, '{}'::jsonb) || jsonb_build_object('rollback', $2::jsonb),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(&record)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("cannot record rollback evidence: {e}"))?;
    sqlx::query(
        "INSERT INTO legacy_migration.issues
            (id, run_id, domain, source_pk, severity, code, message, detail, created_at)
         VALUES ($1, $2, 'rollback', NULL, 'info', 'rollback.executed',
                 'operator recorded rollback to the legacy target', $3, now())",
    )
    .bind(Uuid::now_v7())
    .bind(run_id)
    .bind(record)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("cannot record rollback audit issue: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    println!(
        "rollback audit recorded: run {run_id}, legacy target {}, operator {}",
        legacy_target, operator
    );
    Ok(())
}
