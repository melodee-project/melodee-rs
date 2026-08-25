//! Read-only validation and explicitly confirmed metadata repair for migrated
//! canonical media (ADR-0148/ADR-0149).

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{media, runs};

const MAX_REPAIR_ROWS: i64 = 100_000;

#[derive(Debug, Clone)]
struct MediaRepair {
    asset_id: Uuid,
    relative_path: String,
    expected_size: i64,
    actual_size: i64,
}

/// Validates only media assets mapped by the named migration run. By default
/// this is read-only. `--fix --confirm` changes only target `byte_size`
/// metadata after a no-follow regular-file observation; it never changes a
/// path, hash, identity, relationship, or source media bytes.
pub async fn validate(
    target: &PgPool,
    run_id: Uuid,
    canonical_root: &std::path::Path,
    fix: bool,
    confirm: bool,
    json: bool,
    actor: &str,
) -> Result<(), String> {
    if fix && !confirm {
        return Err("--fix requires --confirm; run without --fix first to review the deterministic repair plan".to_string());
    }
    if actor.trim().is_empty() {
        return Err("--actor must not be blank".to_string());
    }
    let root = media::CanonicalRootValidator::new(canonical_root)?;
    let state: String = sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(target)
        .await
        .map_err(|e| format!("cannot inspect migration run {run_id}: {e}"))?
        .ok_or_else(|| format!("migration run {run_id} does not exist"))?;
    if !matches!(
        state.as_str(),
        "importing" | "verifying" | "delta-ready" | "cutover-ready" | "complete"
    ) {
        return Err(format!(
            "run {run_id} is {state}; validation requires an imported migration run"
        ));
    }
    if fix && state != "delta-ready" {
        return Err("metadata repair requires a delta-ready migration run; complete the read-only validation first and repair before cutover".to_string());
    }

    let rows = sqlx::query(
        "SELECT DISTINCT ma.id, ma.canonical_relative_path, ma.byte_size
           FROM legacy_catalog_id_mappings mapping
           JOIN tracks t ON t.id = mapping.internal_id
           JOIN media_assets ma ON ma.id = t.playable_media_asset_id
          WHERE mapping.migration_run_id = $1
            AND mapping.entity_type = 'track'
          ORDER BY ma.canonical_relative_path, ma.id
          LIMIT $2",
    )
    .bind(run_id)
    .bind(MAX_REPAIR_ROWS)
    .fetch_all(target)
    .await
    .map_err(|e| format!("cannot read migrated media for run {run_id}: {e}"))?;

    if i64::try_from(rows.len()).unwrap_or(i64::MAX) >= MAX_REPAIR_ROWS {
        return Err(format!(
            "refusing validation: mapped media count reached the safety limit of {MAX_REPAIR_ROWS}; narrow the target run before repair"
        ));
    }

    let mut checked = 0_i64;
    let mut valid = 0_i64;
    let mut missing = 0_i64;
    let mut size_mismatch = 0_i64;
    let mut unsafe_paths = 0_i64;
    let mut repairs = Vec::new();
    for row in rows {
        checked += 1;
        let asset_id: Uuid = row.get("id");
        let path: String = row.get("canonical_relative_path");
        let expected_size: i64 = row.get("byte_size");
        let observation = root.inspect(&path, Some(expected_size));
        match observation.check {
            media::MediaCheck::Ok => valid += 1,
            media::MediaCheck::Missing => missing += 1,
            media::MediaCheck::SizeMismatch => {
                size_mismatch += 1;
                if let Some(actual_size) = observation.actual_size {
                    repairs.push(MediaRepair {
                        asset_id,
                        relative_path: path,
                        expected_size,
                        actual_size,
                    });
                }
            }
            media::MediaCheck::Unsafe => unsafe_paths += 1,
        }
    }

    let plan = serde_json::json!({
        "schema_version": 1,
        "kind": "melodee.legacy-media-validation",
        "run_id": run_id,
        "state": state,
        "checked": checked,
        "valid": valid,
        "missing": missing,
        "size_mismatch": size_mismatch,
        "unsafe": unsafe_paths,
        "repair_plan": repairs.iter().map(|repair| serde_json::json!({
            "asset_id": repair.asset_id,
            "action": "update_target_byte_size",
            "expected_size": repair.expected_size,
            "observed_size": repair.actual_size,
            "path_sha256": path_digest(&repair.relative_path),
        })).collect::<Vec<_>>(),
        "applied": false,
    });

    if unsafe_paths > 0 {
        render(&plan, json);
        return Err("validation found unsafe media paths; refusing repair".to_string());
    }
    if !fix {
        render(&plan, json);
        return Ok(());
    }

    let owner = format!("{actor}:{}", std::process::id());
    if !runs::claim_run(target, run_id, &owner).await? {
        return Err(format!(
            "run {run_id} is held by another live owner or is terminal"
        ));
    }
    let result = apply_repairs(target, run_id, &owner, &repairs, &plan).await;
    let release = runs::release_lease(target, run_id, &owner).await;
    release?;
    let applied = result?;
    let mut applied_plan = plan;
    applied_plan["applied"] = serde_json::Value::Bool(true);
    applied_plan["applied_rows"] = serde_json::Value::from(applied);
    render(&applied_plan, json);
    Ok(())
}

async fn apply_repairs(
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    repairs: &[MediaRepair],
    plan: &serde_json::Value,
) -> Result<i64, String> {
    let mut tx = target.begin().await.map_err(|e| e.to_string())?;
    if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
        return Err(format!("run {run_id}: lost lease before metadata repair"));
    }
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET notes = COALESCE(notes, '{}'::jsonb) || jsonb_build_object('media_validation', $2::jsonb),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(plan)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("cannot record media repair plan: {e}"))?;

    let mut applied = 0_i64;
    for repair in repairs {
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during metadata repair"));
        }
        let changed = sqlx::query(
            "UPDATE media_assets
                SET byte_size = $2
              WHERE id = $1
                AND canonical_relative_path = $3
                AND byte_size = $4",
        )
        .bind(repair.asset_id)
        .bind(repair.actual_size)
        .bind(&repair.relative_path)
        .bind(repair.expected_size)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("cannot repair media asset {}: {e}", repair.asset_id))?
        .rows_affected();
        if changed == 1 {
            runs::record_issue(
                &mut tx,
                run_id,
                "validate",
                None,
                "info",
                "media.repair_applied",
                "updated target media byte-size metadata from a validated regular file",
                Some(serde_json::json!({
                    "asset_id": repair.asset_id,
                    "observed_size": repair.actual_size,
                    "path_sha256": path_digest(&repair.relative_path),
                    "actor": owner,
                })),
            )
            .await?;
            applied += 1;
        } else {
            runs::record_issue(
                &mut tx,
                run_id,
                "validate",
                None,
                "warning",
                "media.repair_conflict",
                "target media metadata changed after the repair plan; no mutation was applied",
                Some(serde_json::json!({
                    "asset_id": repair.asset_id,
                    "path_sha256": path_digest(&repair.relative_path),
                })),
            )
            .await?;
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(applied)
}

fn path_digest(path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn render(plan: &serde_json::Value, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(plan).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("legacy media validation run {}:", plan["run_id"]);
        println!("  checked: {}", plan["checked"]);
        println!("  valid: {}", plan["valid"]);
        println!("  missing: {}", plan["missing"]);
        println!("  size mismatches: {}", plan["size_mismatch"]);
        println!("  unsafe paths: {}", plan["unsafe"]);
        println!(
            "  repair rows: {}",
            plan["repair_plan"].as_array().map_or(0, Vec::len)
        );
        println!("  applied: {}", plan["applied"]);
    }
}
