//! `legacy baseline` (ADR-0149 §D.1): captures the immutable rehearsal
//! baseline — source identity, schema fingerprint, snapshot marker,
//! per-domain counts, media totals, and a redacted configuration fingerprint
//! (setting keys only, never values) — and records it durably into the run's
//! `notes` when `--run` is given.

use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use super::runs;
use super::source::{Inventory, SourceDb};

/// Builds the versioned baseline JSON from an inventory plus source markers.
/// Non-secret setting values are represented by a digest; secret values are
/// represented only by their key and a redaction flag. This keeps the artifact
/// useful for comparing snapshots without turning it into a configuration dump.
pub async fn capture(
    source: &SourceDb,
    inventory: &Inventory,
) -> Result<serde_json::Value, String> {
    let identity = source.source_identity().await?;
    let marker = source.snapshot_marker().await?;
    let read_only = source.read_only_status().await?;
    let settings: Vec<(String, Option<String>)> = sqlx::query(
        r#"SELECT "Key", "Value" FROM "Settings"
           WHERE "Key" IS NOT NULL ORDER BY "Key""#,
    )
    .fetch_all(source.pool())
    .await
    .map_err(|e| format!("configuration fingerprint failed: {e}"))?
    .into_iter()
    .map(|row| {
        Ok::<_, sqlx::Error>((
            row.try_get::<String, _>(0)?,
            row.try_get::<Option<String>, _>(1)?,
        ))
    })
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("configuration fingerprint row failed: {e}"))?;

    let mut settings_keys = Vec::with_capacity(settings.len());
    let mut setting_fingerprints = Vec::with_capacity(settings.len());
    for (key, value) in settings {
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        settings_keys.push(key.clone());
        if is_secret_setting(&key) {
            setting_fingerprints.push(serde_json::json!({
                "key": key,
                "redacted": true,
            }));
        } else {
            let digest = Sha256::digest(value.unwrap_or_default().as_bytes());
            setting_fingerprints.push(serde_json::json!({
                "key": key,
                "sha256": hex_digest(&digest),
            }));
        }
    }

    let library_roots: i64 =
        sqlx::query(r#"SELECT COUNT(*) FROM "Libraries" WHERE "Path" IS NOT NULL"#)
            .fetch_one(source.pool())
            .await
            .map(|r| r.get::<i64, _>(0))
            .map_err(|e| format!("library root count failed: {e}"))?;

    Ok(serde_json::json!({
        "artifact": "melodee.legacy-baseline",
        "schema_version": 1,
        "captured_at": time::OffsetDateTime::now_utc().to_string(),
        "source_identity": identity,
        "schema_fingerprint": inventory.schema_fingerprint,
        "table_count": inventory.table_count,
        "snapshot_marker": marker,
        "source_read_only": read_only,
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
            "algorithm": "sha256-v1",
            "settings_keys": settings_keys,
            "settings": setting_fingerprints,
            "library_root_count": library_roots,
        },
    }))
}

fn hex_digest(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_secret_setting(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "privatekey",
        "private_key",
        "apikey",
        "api_key",
        "credential",
    ]
    .iter()
    .any(|marker| key.contains(marker))
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
        let expected_marker: String = sqlx::query_scalar(
            "SELECT source_snapshot_marker FROM legacy_migration.runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| format!("cannot read run {run_id} snapshot marker: {e}"))?;
        let actual_marker = baseline["snapshot_marker"]
            .as_str()
            .ok_or_else(|| "baseline snapshot marker is not a string".to_string())?;
        if expected_marker != actual_marker {
            tx.rollback().await.map_err(|e| e.to_string())?;
            runs::release_lease(target, run_id, &owner).await?;
            return Err(format!(
                "source snapshot marker mismatch for run {run_id}: planned {expected_marker:?}, current {actual_marker:?}; create a new plan from the approved snapshot"
            ));
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
