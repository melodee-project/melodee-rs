//! `legacy delta`: convergent incremental pass over rows changed since the
//! recorded mapping (ADR-0148 §D.5, R3.7). Authorized field updates only;
//! source deletions are reported issues, never destructive target changes.
//!
//! Source and target are separate databases, so every comparison is
//! two-step: fetch source row keys/markers from the read-only source, fetch
//! mappings from the target, and diff in memory with bounded batches.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::LibraryPathMap;
use super::runs;
use super::source::SourceDb;

/// Applies one delta pass. Requires the run to be `delta-ready` (verified)
/// and leaves it `delta-ready` on success. A delta over an unchanged source
/// is a convergent no-op.
///
/// # Errors
/// Returns fencing/source/target errors; row-level anomalies are issues.
#[allow(clippy::too_many_arguments)]
pub async fn apply_delta(
    source: &SourceDb,
    target: &PgPool,
    run: Option<Uuid>,
    batch_size: i64,
    maps: &[LibraryPathMap],
    media_validation: Option<&super::media::CanonicalRootValidator>,
    release_lease: bool,
    legacy_secret_key: Option<&str>,
) -> Result<(), String> {
    let Some(run_id) = run else {
        return Err("delta requires --run <uuid> of the verified import run".to_string());
    };
    let owner = super::default_owner();
    if !runs::claim_run(target, run_id, &owner).await? {
        return Err(format!(
            "run {run_id} is held by another live owner or is terminal"
        ));
    }
    let state: String = sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(target)
        .await
        .map_err(|e| format!("cannot read run {run_id}: {e}"))?;
    if state != "delta-ready" {
        // Handled rejection: release the fresh claim before exiting.
        runs::release_lease(target, run_id, &owner).await?;
        return Err(format!(
            "run {run_id} is {state}; delta requires a verified (delta-ready) run"
        ));
    }

    // 1. New rows beyond each domain checkpoint are imported by re-running
    //    the idempotent import loops; mapped rows are skipped. The delta
    //    keeps the lease for its own fenced passes and releases at the end.
    super::import::import_all(
        source,
        target,
        Some(run_id),
        batch_size,
        maps,
        false,
        media_validation,
        legacy_secret_key,
    )
    .await?;

    // 2. Changed catalog rows: source LastUpdatedAt newer than the import
    //    time recorded on the mapping. Authorized updates are display fields
    //    only; identity (public IDs, relationships) is immutable post-import.
    let artists_updated = delta_artists(source, target, run_id, &owner, batch_size).await?;
    let releases_updated = delta_releases(source, target, run_id, &owner, batch_size).await?;

    // 3. Deletions: mapped source rows that no longer exist upstream become
    //    reconciliation issues; the target is never destructively mutated.
    let mut deletion_issues = 0_i64;
    for (entity, source_table) in [
        ("artist", "Artists"),
        ("release", "Albums"),
        ("track", "Songs"),
    ] {
        deletion_issues +=
            report_deletions(source, target, run_id, &owner, entity, source_table).await?;
    }

    if release_lease {
        runs::release_lease(target, run_id, &owner).await?;
    }
    if release_lease {
        println!(
            "run {run_id}: delta applied (artists updated={artists_updated}, \
             releases updated={releases_updated}, deletion issues={deletion_issues}); \
             state remains delta-ready"
        );
    }
    Ok(())
}

async fn delta_artists(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<i64, String> {
    let mut updated_total = 0_i64;
    let mut key = 0_i64;
    loop {
        let mappings: Vec<(i64, Uuid, time::OffsetDateTime)> = sqlx::query_as(
            "SELECT legacy_numeric_id, internal_id, imported_at
               FROM legacy_catalog_id_mappings
              WHERE migration_run_id = $1 AND entity_type = 'artist'
                AND legacy_numeric_id > $2
              ORDER BY legacy_numeric_id LIMIT $3",
        )
        .bind(run_id)
        .bind(key)
        .bind(batch_size)
        .fetch_all(target)
        .await
        .map_err(|e| format!("delta artist mapping scan failed: {e}"))?;
        if mappings.is_empty() {
            break;
        }
        key = mappings.last().map_or(key, |m| m.0);
        let pks: Vec<i64> = mappings.iter().map(|m| m.0).collect();
        let rows = sqlx::query(
            r#"SELECT "Id", "Name", "SortName", "LastUpdatedAt" FROM "Artists"
               WHERE "Id" = ANY($1)"#,
        )
        .bind(&pks)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("delta artist source read failed: {e}"))?;

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during delta"));
        }
        for row in &rows {
            let pk: i64 = i64::from(row.get::<i32, _>("Id"));
            let source_updated: Option<time::OffsetDateTime> = row.get("LastUpdatedAt");
            let Some((_, target_id, imported_at)) = mappings.iter().find(|m| m.0 == pk) else {
                continue;
            };
            let Some(source_updated) = source_updated else {
                continue;
            };
            if source_updated <= *imported_at {
                continue;
            }
            let name: String = row.get("Name");
            let sort = row
                .get::<Option<String>, _>("SortName")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| name.clone());
            sqlx::query(
                "UPDATE artists SET canonical_name = $2, sort_name = $3,
                    normalized_name = lower(btrim($2)), updated_at = now(),
                    revision = revision + 1
                 WHERE id = $1",
            )
            .bind(target_id)
            .bind(&name)
            .bind(&sort)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("delta artist update failed for {pk}: {e}"))?;
            updated_total += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "delta.artists",
            key,
            0,
            0,
            updated_total,
            0,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        if i64::try_from(mappings.len()).unwrap_or(0) < batch_size {
            break;
        }
    }
    Ok(updated_total)
}

async fn delta_releases(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<i64, String> {
    let mut updated_total = 0_i64;
    let mut key = 0_i64;
    loop {
        let mappings: Vec<(i64, Uuid, time::OffsetDateTime)> = sqlx::query_as(
            "SELECT legacy_numeric_id, internal_id, imported_at
               FROM legacy_catalog_id_mappings
              WHERE migration_run_id = $1 AND entity_type = 'release'
                AND legacy_numeric_id > $2
              ORDER BY legacy_numeric_id LIMIT $3",
        )
        .bind(run_id)
        .bind(key)
        .bind(batch_size)
        .fetch_all(target)
        .await
        .map_err(|e| format!("delta release mapping scan failed: {e}"))?;
        if mappings.is_empty() {
            break;
        }
        key = mappings.last().map_or(key, |m| m.0);
        let pks: Vec<i64> = mappings.iter().map(|m| m.0).collect();
        let rows = sqlx::query(
            r#"SELECT "Id", "Name", "SortName", "ReleaseDate",
                      "OriginalReleaseDate", "LastUpdatedAt"
               FROM "Albums" WHERE "Id" = ANY($1)"#,
        )
        .bind(&pks)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("delta release source read failed: {e}"))?;

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during delta"));
        }
        for row in &rows {
            let pk: i64 = i64::from(row.get::<i32, _>("Id"));
            let source_updated: Option<time::OffsetDateTime> = row.get("LastUpdatedAt");
            let Some((_, target_id, imported_at)) = mappings.iter().find(|m| m.0 == pk) else {
                continue;
            };
            let Some(source_updated) = source_updated else {
                continue;
            };
            if source_updated <= *imported_at {
                continue;
            }
            let title: String = row.get("Name");
            let sort = row
                .get::<Option<String>, _>("SortName")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| title.clone());
            sqlx::query(
                "UPDATE releases SET title = $2, sort_title = $3,
                    normalized_title = lower(btrim($2)), release_date = $4,
                    original_release_date = $5, updated_at = now(),
                    revision = revision + 1
                 WHERE id = $1",
            )
            .bind(target_id)
            .bind(&title)
            .bind(&sort)
            .bind(row.get::<Option<time::Date>, _>("ReleaseDate"))
            .bind(row.get::<Option<time::Date>, _>("OriginalReleaseDate"))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("delta release update failed for {pk}: {e}"))?;
            updated_total += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "delta.releases",
            key,
            0,
            0,
            updated_total,
            0,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        if i64::try_from(mappings.len()).unwrap_or(0) < batch_size {
            break;
        }
    }
    Ok(updated_total)
}

/// Reports mapped rows whose source row no longer exists as reconciliation
/// issues. Two-step across the databases: source keys paged from the source,
/// mapping keys paged from the target, diff in memory.
async fn report_deletions(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    entity: &str,
    source_table: &str,
) -> Result<i64, String> {
    let mut issues = 0_i64;
    let mut key = 0_i64;
    loop {
        let mappings: Vec<i64> = sqlx::query_scalar(
            "SELECT legacy_numeric_id FROM legacy_catalog_id_mappings
              WHERE migration_run_id = $1 AND entity_type = $2
                AND legacy_numeric_id > $3
              ORDER BY legacy_numeric_id LIMIT 1000",
        )
        .bind(run_id)
        .bind(entity)
        .bind(key)
        .fetch_all(target)
        .await
        .map_err(|e| format!("deletion scan failed for {entity}: {e}"))?;
        if mappings.is_empty() {
            break;
        }
        key = mappings.last().copied().unwrap_or(key);
        // Source table names come from the hardcoded deletion-domain
        // allowlist in apply_delta; only key arrays are bound parameters.
        let present: Vec<i64> = sqlx::query_scalar::<_, i32>(sqlx::AssertSqlSafe(format!(
            r#"SELECT "Id" FROM "{source_table}" WHERE "Id" = ANY($1)"#
        )))
        .bind(&mappings)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("deletion source read failed for {entity}: {e}"))?
        .into_iter()
        .map(i64::from)
        .collect();
        let missing: Vec<i64> = mappings
            .iter()
            .copied()
            .filter(|pk| !present.contains(pk))
            .collect();
        if !missing.is_empty() {
            let mut tx = target.begin().await.map_err(|e| e.to_string())?;
            if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
                return Err(format!(
                    "run {run_id}: lost lease during deletion reconciliation"
                ));
            }
            for pk in &missing {
                runs::record_issue(
                    &mut tx, run_id, "delta", Some(*pk), "warning",
                    "source.row_deleted",
                    "source row deleted after import; target retained (no destructive reconciliation)",
                    Some(serde_json::json!({ "entity": entity, "source_table": source_table })),
                )
                .await?;
                issues += 1;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        if mappings.len() < 1000 {
            break;
        }
    }
    Ok(issues)
}
