//! Durable migration run model: records, lease fencing, checkpoints, issues,
//! and status reporting (ADR-0148 §D.5/§D.6).

use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::LibraryPathMap;
use super::source::Inventory;

/// Lease duration for an active run owner; heartbeats happen per batch.
const LEASE_SECS: i64 = 120;

/// Creates a `planned` run from an inventory. No target domain data is
/// written by planning (ADR-0148 §D.6).
///
/// # Errors
/// Returns the database error.
pub async fn record_plan(
    target: &PgPool,
    inventory: &Inventory,
    maps: &[LibraryPathMap],
    source_identity: &str,
    source_snapshot_marker: &str,
    _json: bool,
) -> Result<Uuid, String> {
    let run_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    let notes = serde_json::json!({
        "plan": {
            "schema_fingerprint": inventory.schema_fingerprint,
            "tables": inventory.tables.iter().map(|t| serde_json::json!({
                "domain": t.domain, "rows": t.rows, "max_pk": t.max_pk,
            })).collect::<Vec<_>>(),
            "anomalies": inventory.anomalies,
            "media_file_count": inventory.media_file_count,
            "total_media_bytes": inventory.total_media_bytes,
            "library_path_maps": maps.iter().map(|m| serde_json::json!({
                "legacy_path": m.legacy_path, "relative_prefix": m.relative_prefix,
            })).collect::<Vec<_>>(),
        }
    });
    sqlx::query(
        "INSERT INTO legacy_migration.runs (
            id, state, source_identity, source_snapshot_marker,
            owner, lease_expires_at, notes, created_at, updated_at
         ) VALUES ($1, 'planned', $2, $3, NULL, NULL, $4, $5, $5)",
    )
    .bind(run_id)
    .bind(source_identity)
    .bind(source_snapshot_marker)
    .bind(notes)
    .bind(now)
    .execute(target)
    .await
    .map_err(|e| format!("cannot record planned run: {e}"))?;
    Ok(run_id)
}

/// Claims or re-claims a run for `owner`. Fails (returns `Ok(false)`) when a
/// different live owner holds the lease or the run is terminal; a stale lease
/// is reclaimable, which is what makes restart after process death safe.
///
/// # Errors
/// Returns the database error, or a message when the run does not exist.
pub async fn claim_run(target: &PgPool, run_id: Uuid, owner: &str) -> Result<bool, String> {
    let claimed = sqlx::query_scalar::<_, bool>(
        "UPDATE legacy_migration.runs
            SET owner = $2,
                lease_expires_at = now() + make_interval(secs => $3),
                updated_at = now()
          WHERE id = $1
            AND state NOT IN ('complete', 'failed', 'cancelled')
            AND (owner IS NULL OR owner = $2 OR lease_expires_at < now())
          RETURNING true",
    )
    .bind(run_id)
    .bind(owner)
    .bind(LEASE_SECS)
    .fetch_optional(target)
    .await
    .map_err(|e| format!("cannot claim run {run_id}: {e}"))?;
    if claimed.is_some() {
        return Ok(true);
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM legacy_migration.runs WHERE id = $1)")
            .bind(run_id)
            .fetch_one(target)
            .await
            .map_err(|e| format!("cannot check run {run_id}: {e}"))?;
    if !exists {
        return Err(format!("run {run_id} does not exist"));
    }
    Ok(false)
}

/// Releases the lease after a clean command completion. A crash never calls
/// this — the lease expires naturally — so a released lease is the precise
/// signal that the previous process finished its pass (ADR-0148 §D.6).
///
/// # Errors
/// Returns the database error.
pub async fn release_lease(target: &PgPool, run_id: Uuid, owner: &str) -> Result<(), String> {
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET owner = NULL, lease_expires_at = NULL, updated_at = now()
          WHERE id = $1 AND owner = $2",
    )
    .bind(run_id)
    .bind(owner)
    .execute(target)
    .await
    .map_err(|e| format!("cannot release run {run_id}: {e}"))?;
    Ok(())
}

/// Transitions a run's state, fenced by the current live owner. Returns
/// `Ok(false)` when the caller's lease is no longer authoritative — the batch
/// loop must stop rather than commit further work (ADR-0148 §D.6).
///
/// # Errors
/// Returns the database error.
pub async fn transition(
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    from: &[&str],
    to: &str,
) -> Result<bool, String> {
    let changed = sqlx::query_scalar::<_, bool>(
        "UPDATE legacy_migration.runs
            SET state = $4, updated_at = now(),
                completed_at = CASE WHEN $4 = 'complete' THEN now() ELSE completed_at END
          WHERE id = $1 AND owner = $2 AND lease_expires_at > now()
            AND state = ANY($3)
          RETURNING true",
    )
    .bind(run_id)
    .bind(owner)
    .bind(from)
    .bind(to)
    .fetch_optional(target)
    .await
    .map_err(|e| format!("cannot transition run {run_id} to {to}: {e}"))?;
    Ok(changed.is_some())
}

/// Fencing check usable inside a batch transaction: the batch commits only
/// while the caller still holds the live lease.
///
/// # Errors
/// Returns the database error.
pub async fn is_fenced_in(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    owner: &str,
) -> Result<bool, String> {
    sqlx::query_scalar::<_, bool>(
        "UPDATE legacy_migration.runs
            SET lease_expires_at = now() + make_interval(secs => $3), updated_at = now()
          WHERE id = $1 AND owner = $2 AND lease_expires_at > now()
            AND state IN ('planned', 'importing', 'verifying', 'delta-ready')
          RETURNING true",
    )
    .bind(run_id)
    .bind(owner)
    .bind(LEASE_SECS)
    .fetch_optional(&mut **tx)
    .await
    .map(|v| v.unwrap_or(false))
    .map_err(|e| format!("fencing check failed for run {run_id}: {e}"))
}

/// Advances the per-domain checkpoint inside the batch transaction.
///
/// # Errors
/// Returns the database error.
#[allow(clippy::too_many_arguments)]
pub async fn advance_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    domain: &str,
    last_source_pk: i64,
    processed: i64,
    inserted: i64,
    updated: i64,
    issues: i64,
    elapsed_ms: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO legacy_migration.checkpoints (
            run_id, domain, last_source_pk, processed, inserted, updated,
            issues, elapsed_ms, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (run_id, domain) DO UPDATE SET
            last_source_pk = GREATEST(checkpoints.last_source_pk, EXCLUDED.last_source_pk),
            processed = checkpoints.processed + EXCLUDED.processed,
            inserted = checkpoints.inserted + EXCLUDED.inserted,
            updated = checkpoints.updated + EXCLUDED.updated,
            issues = checkpoints.issues + EXCLUDED.issues,
            elapsed_ms = checkpoints.elapsed_ms + EXCLUDED.elapsed_ms,
            updated_at = now()",
    )
    .bind(run_id)
    .bind(domain)
    .bind(last_source_pk)
    .bind(processed)
    .bind(inserted)
    .bind(updated)
    .bind(issues)
    .bind(elapsed_ms)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("checkpoint advance failed for {domain}: {e}"))?;
    Ok(())
}

/// Reads the current checkpoint key for idempotent resume.
///
/// # Errors
/// Returns the database error.
pub async fn checkpoint_key(target: &PgPool, run_id: Uuid, domain: &str) -> Result<i64, String> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT last_source_pk FROM legacy_migration.checkpoints
          WHERE run_id = $1 AND domain = $2",
    )
    .bind(run_id)
    .bind(domain)
    .fetch_optional(target)
    .await
    .map(|v| v.flatten().unwrap_or(0))
    .map_err(|e| format!("cannot read checkpoint for {domain}: {e}"))
}

/// Records a durable issue; batch loops count issues into their checkpoint.
///
/// # Errors
/// Returns the database error.
#[allow(clippy::too_many_arguments)]
pub async fn record_issue(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    domain: &str,
    source_pk: Option<i64>,
    severity: &str,
    code: &str,
    message: &str,
    detail: Option<serde_json::Value>,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO legacy_migration.issues (
            id, run_id, domain, source_pk, severity, code, message, detail, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())",
    )
    .bind(Uuid::now_v7())
    .bind(run_id)
    .bind(domain)
    .bind(source_pk)
    .bind(severity)
    .bind(code)
    .bind(message)
    .bind(detail)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("cannot record issue {code}: {e}"))?;
    Ok(())
}

/// Prints run status: state, per-domain checkpoints, issue totals.
///
/// # Errors
/// Returns the database error or a message when a named run is missing.
pub async fn status(target: &PgPool, run: Option<Uuid>, json: bool) -> Result<(), String> {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct RunRow {
        id: Uuid,
        state: String,
        owner: Option<String>,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
        completed_at: Option<OffsetDateTime>,
    }
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct CheckpointRow {
        domain: String,
        last_source_pk: i64,
        processed: i64,
        inserted: i64,
        updated: i64,
        issues: i64,
        elapsed_ms: i64,
    }

    let runs: Vec<RunRow> = match run {
        Some(run_id) => sqlx::query_as(
            "SELECT id, state, owner, created_at, updated_at, completed_at
               FROM legacy_migration.runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_all(target)
        .await
        .map_err(|e| format!("cannot read run {run_id}: {e}"))?,
        None => sqlx::query_as(
            "SELECT id, state, owner, created_at, updated_at, completed_at
               FROM legacy_migration.runs ORDER BY created_at DESC LIMIT 20",
        )
        .fetch_all(target)
        .await
        .map_err(|e| format!("cannot list runs: {e}"))?,
    };
    if runs.is_empty() {
        return Err(match run {
            Some(run_id) => format!("run {run_id} does not exist"),
            None => "no migration runs recorded".to_string(),
        });
    }

    if json {
        let mut out = Vec::with_capacity(runs.len());
        for row in &runs {
            let checkpoints: Vec<CheckpointRow> = sqlx::query_as(
                "SELECT domain, last_source_pk, processed, inserted, updated, issues, elapsed_ms
                   FROM legacy_migration.checkpoints WHERE run_id = $1 ORDER BY domain",
            )
            .bind(row.id)
            .fetch_all(target)
            .await
            .map_err(|e| format!("cannot read checkpoints: {e}"))?;
            out.push(serde_json::json!({ "run": row, "checkpoints": checkpoints }));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
        );
        return Ok(());
    }

    for row in &runs {
        println!(
            "run {}  state={}  owner={}  updated={}",
            row.id,
            row.state,
            row.owner.as_deref().unwrap_or("-"),
            row.updated_at
        );
        let checkpoints: Vec<CheckpointRow> = sqlx::query_as(
            "SELECT domain, last_source_pk, processed, inserted, updated, issues, elapsed_ms
               FROM legacy_migration.checkpoints WHERE run_id = $1 ORDER BY domain",
        )
        .bind(row.id)
        .fetch_all(target)
        .await
        .map_err(|e| format!("cannot read checkpoints: {e}"))?;
        if checkpoints.is_empty() {
            println!("  (no checkpoints)");
        }
        for cp in &checkpoints {
            println!(
                "  {:<22} pk@{:<10} processed={:<8} inserted={:<8} updated={:<6} issues={:<4} {}ms",
                cp.domain,
                cp.last_source_pk,
                cp.processed,
                cp.inserted,
                cp.updated,
                cp.issues,
                cp.elapsed_ms
            );
        }
    }
    Ok(())
}
