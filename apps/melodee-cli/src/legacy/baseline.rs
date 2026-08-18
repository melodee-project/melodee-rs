//! `legacy baseline` (ADR-0149 §D.1, R4.1): captures the immutable rehearsal
//! baseline — source identity, schema fingerprint, snapshot marker,
//! per-domain counts, media totals, and a redacted configuration fingerprint
//! (setting keys only, never values) — and records it durably into the run's
//! `notes` when `--run` is given.

use sqlx::Row;
use uuid::Uuid;

use super::runs;
use super::source::{Inventory, SourceDb};

/// Builds the stable baseline JSON from an inventory plus the source markers.
/// Setting values are never read — only key names and categories, so the
/// fingerprint cannot leak credentials.
pub async fn capture(
    source: &SourceDb,
    inventory: &Inventory,
) -> Result<serde_json::Value, String> {
    let identity = source.source_identity().await?;
    let marker = source.snapshot_marker().await?;
    // Legacy Settings keys only. The value column is deliberately never
    // selected: a baseline that hashes or copies values could leak secrets.
    let mut settings_keys: Vec<String> = sqlx::query_scalar(
        r#"SELECT "Key" FROM "Settings" WHERE "Key" IS NOT NULL ORDER BY "Key""#,
    )
    .fetch_all(source.pool())
    .await
    .map_err(|e| format!("settings fingerprint failed: {e}"))?;
    settings_keys.retain(|k| !k.trim().is_empty());

    let library_roots: i64 =
        sqlx::query(r#"SELECT COUNT(*) FROM "Libraries" WHERE "Path" IS NOT NULL"#)
            .fetch_one(source.pool())
            .await
            .map(|r| r.get::<i64, _>(0))
            .map_err(|e| format!("library root count failed: {e}"))?;

    Ok(serde_json::json!({
        "captured_at": time::OffsetDateTime::now_utc().to_string(),
        "source_identity": identity,
        "schema_fingerprint": inventory.schema_fingerprint,
        "table_count": inventory.table_count,
        "snapshot_marker": marker,
        "domains": inventory.tables.iter().map(|t| serde_json::json!({
            "domain": t.domain, "rows": t.rows, "min_pk": t.min_pk, "max_pk": t.max_pk,
        })).collect::<Vec<_>>(),
        "anomalies": inventory.anomalies.iter().map(|a| serde_json::json!({
            "code": a.code, "count": a.count,
        })).collect::<Vec<_>>(),
        "media": {
            "file_count": inventory.media_file_count,
            "total_bytes": inventory.total_media_bytes,
        },
        "configuration_fingerprint": {
            "settings_keys": settings_keys,
            "library_root_count": library_roots,
        },
    }))
}

/// Records the baseline into the run's notes (fenced), or prints it when no
/// run is named.
///
/// # Errors
/// Returns source/target errors or a message when the run does not exist.
pub async fn record_baseline(
    source: &SourceDb,
    target: &sqlx::PgPool,
    run: Option<Uuid>,
    json: bool,
) -> Result<(), String> {
    let inventory = super::source::collect_inventory(source).await?;
    let baseline = capture(source, &inventory).await?;

    if let Some(run_id) = run {
        let owner = super::default_owner();
        if !runs::claim_run(target, run_id, &owner).await? {
            return Err(format!(
                "run {run_id} is held by another live owner or is terminal"
            ));
        }
        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, &owner).await? {
            return Err(format!("run {run_id}: lost lease during baseline record"));
        }
        sqlx::query(
            "UPDATE legacy_migration.runs
                SET notes = COALESCE(notes, '{}'::jsonb) || jsonb_build_object('baseline', $2::jsonb),
                    updated_at = now()
              WHERE id = $1",
        )
        .bind(run_id)
        .bind(serde_json::to_string(&baseline).map_err(|e| e.to_string())?)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("cannot record baseline: {e}"))?;
        tx.commit().await.map_err(|e| e.to_string())?;
        runs::release_lease(target, run_id, &owner).await?;
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&baseline).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("baseline captured:");
        println!("  source identity: {}", baseline["source_identity"]);
        println!("  schema fingerprint: {}", baseline["schema_fingerprint"]);
        println!("  snapshot marker: {}", baseline["snapshot_marker"]);
        println!("  legacy user tables: {}", baseline["table_count"]);
        println!(
            "  media: {} files, {} bytes (metadata only)",
            baseline["media"]["file_count"], baseline["media"]["total_bytes"]
        );
        if let Some(run_id) = run {
            println!("  recorded into run {run_id} notes");
        } else {
            println!("  (not persisted; pass --run to record durably)");
        }
    }
    Ok(())
}
