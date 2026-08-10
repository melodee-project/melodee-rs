//! PostgreSQL proofs for immutable publication intent and convergent claims.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{T0, TestDb, seed_ready_item, test_config};
use melodee_domain::{
    CANONICAL_LAYOUT_VERSION, CURRENT_MANIFEST_VERSION, CanonicalAsset, CanonicalAssetKind,
    Finding, FindingSeverity, IngestionState, ManifestPublication, ManifestReleaseObservation,
    NormalizationObservations, PublicationAuthorization, PublicationJournalStep, PublicationPlan,
    ReleaseId, ReleaseManifest, SourceDisposition, StructuralValidation, TransitionKind,
    ValidationOutcome, WorkErrorCode,
};
use melodee_persistence::{WorkItemStore, WorkerId};
use time::Duration;

async fn seed_validating(
    store: &WorkItemStore,
    db: &TestDb,
    path: &str,
) -> (uuid::Uuid, ReleaseManifest) {
    let id = seed_ready_item(store, &db.pool, path).await;
    let config = test_config();
    let inspector = WorkerId::new();
    store
        .claim(&db.pool, &inspector, &config, T0)
        .await
        .expect("claim inspection");
    store
        .transition(
            &db.pool,
            id,
            Some(&inspector),
            TransitionKind::Worker,
            IngestionState::Inspecting,
            None,
            T0,
        )
        .await
        .expect("start inspection");
    let mut manifest = ReleaseManifest::new(
        id,
        None,
        path.to_owned(),
        None,
        Some("source-signature".to_owned()),
        None,
        None,
        Some(0),
        None,
        true,
        ManifestReleaseObservation {
            title: Some("Release".to_owned()),
            artists: vec!["Observed Artist".to_owned()],
            date: None,
            source_labels: serde_json::Value::Null,
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    store
        .publish_manifest(
            &db.pool,
            id,
            &inspector,
            &manifest,
            &"1".repeat(64),
            None,
            Some("source-signature"),
            1,
            T0,
        )
        .await
        .expect("publish inspection");

    let normalizer = WorkerId::new();
    store
        .claim_normalizing(&db.pool, &normalizer, &config, T0)
        .await
        .expect("claim normalization");
    manifest.schema_version = CURRENT_MANIFEST_VERSION;
    manifest.normalization = Some(NormalizationObservations {
        profile_version: "test-profile".to_owned(),
        ..NormalizationObservations::default()
    });
    store
        .publish_normalization(
            &db.pool,
            id,
            &normalizer,
            &manifest,
            &"b".repeat(64),
            "track.flac",
            1,
            T0,
        )
        .await
        .expect("publish normalization");
    (id, manifest)
}

fn publication_contract(manifest_hash: &str) -> (PublicationPlan, ManifestPublication) {
    let release_id = ReleaseId::new();
    let authorization = PublicationAuthorization::phase3_structural();
    let byte_hash = "a".repeat(64);
    let assets = vec![CanonicalAsset {
        kind: CanonicalAssetKind::Media,
        source_path: "track.flac".to_owned(),
        normalized_path: "audio/track.flac.mp3".to_owned(),
        canonical_path: format!("media/{byte_hash}.mp3"),
        size_bytes: 42,
        byte_hash,
    }];
    let target_relative_path = PublicationPlan::canonical_target_path(release_id);
    let plan = PublicationPlan {
        layout_version: CANONICAL_LAYOUT_VERSION,
        release_id,
        input_manifest_hash: manifest_hash.to_owned(),
        authorization: authorization.clone(),
        manifest_relative_path: format!("{target_relative_path}/manifest.json"),
        target_relative_path,
        assets: assets.clone(),
    };
    let plan_hash = plan.plan_hash().expect("hash plan");
    let publication = ManifestPublication {
        release_id,
        validation: StructuralValidation {
            profile_name: "phase3.structural".to_owned(),
            profile_version: 1,
            outcome: ValidationOutcome::Passed,
            findings: Vec::new(),
        },
        authorization,
        source_disposition: SourceDisposition::Retained,
        layout_version: CANONICAL_LAYOUT_VERSION,
        plan_hash,
        canonical_assets: assets,
    };
    (plan, publication)
}

#[tokio::test]
async fn authorization_stage_publish_is_fenced_and_durable() {
    let db = TestDb::create("canonical_publication").await;
    let store = WorkItemStore;
    let config = test_config();
    let (id, mut manifest) = seed_validating(&store, &db, "release/publication").await;

    let validator = WorkerId::new();
    let claimed = store
        .claim_validating(&db.pool, &validator, &config, T0)
        .await
        .expect("claim validation");
    assert_eq!(claimed.len(), 1);
    let (plan, publication) = publication_contract(&"b".repeat(64));
    manifest
        .upgrade_for_publication(publication.clone(), &plan)
        .expect("upgrade manifest");
    let stored = store
        .stage_authorized_publication(&db.pool, id, &validator, &manifest, &plan, T0)
        .await
        .expect("stage publication");
    assert_eq!(stored.release_id, publication.release_id);
    let staged = store.get(&db.pool, id).await.expect("staged work item");
    assert_eq!(staged.state, IngestionState::Staged);
    assert!(
        staged.lease_owner.is_none(),
        "staging clears validation lease"
    );

    let publisher = WorkerId::new();
    let claimed = store
        .claim_publication(&db.pool, &publisher, &config, T0)
        .await
        .expect("claim publication");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].state, IngestionState::Publishing);
    let sealed_manifest_hash = "d".repeat(64);

    assert!(
        store
            .complete_publication(
                &db.pool,
                id,
                &publisher,
                &publication.plan_hash,
                &sealed_manifest_hash,
                T0,
            )
            .await
            .is_err(),
        "caller assertion without durable evidence is insufficient"
    );
    store
        .record_publication_progress(
            &db.pool,
            id,
            &publisher,
            &format!("bundle_sealed:{}", publication.plan_hash),
            PublicationJournalStep::BundleSealed,
            None,
            None,
            None,
            None,
            T0,
        )
        .await
        .expect("record sealed bundle");
    assert!(
        store
            .complete_publication(
                &db.pool,
                id,
                &publisher,
                &publication.plan_hash,
                &sealed_manifest_hash,
                T0,
            )
            .await
            .is_err(),
        "sealed bundle without every planned asset is insufficient"
    );
    let asset = &plan.assets[0];
    store
        .record_publication_progress(
            &db.pool,
            id,
            &publisher,
            &format!("asset:{}", asset.canonical_path),
            PublicationJournalStep::AssetCopied,
            Some(&asset.canonical_path),
            Some(asset.size_bytes),
            Some(&asset.byte_hash),
            None,
            T0,
        )
        .await
        .expect("record asset");
    assert!(
        store
            .complete_publication(
                &db.pool,
                id,
                &publisher,
                &publication.plan_hash,
                &sealed_manifest_hash,
                T0,
            )
            .await
            .is_err(),
        "copied assets without exact final evidence are insufficient"
    );
    store
        .record_publication_progress(
            &db.pool,
            id,
            &publisher,
            "finalized:wrong-plan",
            PublicationJournalStep::Finalized,
            Some(&plan.manifest_relative_path),
            Some(128),
            Some(&sealed_manifest_hash),
            None,
            T0,
        )
        .await
        .expect("record wrong-key finalization");
    assert!(
        store
            .complete_publication(
                &db.pool,
                id,
                &publisher,
                &publication.plan_hash,
                &sealed_manifest_hash,
                T0,
            )
            .await
            .is_err(),
        "a final marker with the wrong idempotency key is insufficient"
    );
    store
        .record_publication_progress(
            &db.pool,
            id,
            &publisher,
            &format!("finalized:{}", publication.plan_hash),
            PublicationJournalStep::Finalized,
            Some(&plan.manifest_relative_path),
            Some(128),
            Some(&sealed_manifest_hash),
            None,
            T0,
        )
        .await
        .expect("record exact finalization");
    let published = store
        .complete_publication(
            &db.pool,
            id,
            &publisher,
            &publication.plan_hash,
            &sealed_manifest_hash,
            T0,
        )
        .await
        .expect("complete publication");
    assert_eq!(published.state, IngestionState::Published);
    assert!(published.lease_owner.is_none());

    let durable = store
        .get_publication(&db.pool, id)
        .await
        .expect("read publication")
        .expect("publication exists");
    assert_eq!(durable.release_id, publication.release_id);
    assert_eq!(durable.published_at, Some(T0));
    assert_eq!(
        durable.sealed_manifest_hash.as_deref(),
        Some(sealed_manifest_hash.as_str())
    );
    assert_eq!(
        store
            .list_publication_journal(&db.pool, id)
            .await
            .expect("journal")
            .len(),
        4
    );
    db.drop().await;
}

#[tokio::test]
async fn expired_publication_lease_remains_convergent_and_directly_reclaimable() {
    let db = TestDb::create("publication_reclaim").await;
    let store = WorkItemStore;
    let config = test_config();
    let (id, mut manifest) = seed_validating(&store, &db, "release/reclaim").await;
    let validator = WorkerId::new();
    store
        .claim_validating(&db.pool, &validator, &config, T0)
        .await
        .expect("claim validation");
    let (plan, publication) = publication_contract(&"b".repeat(64));
    manifest
        .upgrade_for_publication(publication, &plan)
        .expect("upgrade");
    store
        .stage_authorized_publication(&db.pool, id, &validator, &manifest, &plan, T0)
        .await
        .expect("stage");
    let first_publisher = WorkerId::new();
    store
        .claim_publication(&db.pool, &first_publisher, &config, T0)
        .await
        .expect("first claim");

    let after_expiry = T0 + config.lease_duration + Duration::seconds(1);
    store
        .recover_expired_leases(&db.pool, &config.retry_policy, after_expiry)
        .await
        .expect("recover expired lease");
    let recoverable = store.get(&db.pool, id).await.expect("recoverable item");
    assert_eq!(recoverable.state, IngestionState::Publishing);
    assert_eq!(recoverable.attempt_count, 0);
    assert!(recoverable.lease_owner.is_none());

    let second_publisher = WorkerId::new();
    let reclaimed = store
        .claim_publication(&db.pool, &second_publisher, &config, after_expiry)
        .await
        .expect("reclaim publication");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].state, IngestionState::Publishing);
    db.drop().await;
}

#[tokio::test]
async fn validation_review_preserves_findings_without_consuming_an_attempt() {
    let db = TestDb::create("validation_review_evidence").await;
    let store = WorkItemStore;
    let config = test_config();
    let (id, _) = seed_validating(&store, &db, "release/review").await;
    let validator = WorkerId::new();
    store
        .claim_validating(&db.pool, &validator, &config, T0)
        .await
        .expect("claim validation");
    let validation = StructuralValidation {
        profile_name: "phase3.structural".to_owned(),
        profile_version: 1,
        outcome: ValidationOutcome::NeedsReview,
        findings: vec![Finding {
            code: "validation.track_order_ambiguous".to_owned(),
            message: "two Tracks have the same local ordering".to_owned(),
            severity: FindingSeverity::Error,
            file: Some("disc/track.mp3".to_owned()),
            evidence: serde_json::json!({ "track_number": 1 }),
        }],
    };

    let parked = store
        .record_validation_review(&db.pool, id, &validator, &validation, T0)
        .await
        .expect("park review");
    assert_eq!(parked.state, IngestionState::NeedsReview);
    assert_eq!(parked.attempt_count, 0);
    assert!(parked.lease_owner.is_none());
    let events = store.list_events(&db.pool, id).await.expect("events");
    let evidence = events
        .iter()
        .find(|event| event.reason_code.as_deref() == Some("validation.needs_review"))
        .and_then(|event| event.metadata.clone())
        .expect("full validation event evidence");
    let stored_validation: StructuralValidation =
        serde_json::from_value(evidence).expect("decode validation evidence");
    assert_eq!(stored_validation, validation);
    db.drop().await;
}

#[tokio::test]
async fn publication_problem_keeps_intent_and_is_directly_reclaimable() {
    let db = TestDb::create("publication_problem").await;
    let store = WorkItemStore;
    let config = test_config();
    let (id, mut manifest) = seed_validating(&store, &db, "release/problem").await;
    let validator = WorkerId::new();
    store
        .claim_validating(&db.pool, &validator, &config, T0)
        .await
        .expect("claim validation");
    let (plan, publication) = publication_contract(&"b".repeat(64));
    manifest
        .upgrade_for_publication(publication.clone(), &plan)
        .expect("upgrade");
    store
        .stage_authorized_publication(&db.pool, id, &validator, &manifest, &plan, T0)
        .await
        .expect("stage");
    let publisher = WorkerId::new();
    store
        .claim_publication(&db.pool, &publisher, &config, T0)
        .await
        .expect("claim publication");

    let evidence = serde_json::json!({ "target": plan.target_relative_path });
    let problem = store
        .record_publication_problem(
            &db.pool,
            id,
            &publisher,
            PublicationJournalStep::TargetConflict,
            WorkErrorCode::new("publication.target_conflict"),
            "existing final differs from the immutable plan",
            Some(evidence.clone()),
            T0,
        )
        .await
        .expect("record convergent problem");
    assert_eq!(problem.state, IngestionState::Publishing);
    assert_eq!(problem.attempt_count, 0);
    assert!(problem.lease_owner.is_none());
    assert_eq!(
        problem.last_error_code.as_ref().map(WorkErrorCode::as_str),
        Some("publication.target_conflict")
    );
    assert!(
        store
            .get_publication(&db.pool, id)
            .await
            .expect("publication")
            .is_some(),
        "immutable plan remains durable"
    );
    let journal = store
        .list_publication_journal(&db.pool, id)
        .await
        .expect("journal");
    assert!(journal.iter().any(|entry| {
        entry.step == PublicationJournalStep::TargetConflict
            && entry.journal_key == format!("target_conflict:{}", publication.plan_hash)
    }));

    let second_publisher = WorkerId::new();
    let reclaimed = store
        .claim_publication(&db.pool, &second_publisher, &config, T0)
        .await
        .expect("reclaim problem");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].state, IngestionState::Publishing);
    store
        .record_publication_problem(
            &db.pool,
            id,
            &second_publisher,
            PublicationJournalStep::TargetConflict,
            WorkErrorCode::new("publication.target_conflict"),
            "existing final differs from the immutable plan",
            Some(evidence),
            T0,
        )
        .await
        .expect("idempotently retain existing problem evidence");
    assert_eq!(
        store
            .list_publication_journal(&db.pool, id)
            .await
            .expect("journal after repeat")
            .iter()
            .filter(|entry| entry.step == PublicationJournalStep::TargetConflict)
            .count(),
        1
    );
    db.drop().await;
}
