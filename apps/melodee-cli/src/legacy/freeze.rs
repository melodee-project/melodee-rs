//! `legacy freeze` (ADR-0149 §D.1/D.2, R4.7): proves source write-stillness
//! across the final delta window, re-verifies the gate, records the freeze
//! evidence durably, and — only with `--acknowledge-cutover` — transitions
//! the run to `cutover-ready`.

use sqlx::PgPool;
use uuid::Uuid;

use super::media::CanonicalRootValidator;
use super::runs;
use super::source::SourceDb;
use super::LibraryPathMap;

/// Runs the final freeze pass for a verified run.
///
/// # Errors
/// Returns fencing errors, a source-write-during-freeze abort, a non-clean
/// verification gate, or database errors.
#[allow(clippy::too_many_arguments)]
pub async fn freeze(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    batch_size: i64,
    maps: &[LibraryPathMap],
    media_validation: Option<&CanonicalRootValidator>,
    legacy_secret_key: Option<&str>,
    acknowledge: bool,
    note: Option<&str>,
    json: bool,
) -> Result<(), String> {
    let owner = super::default_owner();
    if !runs::claim_run(target, run_id, &owner).await? {
        return Err(format!("run {run_id} is held by another live owner or is terminal"));
    }
    let state: String = sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(target)
        .await
        .map_err(|e| format!("cannot read run {run_id}: {e}"))?;
    // Early rejections release the claim: the lease exists to fence a live
    // worker, and a process exiting on a handled error is no longer working.
    if state != "delta-ready" {
        runs::release_lease(target, run_id, &owner).await?;
        return Err(format!(
            "run {run_id} is {state}; freeze requires a verified (delta-ready) run"
        ));
    }
    if acknowledge && note.is_none() {
        runs::release_lease(target, run_id, &owner).await?;
        return Err("--acknowledge-cutover requires --note with the operator statement".to_string());
    }

    let started = std::time::Instant::now();
    let marker_before = source.snapshot_marker().await?;

    // Final convergent delta inside the freeze window (ADR-0149 D.1 step 2).
    super::delta::apply_delta(
        source,
        target,
        Some(run_id),
        batch_size,
        maps,
        media_validation,
        false,
        legacy_secret_key,
    )
    .await?;

    let marker_after = source.snapshot_marker().await?;
    if marker_before != marker_after {
        runs::release_lease(target, run_id, &owner).await?;
        return Err(format!(
            "source writes observed during the freeze window ({marker_before} -> {marker_after}); \
             aborting: enforce the write-freeze (ADR-0149 D.2) and re-run freeze"
        ));
    }

    // Final verification gate (step 3): totals only, no output or transition.
    let gate = super::verify::compute_gate(source, target, run_id, 20, media_validation).await?;
    if gate.total_mismatches != 0 || gate.duplicate_public_ids != 0 {
        runs::release_lease(target, run_id, &owner).await?;
        return Err(format!(
            "freeze verification is not clean: {} mismatch gap(s), {} duplicate public id(s); \
             resolve and re-run verify before freeze",
            gate.total_mismatches, gate.duplicate_public_ids
        ));
    }
    let freeze_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

    // Record the freeze evidence durably, fenced (step 4).
    let mut tx = target.begin().await.map_err(|e| e.to_string())?;
    if !runs::is_fenced_in(&mut tx, run_id, &owner).await? {
        return Err(format!("run {run_id}: lost lease during freeze record"));
    }
    let evidence = serde_json::json!({
        "at": time::OffsetDateTime::now_utc().to_string(),
        "snapshot_marker": marker_after,
        "freeze_window_ms": freeze_ms,
        "verification": {
            "total_mismatch_gaps": gate.total_mismatches,
            "duplicate_public_ids": gate.duplicate_public_ids,
        },
        "acknowledgement": note,
    });
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET notes = COALESCE(notes, '{}'::jsonb) || jsonb_build_object('freeze', $2::jsonb),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(serde_json::to_string(&evidence).map_err(|e| e.to_string())?)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("cannot record freeze evidence: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;

    // Step 5: only an explicit operator acknowledgement reaches cutover-ready.
    let mut final_state = "delta-ready";
    if acknowledge {
        if !runs::transition(target, run_id, &owner, &["delta-ready"], "cutover-ready").await? {
            return Err(format!("run {run_id}: lost fencing before cutover-ready transition"));
        }
        final_state = "cutover-ready";
    }
    runs::release_lease(target, run_id, &owner).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "run_id": run_id,
                "state": final_state,
                "freeze": evidence,
            })
        );
    } else {
        println!("run {run_id}: freeze window {freeze_ms} ms, marker {marker_after}");
        println!("run {run_id}: verification clean; state is {final_state}");
    }
    Ok(())
}
