//! Reviewed metadata and explicit catalog identifications. Every change to
//! the publication input commits together with the worker hand-off and audit.
use melodee_domain::{
    IngestionState, ReleaseManifest, ReleaseWorkItem, TransitionKind, WorkEventType,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    CatalogStoreError, ReleaseMerge, WorkStoreError,
    work_items::{insert_event, lock_for_update},
};

/// Selection resolved against a server-side provider response or catalog row.
pub struct ReviewIdentification {
    pub provider: String,
    pub external_id: String,
    pub target_release_id: Option<Uuid>,
}

/// The editor can only change an unsealed, normalized manifest. Parallel
/// editors must reload if a worker or another editor changed its revision.
pub struct ReviewedManifest<'a> {
    pub expected_hash: &'a str,
    pub new_hash: &'a str,
    pub manifest: &'a ReleaseManifest,
    pub changes: Value,
}

pub async fn save_review_correction(
    pool: &PgPool,
    id: Uuid,
    actor: Uuid,
    correction: ReviewedManifest<'_>,
    selection: Option<&ReviewIdentification>,
    now: OffsetDateTime,
) -> Result<(), WorkStoreError> {
    let mut tx = pool.begin().await?;
    let item = lock_for_update(&mut tx, id).await?;
    if item.state != IngestionState::NeedsReview || item.has_active_lease(now) {
        return Err(WorkStoreError::InvalidState(id, item.state));
    }
    if item.manifest_hash.as_deref() != Some(correction.expected_hash) {
        return Err(WorkStoreError::ManifestConflict(id));
    }
    let retarget = item.state_reason.as_deref() == Some("review.merge_target_unavailable");
    let next = if retarget {
        IngestionState::Indexing
    } else {
        IngestionState::Validating
    };
    item.state
        .ensure_transition(next, TransitionKind::Operator)?;
    if retarget {
        if selection.is_none_or(|s| s.target_release_id.is_none()) {
            return Err(WorkStoreError::InvalidRequest(
                "Select an active Melodee Release",
            ));
        }
    } else {
        let changed = sqlx::query(
            "UPDATE release_work_item_manifests m SET manifest_hash=$3, manifest_payload=$4
             WHERE work_item_id=$1 AND manifest_hash=$2 AND schema_version=2
             AND NOT EXISTS (SELECT 1 FROM review_deletion_authorizations WHERE work_item_id=$1)
             AND NOT EXISTS (SELECT 1 FROM release_publication_authorizations WHERE work_item_id=$1)
             AND NOT EXISTS (SELECT 1 FROM release_publication_plans WHERE work_item_id=$1)
             AND NOT EXISTS (SELECT 1 FROM catalog_imports WHERE work_item_id=$1)",
        )
        .bind(id)
        .bind(correction.expected_hash)
        .bind(correction.new_hash)
        .bind(sqlx::types::Json(correction.manifest))
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(WorkStoreError::ManifestConflict(id));
        }
    }
    if let Some(s) = selection {
        if let Some(target) = s.target_release_id {
            let active: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM releases WHERE id=$1 AND status='active' FOR SHARE",
            )
            .bind(target)
            .fetch_optional(&mut *tx)
            .await?;
            if active.is_none() {
                return Err(WorkStoreError::ManifestConflict(id));
            }
        }
        let stored = sqlx::query("INSERT INTO release_review_identifications(work_item_id,provider,external_id,target_release_id,actor)
            VALUES($1,$2,$3,$4,$5) ON CONFLICT(work_item_id) DO UPDATE SET
            provider=EXCLUDED.provider,external_id=EXCLUDED.external_id,target_release_id=EXCLUDED.target_release_id,actor=EXCLUDED.actor
            WHERE release_review_identifications.applied_at IS NULL")
            .bind(id).bind(&s.provider).bind(&s.external_id).bind(s.target_release_id).bind(actor).execute(&mut *tx).await?;
        if stored.rows_affected() != 1 {
            return Err(WorkStoreError::ManifestConflict(id));
        }
    }
    let reason = if selection.is_some() {
        "review.identified"
    } else {
        "review.metadata_edited"
    };
    let hash = if retarget {
        correction.expected_hash
    } else {
        correction.new_hash
    };
    sqlx::query(
        "UPDATE release_work_items SET state=$2,state_reason=$3,manifest_hash=$4,updated_at=$5,
        lease_owner=NULL,lease_expires_at=NULL,lease_acquired_at=NULL,next_attempt_at=NULL,
        last_error_code=NULL,last_error_message=NULL WHERE id=$1",
    )
    .bind(id)
    .bind(next.as_str())
    .bind(reason)
    .bind(hash)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let metadata = json!({
        "actor": actor,
        "previousHash": correction.expected_hash,
        "newHash": hash,
        "changes": correction.changes,
        "provider": selection.map(|s| &s.provider),
        "externalId": selection.map(|s| &s.external_id),
    });
    insert_event(
        &mut tx,
        id,
        WorkEventType::Transitioned,
        Some(item.state),
        Some(next),
        Some(reason),
        None,
        item.correlation_id,
        now,
        Some(metadata),
    )
    .await?;
    if retarget {
        let item = lock_for_update(&mut tx, id).await?;
        if apply_accepted_identification(&mut tx, item, now)
            .await?
            .state
            != IngestionState::Indexing
        {
            return Err(WorkStoreError::ManifestConflict(id));
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Apply an editor's selection in the same transaction that accepts this
/// Release. No catalog mutation depends on the search projection being online.
pub(crate) async fn apply_accepted_identification(
    tx: &mut Transaction<'_, Postgres>,
    item: ReleaseWorkItem,
    now: OffsetDateTime,
) -> Result<ReleaseWorkItem, WorkStoreError> {
    if item.state != IngestionState::Indexing {
        return Ok(item);
    }
    let id = item.id;
    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>, String, String, Uuid)>(
        "SELECT ci.release_id,s.target_release_id,s.provider,s.external_id,s.actor
         FROM release_review_identifications s JOIN catalog_imports ci ON ci.work_item_id=s.work_item_id
         WHERE s.work_item_id=$1 AND s.applied_at IS NULL FOR UPDATE OF s")
        .bind(id).fetch_optional(&mut **tx).await?;
    let Some((source, target, provider, external, actor)) = row else {
        return Ok(item);
    };
    if item.state_reason.as_deref() == Some("review.identified") {
        sqlx::query("UPDATE releases SET status='active',revision=revision+1,updated_at=$2 WHERE id=$1 AND status='deprecated'")
            .bind(source).bind(now).execute(&mut **tx).await?;
    }
    let mut event = None;
    if let Some(target) = target {
        if source != target {
            let merge = ReleaseMerge {
                source_release_id: source,
                target_release_id: target,
                actor: actor.to_string(),
                reason: "Release Review identification".to_owned(),
                occurred_at: now,
            };
            // A stale target or an oversized merge parks only this item.
            // A savepoint also rolls back any partial merge writes.
            use sqlx::Acquire;
            let mut save = tx.begin().await?;
            match crate::catalog::merge_review_selection(&mut save, &merge).await {
                Ok(merged) => {
                    event = Some(merged);
                    save.commit().await?;
                }
                Err(
                    CatalogStoreError::MutationConflict
                    | CatalogStoreError::CorrectionScopeExceeded { .. },
                ) => {
                    save.rollback().await?;
                    let reason = "review.merge_target_unavailable";
                    sqlx::query("UPDATE release_work_items SET state='needs_review',state_reason=$2,updated_at=$3 WHERE id=$1")
                            .bind(id).bind(reason).bind(now).execute(&mut **tx).await?;
                    insert_event(
                        tx,
                        id,
                        WorkEventType::Transitioned,
                        Some(IngestionState::Indexing),
                        Some(IngestionState::NeedsReview),
                        Some(reason),
                        None,
                        item.correlation_id,
                        now,
                        Some(json!({"target":target})),
                    )
                    .await?;
                    sqlx::query("UPDATE releases SET status='deprecated',revision=revision+1,updated_at=$2 WHERE id=$1 AND status='active'")
                            .bind(source).bind(now).execute(&mut **tx).await?;
                    return lock_for_update(tx, id).await;
                }
                Err(error) => return Err(WorkStoreError::ReviewMerge(Box::new(error))),
            }
        }
    } else {
        sqlx::query("INSERT INTO release_external_identities(id,release_id,provider,external_id,trusted,confidence,evidence_source,is_locked,observed_at)
               VALUES($1,$2,$3,$4,true,10000,'operator.review',true,$5)
               ON CONFLICT(release_id,provider,external_id) DO UPDATE SET trusted=true,confidence=10000,
               evidence_source='operator.review',is_locked=true,observed_at=EXCLUDED.observed_at")
               .bind(Uuid::now_v7()).bind(source).bind(provider).bind(external).bind(now).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE release_review_identifications SET applied_at=$2,merge_event_id=$3 WHERE work_item_id=$1")
            .bind(id).bind(now).bind(event).execute(&mut **tx).await?;
    Ok(item)
}
