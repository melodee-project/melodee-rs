//! Durable, bounded and resumable inbound-source discovery partitions.

use melodee_persistence::{DiscoveryScanFence, WorkStoreError};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    config::InboundSource,
    error::{IngestionError, Result},
};

const PARTITIONS: i16 = 16;
const BATCH_SIZE: i64 = 128;

fn db(error: sqlx::Error) -> IngestionError {
    IngestionError::Persistence(WorkStoreError::Database(error))
}

pub(crate) async fn acquire(
    pool: &PgPool,
    source: &InboundSource,
    wait: std::time::Duration,
) -> Result<DiscoveryScanFence> {
    tokio::time::timeout(wait, acquire_inner(pool, source))
        .await
        .map_err(|_| {
            IngestionError::Persistence(WorkStoreError::DiscoveryScanBusy(source.name.clone()))
        })?
}

async fn acquire_inner(pool: &PgPool, source: &InboundSource) -> Result<DiscoveryScanFence> {
    let scope_key = source
        .id
        .map_or_else(|| "default".to_owned(), |id| id.to_string());
    let owner = Uuid::now_v7();
    loop {
        let generation = Uuid::now_v7();
        let mut tx = pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO ingestion_discovery_scan (scope_key, generation, owner, lease_expires_at, birth_cutoff) VALUES ($1,$2,$3,clock_timestamp(),clock_timestamp()) ON CONFLICT DO NOTHING")
            .bind(&scope_key).bind(generation).bind(owner).execute(&mut *tx).await.map_err(db)?;
        let (existing, expired, recovery_reset, phase, missing_resume, missing_revalidation) = sqlx::query_as::<_, (Uuid, bool, bool, String, bool, bool)>("SELECT generation, lease_expires_at <= clock_timestamp(), recovery_reset, phase, missing_resume, missing_revalidation FROM ingestion_discovery_scan WHERE scope_key=$1 FOR UPDATE")
            .bind(&scope_key).fetch_one(&mut *tx).await.map_err(db)?;
        if expired {
            // A crash in the middle of a partition invalidates only that
            // partition's observations. Completed partitions remain durable.
            if existing == generation {
                sqlx::query("UPDATE ingestion_discovery_scan SET owner=$2, lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE scope_key=$1")
                    .bind(&scope_key).bind(owner).execute(&mut *tx).await.map_err(db)?;
            } else if recovery_reset {
                // No observation occurs until the global reset completes, so
                // its durable cursor is safe even if the source root changed.
                sqlx::query("UPDATE ingestion_discovery_scan SET owner=$2, lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE scope_key=$1")
                    .bind(&scope_key).bind(owner).execute(&mut *tx).await.map_err(db)?;
            } else if phase == "missing" && missing_resume {
                // The first handoff starts a bounded sweep over all scoped
                // paths. Subsequent owners preserve its cursor and recheck
                // each prospective missing result against their pinned root.
                sqlx::query("UPDATE ingestion_discovery_scan SET owner=$2, lease_expires_at=clock_timestamp()+interval '5 minutes', clear_after=CASE WHEN $3 THEN clear_after ELSE NULL END, missing_revalidation=true WHERE scope_key=$1")
                    .bind(&scope_key).bind(owner).bind(missing_revalidation).execute(&mut *tx).await.map_err(db)?;
            } else {
                // A prior process can no longer pin the original root. Do not
                // trust any old observation until every partition is rescanned.
                // Done markers survive and are revalidated before reuse.
                sqlx::query("UPDATE ingestion_discovery_scan SET owner=$2, lease_expires_at=clock_timestamp()+interval '5 minutes', partition=0, phase='clear', phase_started=false, clear_after=NULL, initial_pass_complete=false, restart_observations=true, recovery_reset=true, missing_resume=false, missing_revalidation=false, missing_retry=false, retry_mask=0 WHERE scope_key=$1")
                    .bind(&scope_key).bind(owner).execute(&mut *tx).await.map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            return Ok(DiscoveryScanFence {
                scope_key,
                generation: existing,
                owner,
            });
        }
        tx.commit().await.map_err(db)?;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

pub(crate) async fn renew(pool: &PgPool, fence: &DiscoveryScanFence) -> Result<()> {
    let count = sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp()+interval '5 minutes' WHERE scope_key=$1 AND generation=$2 AND owner=$3 AND lease_expires_at>clock_timestamp()")
        .bind(&fence.scope_key).bind(fence.generation).bind(fence.owner).execute(pool).await.map_err(db)?.rows_affected();
    if count == 0 {
        return Err(IngestionError::Persistence(
            WorkStoreError::DiscoveryScanLeaseExpired,
        ));
    }
    Ok(())
}

/// Return the next partition, clearing only an interrupted partition in
/// transactionally checkpointed pages. `None` means enumeration is complete.
pub(crate) async fn next_partition(
    pool: &PgPool,
    source: &InboundSource,
    fence: &DiscoveryScanFence,
    bounded_turn: bool,
) -> Result<Option<(u8, bool)>> {
    let mut clear_pages = 0;
    loop {
        renew(pool, fence).await?;
        let mut tx = pool.begin().await.map_err(db)?;
        fence.lock(&mut tx).await?;
        let (partition, phase, after, recovery_reset) = sqlx::query_as::<_, (i16, String, Option<Uuid>, bool)>(
            "SELECT partition, phase, clear_after, recovery_reset FROM ingestion_discovery_scan WHERE scope_key=$1",
        )
        .bind(&fence.scope_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if phase == "missing" {
            tx.commit().await.map_err(db)?;
            return Ok(None);
        }
        if phase == "clear" {
            let bit = 1_i32 << partition;
            let ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM release_work_items WHERE source_library_id IS NOT DISTINCT FROM $1 AND discovery_scan_generation=$2 AND (CASE WHEN $6 THEN discovery_seen_mask <> 0 ELSE (discovery_seen_mask & $3) <> 0 END) AND ($4::uuid IS NULL OR id > $4) ORDER BY id LIMIT $5 FOR UPDATE")
                .bind(source.id).bind(fence.generation).bind(bit).bind(after).bind(BATCH_SIZE).bind(recovery_reset).fetch_all(&mut *tx).await.map_err(db)?;
            if let Some(last) = ids.last() {
                sqlx::query("UPDATE release_work_items SET discovery_seen_mask=CASE WHEN $3 THEN 0 ELSE discovery_seen_mask & ~$2 END WHERE id=ANY($1)")
                    .bind(&ids).bind(bit).bind(recovery_reset).execute(&mut *tx).await.map_err(db)?;
                sqlx::query(
                    "UPDATE ingestion_discovery_scan SET clear_after=$2 WHERE scope_key=$1",
                )
                .bind(&fence.scope_key)
                .bind(last)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
                clear_pages += 1;
                if bounded_turn && clear_pages >= 4 {
                    if recovery_reset {
                        sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp() WHERE scope_key=$1")
                            .bind(&fence.scope_key).execute(&mut *tx).await.map_err(db)?;
                    } else {
                        // The pinned root will be released when yielding. Do
                        // not reuse any old observations under the next owner.
                        sqlx::query("UPDATE ingestion_discovery_scan SET lease_expires_at=clock_timestamp(), recovery_reset=true, partition=0, phase='clear', clear_after=NULL, restart_observations=true, initial_pass_complete=false, retry_mask=0 WHERE scope_key=$1")
                            .bind(&fence.scope_key).execute(&mut *tx).await.map_err(db)?;
                    }
                    tx.commit().await.map_err(db)?;
                    return Err(IngestionError::Persistence(
                        WorkStoreError::DiscoveryScanIncomplete(source.name.clone()),
                    ));
                }
            } else {
                sqlx::query("UPDATE ingestion_discovery_scan SET phase='replay', phase_started=false, clear_after=NULL, recovery_reset=false, restart_observations=CASE WHEN $2 THEN false ELSE restart_observations END WHERE scope_key=$1")
                    .bind(&fence.scope_key).bind(recovery_reset).execute(&mut *tx).await.map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
            continue;
        }
        sqlx::query("UPDATE ingestion_discovery_scan SET phase_started=true WHERE scope_key=$1")
            .bind(&fence.scope_key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        return u8::try_from(partition)
            .map(|value| Some((value, phase == "replay")))
            .map_err(|_| IngestionError::InspectionTask("invalid discovery partition".to_owned()));
    }
}

pub(crate) async fn mark_seen(
    pool: &PgPool,
    source: &InboundSource,
    fence: &DiscoveryScanFence,
    relative: &str,
    partition: u8,
) -> Result<bool> {
    let bit = 1_i32 << partition;
    let mut tx = pool.begin().await.map_err(db)?;
    fence.lock(&mut tx).await?;
    let done = sqlx::query_scalar::<_, bool>("UPDATE release_work_items SET discovery_seen_mask=CASE WHEN discovery_scan_generation=$3 THEN discovery_seen_mask | $4 ELSE $4 END, discovery_done_mask=CASE WHEN discovery_scan_generation=$3 THEN discovery_done_mask ELSE 0 END, discovery_scan_generation=$3 WHERE source_library_id IS NOT DISTINCT FROM $1 AND ingest_occurrence_hash(source_path)=ingest_occurrence_hash($2) AND source_path=$2 AND inbound_active AND (strpos($2,chr(65533))=0 OR discovery_path_format=1) RETURNING (discovery_done_mask & $4) <> 0")
        .bind(source.id).bind(relative).bind(fence.generation).bind(bit).fetch_optional(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(done.unwrap_or(false))
}

pub(crate) async fn mark_legacy_seen(
    pool: &PgPool,
    source: &InboundSource,
    fence: &DiscoveryScanFence,
    relative: &str,
    partition: u8,
) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db)?;
    fence.lock(&mut tx).await?;
    sqlx::query("UPDATE release_work_items SET discovery_seen_mask=CASE WHEN discovery_scan_generation=$3 THEN discovery_seen_mask | $4 ELSE $4 END, discovery_done_mask=CASE WHEN discovery_scan_generation=$3 THEN discovery_done_mask ELSE 0 END, discovery_scan_generation=$3 WHERE source_library_id IS NOT DISTINCT FROM $1 AND ingest_occurrence_hash(source_path)=ingest_occurrence_hash($2) AND source_path=$2 AND inbound_active AND discovery_path_format=0")
        .bind(source.id).bind(relative).bind(fence.generation).bind(1_i32 << partition).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(())
}

pub(crate) async fn mark_done(
    pool: &PgPool,
    source: &InboundSource,
    fence: &DiscoveryScanFence,
    relative: &str,
    partition: u8,
) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db)?;
    fence.lock(&mut tx).await?;
    sqlx::query("UPDATE release_work_items SET discovery_done_mask=discovery_done_mask | $4 WHERE source_library_id IS NOT DISTINCT FROM $1 AND ingest_occurrence_hash(source_path)=ingest_occurrence_hash($2) AND source_path=$2 AND discovery_scan_generation=$3 AND inbound_active AND (strpos($2,chr(65533))=0 OR discovery_path_format=1)")
        .bind(source.id).bind(relative).bind(fence.generation).bind(1_i32 << partition).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(())
}

/// Finish one partition, retaining failed partitions for a later bounded turn.
/// The first pass visits every partition before retrying any of them. Subsequent
/// retry turns rotate across pending partitions so one failure cannot starve its peers.
pub(crate) async fn finish_partition(
    pool: &PgPool,
    fence: &DiscoveryScanFence,
    partition: u8,
    failed: bool,
) -> Result<bool> {
    let mut tx = pool.begin().await.map_err(db)?;
    fence.lock(&mut tx).await?;
    let (retry_mask, initial_complete, restarting) = sqlx::query_as::<_, (i32, bool, bool)>(
        "SELECT retry_mask, initial_pass_complete, restart_observations FROM ingestion_discovery_scan WHERE scope_key=$1",
    )
    .bind(&fence.scope_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    let pending = retry_mask | if failed { 1_i32 << partition } else { 0 };
    let next = i16::from(partition) + 1;
    if !initial_complete && next < PARTITIONS {
        sqlx::query("UPDATE ingestion_discovery_scan SET partition=$2, phase=CASE WHEN $4 THEN 'clear' ELSE 'collect' END, phase_started=false, clear_after=NULL, retry_mask=$3 WHERE scope_key=$1")
            .bind(&fence.scope_key).bind(next).bind(pending).bind(restarting).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        return Ok(false);
    }
    if let Some(retry) = next_retry_partition(pending, partition) {
        let remaining = pending & !(1_i32 << retry);
        sqlx::query("UPDATE ingestion_discovery_scan SET partition=$2, phase='clear', phase_started=false, clear_after=NULL, initial_pass_complete=true, restart_observations=false, retry_mask=$3, lease_expires_at=clock_timestamp()+interval '5 seconds' WHERE scope_key=$1")
            .bind(&fence.scope_key).bind(i16::from(retry)).bind(remaining).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        return Ok(true);
    }
    sqlx::query("UPDATE ingestion_discovery_scan SET partition=$2, phase='missing', phase_started=false, clear_after=NULL, initial_pass_complete=true, restart_observations=false, retry_mask=0 WHERE scope_key=$1")
        .bind(&fence.scope_key).bind(PARTITIONS).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(false)
}

fn next_retry_partition(mask: i32, after: u8) -> Option<u8> {
    (1..=PARTITIONS as u8)
        .map(|offset| (after + offset) % PARTITIONS as u8)
        .find(|partition| mask & (1_i32 << partition) != 0)
}

pub(crate) enum MissingOutcome {
    Complete(u64),
    Reobserve,
}

/// Process a bounded page at a time. After an owner handoff, a durable
/// revalidation sweep checks each representable path against the new pinned
/// root; a final complete observation repairs stale effects from prior owners.
pub(crate) async fn mark_missing(
    pool: &PgPool,
    source: &InboundSource,
    fence: &DiscoveryScanFence,
    root: &crate::source::GuardedRoot,
    now: OffsetDateTime,
    bounded_turn: bool,
) -> Result<MissingOutcome> {
    let mut total = 0;
    let mut pages = 0;
    loop {
        renew(pool, fence).await?;
        let mut tx = pool.begin().await.map_err(db)?;
        fence.lock(&mut tx).await?;
        let (cutoff, after, revalidation, retry) = sqlx::query_as::<_, (OffsetDateTime, Option<Uuid>, bool, bool)>(
            "SELECT birth_cutoff, clear_after, missing_revalidation, missing_retry FROM ingestion_discovery_scan WHERE scope_key=$1",
        ).bind(&fence.scope_key).fetch_one(&mut *tx).await.map_err(db)?;
        let rows = sqlx::query_as::<_, (Uuid, String)>("SELECT id, source_path FROM release_work_items WHERE source_library_id IS NOT DISTINCT FROM $1 AND inbound_active AND lease_owner IS NULL AND state IN ('discovered','settling','ready') AND discovery_birth_at <= $2 AND ($4::uuid IS NULL OR id>$4) AND CASE WHEN $7 THEN discovery_missing_retry_generation=$3 WHEN $6 THEN true ELSE (discovery_scan_generation IS DISTINCT FROM $3 OR discovery_seen_mask=0) AND (discovery_missing_generation IS DISTINCT FROM $3 OR state_reason IS DISTINCT FROM 'source-missing') END ORDER BY id LIMIT $5")
            .bind(source.id).bind(cutoff).bind(fence.generation).bind(after).bind(BATCH_SIZE).bind(revalidation).bind(retry).fetch_all(&mut *tx).await.map_err(db)?;
        if rows.is_empty() {
            let failures = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM release_work_items WHERE source_library_id IS NOT DISTINCT FROM $1 AND discovery_missing_retry_generation=$2 AND inbound_active AND lease_owner IS NULL AND state IN ('discovered','settling','ready'))")
                .bind(source.id).bind(fence.generation).fetch_one(&mut *tx).await.map_err(db)?;
            if failures {
                sqlx::query("UPDATE ingestion_discovery_scan SET missing_resume=true, missing_retry=true, clear_after=NULL, lease_expires_at=clock_timestamp() WHERE scope_key=$1")
                    .bind(&fence.scope_key).execute(&mut *tx).await.map_err(db)?;
                tx.commit().await.map_err(db)?;
                return Err(IngestionError::Persistence(
                    WorkStoreError::DiscoveryScanIncomplete(source.name.clone()),
                ));
            }
            if revalidation {
                // Any earlier absence decision could have raced a root swap.
                // Reobserve once, preserving missing markers so the final
                // missing pass need not repeat the recovery sweep.
                sqlx::query("UPDATE ingestion_discovery_scan SET recovery_reset=true, missing_resume=false, missing_revalidation=false, missing_retry=false, partition=0, phase='clear', phase_started=false, clear_after=NULL, initial_pass_complete=false, restart_observations=true, retry_mask=0, lease_expires_at=clock_timestamp() WHERE scope_key=$1")
                    .bind(&fence.scope_key).execute(&mut *tx).await.map_err(db)?;
                tx.commit().await.map_err(db)?;
                return Ok(MissingOutcome::Reobserve);
            }
            tx.commit().await.map_err(db)?;
            return Ok(MissingOutcome::Complete(total));
        }
        tx.commit().await.map_err(db)?;
        let last = rows.last().map(|(id, _)| *id);
        let verify_root = root.clone();
        let checked = crate::admission::run(move || {
            verify_root.verify_current()?;
            let mut result = Vec::with_capacity(rows.len());
            for (id, path) in rows {
                // Old source_path values are lossy UTF-8. Only a fresh full
                // discovery pass may establish absence for these ambiguous
                // names; a later final pass will handle them.
                if revalidation && path.contains('\u{fffd}') {
                    result.push((id, path, None));
                    continue;
                }
                if revalidation {
                    match crate::source::decode_candidate_relative_path(&path)
                        .and_then(|relative| verify_root.resolve_relative(&relative)) {
                        Ok(candidate) => result.push((id, path, Some(candidate.is_none()))),
                        Err(error) => {
                            tracing::warn!(source_path=%path, %error, "missing-path revalidation deferred");
                            result.push((id, path, None));
                        }
                    }
                } else {
                    result.push((id, path, Some(true)));
                }
            }
            verify_root.verify_current()?;
            Ok::<_, crate::source::SourceDiscoveryError>(result)
        }).await.map_err(IngestionError::InspectionTask)??;
        let verify_root = root.clone();
        crate::admission::run(move || verify_root.verify_current())
            .await
            .map_err(IngestionError::InspectionTask)??;
        let mut absent_ids = Vec::new();
        let mut absent_paths = Vec::new();
        let mut failed_ids = Vec::new();
        let mut failed_paths = Vec::new();
        let mut checked_ids = Vec::new();
        let mut checked_paths = Vec::new();
        for (id, path, absent) in checked {
            match absent {
                Some(true) => {
                    absent_ids.push(id);
                    absent_paths.push(path.clone());
                    checked_ids.push(id);
                    checked_paths.push(path);
                }
                Some(false) => {
                    checked_ids.push(id);
                    checked_paths.push(path);
                }
                None if path.contains('\u{fffd}') => {
                    checked_ids.push(id);
                    checked_paths.push(path);
                } // deferred to final full observation
                None => {
                    failed_ids.push(id);
                    failed_paths.push(path);
                }
            }
        }
        let mut tx = pool.begin().await.map_err(db)?;
        fence.lock(&mut tx).await?;
        if !absent_ids.is_empty() {
            total += sqlx::query("UPDATE release_work_items AS item SET last_observed_at=$3,state_reason='source-missing',updated_at=$3,discovery_missing_generation=$4 FROM unnest($1::uuid[],$2::text[]) AS observation(id,path) WHERE item.id=observation.id AND item.source_path=observation.path AND item.source_library_id IS NOT DISTINCT FROM $5 AND item.inbound_active AND item.lease_owner IS NULL AND item.state IN ('discovered','settling','ready') AND item.discovery_birth_at <= $6 AND ($7 OR item.discovery_scan_generation IS DISTINCT FROM $4 OR item.discovery_seen_mask=0)")
                .bind(&absent_ids).bind(&absent_paths).bind(now).bind(fence.generation).bind(source.id).bind(cutoff).bind(revalidation).execute(&mut *tx).await.map_err(db)?.rows_affected();
        }
        if !checked_ids.is_empty() {
            sqlx::query("UPDATE release_work_items AS item SET discovery_missing_retry_generation=NULL FROM unnest($1::uuid[],$2::text[]) AS observation(id,path) WHERE item.id=observation.id AND item.source_path=observation.path AND item.source_library_id IS NOT DISTINCT FROM $3 AND item.discovery_missing_retry_generation=$4")
                .bind(&checked_ids).bind(&checked_paths).bind(source.id).bind(fence.generation).execute(&mut *tx).await.map_err(db)?;
        }
        if !failed_ids.is_empty() {
            sqlx::query("UPDATE release_work_items AS item SET discovery_missing_retry_generation=$3 FROM unnest($1::uuid[],$2::text[]) AS observation(id,path) WHERE item.id=observation.id AND item.source_path=observation.path AND item.source_library_id IS NOT DISTINCT FROM $4 AND item.inbound_active AND item.lease_owner IS NULL AND item.state IN ('discovered','settling','ready')")
                .bind(&failed_ids).bind(&failed_paths).bind(fence.generation).bind(source.id).execute(&mut *tx).await.map_err(db)?;
        }
        sqlx::query("UPDATE ingestion_discovery_scan SET clear_after=$2 WHERE scope_key=$1")
            .bind(&fence.scope_key)
            .bind(last)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        pages += 1;
        if bounded_turn && pages >= 4 {
            sqlx::query("UPDATE ingestion_discovery_scan SET missing_resume=true,lease_expires_at=clock_timestamp() WHERE scope_key=$1")
                .bind(&fence.scope_key).execute(&mut *tx).await.map_err(db)?;
            tx.commit().await.map_err(db)?;
            return Err(IngestionError::Persistence(
                WorkStoreError::DiscoveryScanIncomplete(source.name.clone()),
            ));
        }
        tx.commit().await.map_err(db)?;
    }
}

pub(crate) async fn finish(pool: &PgPool, fence: &DiscoveryScanFence) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db)?;
    fence.lock(&mut tx).await?;
    sqlx::query(
        "DELETE FROM ingestion_discovery_scan WHERE scope_key=$1 AND generation=$2 AND owner=$3",
    )
    .bind(&fence.scope_key)
    .bind(fence.generation)
    .bind(fence.owner)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(())
}

/// Discard one expired scan belonging to a source no longer configured.
pub(crate) async fn cleanup(pool: &PgPool, sources: &[InboundSource]) -> Result<()> {
    let scopes: Vec<String> = sources
        .iter()
        .map(|source| {
            source
                .id
                .map_or_else(|| "default".to_owned(), |id| id.to_string())
        })
        .collect();
    sqlx::query("DELETE FROM ingestion_discovery_scan WHERE scope_key IN (SELECT scope_key FROM ingestion_discovery_scan scan WHERE lease_expires_at<=clock_timestamp() AND NOT (scope_key=ANY($1)) AND NOT EXISTS (SELECT 1 FROM release_work_item_settling_snapshots snapshot WHERE snapshot.capture_scope=scan.scope_key AND NOT snapshot.capture_complete) ORDER BY lease_expires_at LIMIT 1 FOR UPDATE SKIP LOCKED)")
        .bind(&scopes).execute(pool).await.map_err(db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::next_retry_partition;

    #[test]
    fn retries_rotate_instead_of_repeatedly_selecting_the_first_failure() {
        let pending = (1 << 0) | (1 << 15);
        assert_eq!(next_retry_partition(pending, 0), Some(15));
        assert_eq!(next_retry_partition(pending, 15), Some(0));
        assert_eq!(next_retry_partition(0, 4), None);
    }
}
