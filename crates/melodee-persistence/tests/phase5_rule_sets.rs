//! Versioned policy rule-set persistence and fail-closed behavior.

#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::{T0, TestDb};
use melodee_domain::{
    Confidence, Disposition, DomainInvariantError, PolicyActor, PolicyRule, PolicyRuleSet,
    PolicyRuleSetId, PolicyRuleType,
};
use melodee_persistence::{PolicyStore, PolicyStoreError};
use serde_json::json;

fn test_rule_set(id: PolicyRuleSetId) -> PolicyRuleSet {
    PolicyRuleSet::new(PolicyRuleSet {
        id,
        name: "phase5-acceptance-rules".to_string(),
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
                id: "redundant-greatest-hits".to_string(),
                rule_type: PolicyRuleType::GreatestHitsRedundancy,
                predicate_json: json!({ "required_traits": ["greatest_hits"] }),
                disposition: Disposition::Ignore,
                confidence_threshold: Some(Confidence::new(8_000).expect("valid confidence")),
            },
            PolicyRule {
                id: "uncertain-greatest-hits".to_string(),
                rule_type: PolicyRuleType::GreatestHitsUncertain,
                predicate_json: json!({ "required_traits": ["greatest_hits"] }),
                disposition: Disposition::Review,
                confidence_threshold: None,
            },
        ],
        created_at: T0,
        created_by: PolicyActor::System {
            policy_rule_set_version: 1,
        },
    })
    .expect("valid test rule set")
}

#[tokio::test]
async fn actual_rule_set_is_immutable_idempotent_and_conflicts_fail_closed() {
    let db = TestDb::create("phase5_rule_set_identity").await;
    let store = PolicyStore;
    let rule_set = test_rule_set(PolicyRuleSetId::new());

    let inserted = store
        .ensure_rule_set(&db.pool, &rule_set, T0)
        .await
        .expect("insert rule set");
    let adopted = store
        .ensure_rule_set(&db.pool, &rule_set, T0)
        .await
        .expect("adopt identical rule set retry");
    assert_eq!(inserted, rule_set);
    assert_eq!(adopted, rule_set);

    let conflicting = test_rule_set(PolicyRuleSetId::new());
    assert!(matches!(
        store.ensure_rule_set(&db.pool, &conflicting, T0).await,
        Err(PolicyStoreError::RuleSetConflict)
    ));

    let stored = store
        .load_rule_set(&db.pool, &rule_set.name)
        .await
        .expect("load rule set")
        .expect("rule set exists");
    assert_eq!(stored, rule_set);
    let row_count: (i64,) = sqlx::query_as("SELECT count(*) FROM policy_rule_sets")
        .fetch_one(&db.pool)
        .await
        .expect("count immutable rule sets");
    assert_eq!(row_count.0, 1);

    assert!(
        sqlx::query("UPDATE policy_rule_sets SET name = 'tampered' WHERE id = $1")
            .bind(rule_set.id.into_uuid())
            .execute(&db.pool)
            .await
            .is_err(),
        "a persisted rule-set version is immutable"
    );
    assert!(
        sqlx::query("DELETE FROM policy_rule_sets WHERE id = $1")
            .bind(rule_set.id.into_uuid())
            .execute(&db.pool)
            .await
            .is_err(),
        "a persisted rule-set version is append-only"
    );

    db.drop().await;
}

#[tokio::test]
async fn unsafe_or_duplicate_rules_are_rejected_before_database_writes() {
    let db = TestDb::create("phase5_rule_set_validation").await;
    let store = PolicyStore;
    let mut unsafe_rule_set = test_rule_set(PolicyRuleSetId::new());
    unsafe_rule_set.rules[1].disposition = Disposition::Accept;
    assert!(matches!(
        store.ensure_rule_set(&db.pool, &unsafe_rule_set, T0).await,
        Err(PolicyStoreError::Domain(
            DomainInvariantError::InvalidPolicyRuleDisposition
        ))
    ));

    let mut duplicate_rule_set = test_rule_set(PolicyRuleSetId::new());
    duplicate_rule_set.rules[2].id = duplicate_rule_set.rules[1].id.clone();
    assert!(matches!(
        store
            .ensure_rule_set(&db.pool, &duplicate_rule_set, T0)
            .await,
        Err(PolicyStoreError::Domain(
            DomainInvariantError::DuplicatePolicyRuleId
        ))
    ));

    let row_count: (i64,) = sqlx::query_as("SELECT count(*) FROM policy_rule_sets")
        .fetch_one(&db.pool)
        .await
        .expect("count rejected rule sets");
    assert_eq!(row_count.0, 0);

    db.drop().await;
}
