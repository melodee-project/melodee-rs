//! Phase 5 dry-run persistence regressions not covered by the fenced atomic
//! evaluation, override, fault, and schema acceptance binaries.

#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::{T0, TestDb};
use melodee_domain::{
    Disposition, PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleSetId, PolicyRuleType,
    PolicyTestRunId, PolicyTestSummary,
};
use melodee_persistence::PolicyStore;
use serde_json::json;

fn dry_run_rule_set() -> PolicyRuleSet {
    PolicyRuleSet::new(PolicyRuleSet {
        id: PolicyRuleSetId::new(),
        name: "phase5-dry-run-acceptance".to_string(),
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
                id: "unknown".to_string(),
                rule_type: PolicyRuleType::UnknownClassification,
                predicate_json: json!({}),
                disposition: Disposition::Review,
                confidence_threshold: None,
            },
        ],
        created_at: T0,
        created_by: PolicyActor::System {
            policy_rule_set_version: 1,
        },
    })
    .expect("valid dry-run rule set")
}

#[tokio::test]
async fn dry_run_default_accept_has_no_synthetic_rule_bucket_or_decision_writes() {
    let db = TestDb::create("phase5_dry_run_default_accept").await;
    let store = PolicyStore;
    let rule_set = dry_run_rule_set();
    store
        .ensure_rule_set(&db.pool, &rule_set, T0)
        .await
        .expect("persist dry-run rule set");
    let summary = PolicyTestSummary::new(PolicyTestSummary {
        test_run_id: PolicyTestRunId::new(),
        rule_set_id: rule_set.id,
        rule_set_version: rule_set.version,
        total_releases_evaluated: 3,
        matched_rule_counts: vec![],
        existing_disposition_counts: vec![(None, 3)],
        disposition_counts: vec![(Disposition::Accept, 3)],
        destructive_impact_count: 0,
        // Store-level coverage does not need samples. Empty is preferable to
        // inventing a Release ID that the schema must correctly reject.
        sample_release_ids: vec![],
        ran_by: PolicyActor::Administrator {
            actor: "dry-run-admin".to_string(),
        },
        ran_at: T0,
    })
    .expect("valid single-bucket dry-run summary");

    store
        .store_test_run(&db.pool, &summary)
        .await
        .expect("persist dry-run summary only");
    let stored: (
        i32,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        i32,
        Vec<uuid::Uuid>,
    ) = sqlx::query_as(
        "SELECT total_releases_evaluated, matched_rule_counts,
                existing_disposition_counts, disposition_counts,
                destructive_impact_count, sample_release_ids
           FROM policy_test_runs WHERE id = $1",
    )
    .bind(summary.test_run_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("load exact dry-run buckets");
    assert_eq!(stored.0, 3);
    assert_eq!(stored.1, json!([]));
    assert_eq!(stored.2, json!([[null, 3]]));
    assert_eq!(stored.3, json!([["accept", 3]]));
    assert_eq!(stored.4, 0);
    assert!(stored.5.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT count(*) FROM policy_decisions)
              + (SELECT count(*) FROM release_classifications)
              + (SELECT count(*) FROM track_novelty)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("dry run writes no authoritative evaluation"),
        0
    );

    db.drop().await;
}
