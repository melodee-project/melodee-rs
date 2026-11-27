//! Database fencing for disconnected protected stage APIs; these tests do not
//! claim protected filesystem or lifecycle-lock integration is complete.
#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use melodee_domain::{
    IngestionState, ManifestReleaseObservation, NormalizationObservations, ReleaseManifest,
    ReleaseWorkItem, TransitionKind,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn protected_stage_writes_require_original_epoch_through_normalization() {
    let db = common::TestDb::create("protected_stage_write").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let config = WorkEngineConfig::default();
    let worker = WorkerId::new();
    let mut item = ReleaseWorkItem::discovered("source".to_owned(), None, Uuid::now_v7(), now);
    item.state = IngestionState::Ready;
    store.create(&db.pool, &item).await.unwrap();
    let (_, old) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    store
        .begin_protected_staging_handoff(
            &db.pool,
            &old,
            &worker,
            "releases/item/source",
            "src-gen",
            now,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE release_work_items SET lease_expires_at=$2 WHERE id=$1")
        .bind(item.id)
        .bind(now - time::Duration::seconds(1))
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .claim_ordinary_with_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    let (_, current) = store
        .claim_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    assert!(
        store
            .transition_protected(
                &db.pool,
                &old,
                &worker,
                IngestionState::Inspecting,
                None,
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .transition(
                &db.pool,
                item.id,
                Some(&worker),
                TransitionKind::Worker,
                IngestionState::Inspecting,
                None,
                now
            )
            .await
            .is_err()
    );
    store
        .transition_protected(
            &db.pool,
            &current,
            &worker,
            IngestionState::Inspecting,
            None,
            now,
        )
        .await
        .unwrap();
    let mut manifest = ReleaseManifest::new(
        item.id,
        None,
        "source".to_owned(),
        None,
        Some("signature".to_owned()),
        None,
        None,
        Some(0),
        None,
        true,
        ManifestReleaseObservation {
            title: Some("Observed".to_owned()),
            artists: vec![],
            date: None,
            source_labels: serde_json::Value::Null,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let hash = "1".repeat(64);
    assert!(
        store
            .publish_manifest(
                &db.pool,
                item.id,
                &worker,
                &manifest,
                &hash,
                None,
                Some("signature"),
                1,
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .publish_manifest_protected(
                &db.pool,
                &old,
                &worker,
                &manifest,
                &hash,
                None,
                Some("signature"),
                1,
                now
            )
            .await
            .is_err()
    );
    store
        .publish_manifest_protected(
            &db.pool,
            &current,
            &worker,
            &manifest,
            &hash,
            None,
            Some("signature"),
            1,
            now,
        )
        .await
        .unwrap();
    assert!(
        store
            .claim_ordinary_normalizing_with_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .is_empty()
    );
    let (_, normalizing) = store
        .claim_normalizing_with_protected_epoch(&db.pool, &worker, &config, now)
        .await
        .unwrap()
        .remove(0)
        .into_parts();
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "test".to_owned(),
        ..NormalizationObservations::default()
    });
    let normalized_hash = "2".repeat(64);
    assert!(
        store
            .publish_normalization(
                &db.pool,
                item.id,
                &worker,
                &manifest,
                &normalized_hash,
                "file",
                1,
                now
            )
            .await
            .is_err()
    );
    assert!(
        store
            .publish_normalization_protected(
                &db.pool,
                &current,
                &worker,
                &manifest,
                &normalized_hash,
                "file",
                1,
                now
            )
            .await
            .is_err()
    );
    store
        .publish_normalization_protected(
            &db.pool,
            &normalizing,
            &worker,
            &manifest,
            &normalized_hash,
            "file",
            1,
            now,
        )
        .await
        .unwrap();
    let result = store.get(&db.pool, item.id).await.unwrap();
    assert_eq!(result.state, IngestionState::Validating);
    db.drop().await;
}

#[tokio::test]
async fn interrupted_protected_inspections_reclaim_fairly_with_new_epochs() {
    let db = common::TestDb::create("protected_recovery_fair").await;
    let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let store = WorkItemStore;
    let config = WorkEngineConfig {
        claim_batch_size: 1,
        ..WorkEngineConfig::default()
    };
    let worker = WorkerId::new();
    let oversized = WorkEngineConfig {
        claim_batch_size: 129,
        ..config
    };
    assert!(
        store
            .claim_interrupted_protected_inspections(&db.pool, &worker, &oversized)
            .await
            .is_err()
    );
    let cursor: (Option<OffsetDateTime>, Option<Uuid>) = sqlx::query_as(
        "SELECT last_expiry,last_id FROM protected_inspection_recovery_cursor WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(cursor, (None, None));
    let mut old = Vec::new();
    for source in ["source-a", "source-b"] {
        let mut item = ReleaseWorkItem::discovered(source.to_owned(), None, Uuid::now_v7(), now);
        item.state = IngestionState::Ready;
        store.create(&db.pool, &item).await.unwrap();
        let (_, claim) = store
            .claim_with_protected_epoch(&db.pool, &worker, &config, now)
            .await
            .unwrap()
            .remove(0)
            .into_parts();
        store
            .begin_protected_staging_handoff(
                &db.pool,
                &claim,
                &worker,
                &format!("releases/{}/source", item.id),
                "generation",
                now,
            )
            .await
            .unwrap();
        store
            .transition_protected(
                &db.pool,
                &claim,
                &worker,
                IngestionState::Inspecting,
                None,
                now,
            )
            .await
            .unwrap();
        old.push((item.id, claim));
    }
    sqlx::query(
        "UPDATE release_work_items SET lease_expires_at=$1 WHERE protected_custody_required",
    )
    .bind(now - time::Duration::seconds(1))
    .execute(&db.pool)
    .await
    .unwrap();
    let first:(Uuid,Option<Uuid>,String)=sqlx::query_as("SELECT id,source_library_id,source_path FROM release_work_items WHERE protected_custody_required ORDER BY lease_expires_at,id LIMIT 1").fetch_one(&db.pool).await.unwrap();
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(1869834357,hashtext(COALESCE($1::uuid::text,'__default__') || '|' || $2::text))").bind(first.1).bind(&first.2).execute(&mut *blocker).await.unwrap();
    assert!(
        store
            .claim_interrupted_protected_inspections(&db.pool, &worker, &config)
            .await
            .unwrap()
            .is_empty()
    );
    let claims = store
        .claim_interrupted_protected_inspections(&db.pool, &worker, &config)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    let (second_item, second_claim) = claims.into_iter().next().unwrap().into_parts();
    assert_ne!(second_item.id, first.0);
    let stale = old.iter().find(|(id, _)| *id == second_item.id).unwrap();
    assert!(
        store
            .renew_protected_claim_epoch(&db.pool, &stale.1, time::Duration::minutes(1))
            .await
            .is_err()
    );
    assert!(
        store
            .release_protected_lease(&db.pool, &stale.1, &worker, now)
            .await
            .is_err()
    );
    assert!(
        store
            .renew_protected_claim_epoch(&db.pool, &second_claim, time::Duration::minutes(1))
            .await
            .is_ok()
    );
    sqlx::query("SELECT pg_advisory_unlock(1869834357,hashtext(COALESCE($1::uuid::text,'__default__') || '|' || $2::text))").bind(first.1).bind(&first.2).execute(&mut *blocker).await.unwrap();
    assert!(
        store
            .claim_interrupted_protected_inspections(&db.pool, &worker, &config)
            .await
            .unwrap()
            .is_empty()
    );
    let wrapped = store
        .claim_interrupted_protected_inspections(&db.pool, &worker, &config)
        .await
        .unwrap();
    assert_eq!(wrapped.len(), 1);
    assert_eq!(wrapped[0].item().id, first.0);
    drop(blocker);
    db.drop().await;
}
