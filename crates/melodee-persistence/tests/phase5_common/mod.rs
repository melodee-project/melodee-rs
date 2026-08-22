//! Shared cross-slice PostgreSQL fixtures.
//!
//! The fixture deliberately reaches `Cataloged` through the public ingestion-to-catalog
//! handoff and leaves every media path nonexistent. Tests can therefore
//! prove policy reads PostgreSQL facts instead of reopening inbound or canonical
//! media.

#![allow(dead_code, clippy::expect_used)]

use crate::catalog_common::{
    CatalogFixturePresentation, PublishedCatalogFixture, seed_published_release,
    seed_published_release_with_normalized_hash, seed_published_release_with_read_error,
};
use crate::common::{TestDb, test_config};
use std::collections::BTreeSet;

use melodee_domain::{
    ClassificationEvidence, ClassificationEvidenceKind, Confidence, DecisionCompletionState,
    Disposition, EvidenceSource, IngestionState, NoveltyEvaluation, NoveltyEvidenceKind,
    PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleSetId, PolicyRuleType, ReleaseClassification,
    ReleaseClassificationId, ReleaseId, ReleasePolicyDecision, ReleasePolicyDecisionId,
    ReleaseTrait, TrackNovelty, TrackNoveltyCategory, TrackNoveltyId,
};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, PolicyCatalogContext, PolicyEvaluationCommit,
    WorkItemStore, WorkerId,
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

/// One complete catalog handoff ready for policy evaluation.
#[derive(Debug, Clone)]
pub struct CatalogedPolicyFixture {
    pub published: PublishedCatalogFixture,
    pub cataloged_at: OffsetDateTime,
}

impl CatalogedPolicyFixture {
    pub fn work_item_id(&self) -> Uuid {
        self.published.work_item_id
    }

    pub fn release_id(&self) -> ReleaseId {
        self.published.release_id
    }

    pub fn assert_media_paths_absent(&self) {
        assert!(
            !std::path::Path::new(&self.published.source_path).exists(),
            "fixture source path must not exist"
        );
        assert!(
            !std::path::Path::new(&self.published.canonical_asset_path).exists(),
            "fixture canonical asset path must not exist"
        );
    }
}

/// Publishes and catalogs one single-Track Release through public persistence
/// APIs. The current time is used for catalog leases because PostgreSQL fences
/// them against `clock_timestamp()`.
pub async fn seed_cataloged_release(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
) -> CatalogedPolicyFixture {
    let published = seed_published_release(db, ordinal, presentation).await;
    catalog_published_release(db, published).await
}

/// Variant with an explicit observed genre tag for adapter-surface tests.
pub async fn seed_cataloged_release_with_genre(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    genre: &str,
) -> CatalogedPolicyFixture {
    let published = crate::catalog_common::seed_published_release_with_normalized_hash_and_genre(
        db,
        ordinal,
        presentation,
        crate::catalog_common::deterministic_hash('a', ordinal),
        Some(genre),
    )
    .await;
    catalog_published_release(db, published).await
}

/// Builds a complete second v3/catalog handoff whose playable bytes match an
/// existing immutable MediaAsset hash.
pub async fn seed_cataloged_release_with_normalized_hash(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
    normalized_byte_hash: String,
) -> CatalogedPolicyFixture {
    let published = seed_published_release_with_normalized_hash(
        db,
        ordinal,
        presentation,
        normalized_byte_hash,
    )
    .await;
    catalog_published_release(db, published).await
}

/// Catalogs a complete v3 whose immutable inspection observations record a
/// media read error; policy must quarantine it without reopening the path.
pub async fn seed_cataloged_release_with_read_error(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
) -> CatalogedPolicyFixture {
    let published = seed_published_release_with_read_error(db, ordinal, presentation).await;
    catalog_published_release(db, published).await
}

async fn catalog_published_release(
    db: &TestDb,
    published: PublishedCatalogFixture,
) -> CatalogedPolicyFixture {
    let cataloged_at = OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    let claimed = CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), cataloged_at)
        .await
        .expect("claim published Phase 5 fixture for catalog import");
    assert_eq!(claimed.len(), 1, "fixture import claim remains isolated");
    assert_eq!(claimed[0].id, published.work_item_id);

    let result = CatalogStore
        .import_manifest_v3(
            &db.pool,
            published.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase5.fixture-resolver.v1", "phase5-acceptance"),
            cataloged_at,
        )
        .await
        .expect("catalog Phase 5 fixture from immutable v3");
    assert!(result.inserted);
    assert_eq!(result.release_id, published.release_id);

    let item = WorkItemStore
        .get(&db.pool, published.work_item_id)
        .await
        .expect("read cataloged Phase 5 fixture");
    assert_eq!(item.state, IngestionState::Cataloged);
    assert!(item.lease_owner.is_none());

    let fixture = CatalogedPolicyFixture {
        published,
        cataloged_at,
    };
    fixture.assert_media_paths_absent();
    fixture
}

/// Acceptance rule set with explicit operator, redundant-collection, and
/// unknown-classification outcomes.
pub fn policy_rule_set(id: PolicyRuleSetId, now: OffsetDateTime) -> PolicyRuleSet {
    PolicyRuleSet::new(PolicyRuleSet {
        id,
        name: format!("phase5-acceptance-{}", id.into_uuid()),
        version: 1,
        rules: vec![
            PolicyRule {
                id: "operator-override".to_string(),
                rule_type: PolicyRuleType::AdministratorOverride,
                predicate_json: json!({}),
                disposition: Disposition::Review,
                confidence_threshold: None,
            },
            PolicyRule {
                id: "corrupt-media".to_string(),
                rule_type: PolicyRuleType::CorruptMedia,
                predicate_json: json!({}),
                disposition: Disposition::Quarantine,
                confidence_threshold: None,
            },
            PolicyRule {
                id: "redundant-greatest-hits".to_string(),
                rule_type: PolicyRuleType::GreatestHitsRedundancy,
                predicate_json: json!({ "required_traits": ["greatest_hits"] }),
                disposition: Disposition::Ignore,
                confidence_threshold: None,
            },
            PolicyRule {
                id: "unknown".to_string(),
                rule_type: PolicyRuleType::UnknownClassification,
                predicate_json: json!({}),
                disposition: Disposition::Review,
                confidence_threshold: None,
            },
        ],
        created_at: now,
        created_by: PolicyActor::System {
            policy_rule_set_version: 1,
        },
    })
    .expect("valid acceptance rule set")
}

/// Builds the valid single-Track Unknown/Uncertain/Review result expected from
/// the pure policy engine for the default catalog fixture. A fresh catalog
/// import has a local AudioIdentity but no durable manual/provider confirmation.
pub fn unknown_review_commit(
    context: &PolicyCatalogContext,
    rule_set_id: PolicyRuleSetId,
    now: OffsetDateTime,
) -> PolicyEvaluationCommit {
    assert_eq!(context.tracks.len(), 1);
    let classification_id = ReleaseClassificationId::new();
    let release_id = context.claim.release_id;
    let classification = ReleaseClassification::new(ReleaseClassification {
        id: classification_id,
        release_id,
        traits: BTreeSet::from([ReleaseTrait::Unknown]),
        evidence: vec![ClassificationEvidence {
            id: classification_id,
            release_id,
            trait_: ReleaseTrait::Unknown,
            source: EvidenceSource::ReleaseTitle,
            kind: ClassificationEvidenceKind::TitlePattern,
            confidence: Confidence::new(100).expect("valid confidence"),
            detail: "No classification evidence found; defaulting to Unknown".to_string(),
        }],
        confidence: Confidence::new(100).expect("valid confidence"),
        classified_at: now,
        classifier_version: "melodee.policy.classifier-v1".to_string(),
    })
    .expect("valid unknown classification");
    let track = &context.tracks[0];
    let track_novelty = vec![
        TrackNovelty::new(TrackNovelty {
            id: TrackNoveltyId::new(),
            release_id,
            track_id: melodee_domain::TrackId::from_uuid(track.track_id),
            audio_identity_id: melodee_domain::AudioIdentityId::from_uuid(track.audio_identity_id),
            media_asset_id: track
                .media_asset_id
                .map(melodee_domain::MediaAssetId::from_uuid),
            category: TrackNoveltyCategory::Uncertain,
            confidence: Confidence::zero(),
            evidence_kind: NoveltyEvidenceKind::InsufficientEvidence,
            detail: "AudioIdentity lacks durable fingerprint or match confirmation".to_string(),
            evaluated_at: now,
        })
        .expect("valid new Track novelty"),
    ];
    let novelty_evaluation = NoveltyEvaluation::new(NoveltyEvaluation {
        release_id,
        policy_rule_set_id: rule_set_id,
        exact_duplicate_count: 0,
        known_audio_identity_count: 0,
        alternate_recording_count: 0,
        new_track_count: 0,
        uncertain_count: 1,
        total_track_count: 1,
        evaluated_at: now,
        evaluator_version: "melodee.policy.novelty-v1".to_string(),
    })
    .expect("valid novelty aggregate");
    let decision = ReleasePolicyDecision::new(ReleasePolicyDecision {
        id: ReleasePolicyDecisionId::new(),
        release_id,
        rule_set_id,
        rule_set_version: 1,
        matched_rule_ids: vec!["unknown".to_string()],
        disposition: Disposition::Review,
        evidence_summary: "Release classification is Unknown".to_string(),
        source_fingerprint: context.claim.source_fingerprint.clone(),
        actor: PolicyActor::System {
            policy_rule_set_version: 1,
        },
        decided_at: now,
        completion_state: DecisionCompletionState::Pending,
        completed_at: None,
    })
    .expect("valid pending decision");
    PolicyEvaluationCommit {
        work_item_id: context.claim.item.id,
        release_id,
        source_fingerprint: context.claim.source_fingerprint.clone(),
        classification,
        novelty_evaluation,
        track_novelty,
        decision,
        now,
    }
}
