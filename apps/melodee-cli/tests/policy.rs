//! CLI-level dry-run regression against one real catalog handoff.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::process::Command;

use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use phase5_common::seed_cataloged_release;

#[tokio::test]
async fn policy_test_persists_default_accept_without_a_synthetic_matched_rule() {
    let db = TestDb::create("cli_policy_test_single_disposition").await;
    let fixture = seed_cataloged_release(&db, 1, CatalogFixturePresentation::default()).await;
    let now = time::OffsetDateTime::now_utc();
    melodee_persistence::PolicyStore
        .record_release_catalog_trait(
            &db.pool,
            &melodee_persistence::PolicyReleaseCatalogTraitAssertion {
                id: uuid::Uuid::now_v7(),
                release_id: fixture.release_id(),
                trait_: melodee_domain::ReleaseTrait::Studio,
                source_version: "cli-policy-test-v1".to_owned(),
                actor: "cli-policy-test-curator".to_owned(),
                reason: "catalog evidence confirms an ordinary Studio Release".to_owned(),
                evidence: serde_json::json!({ "catalog_record": "cli-studio-1" }),
                created_at: now,
            },
        )
        .await
        .expect("persist ordinary Studio catalog assertion");
    let database_url = common::admin_url().replace("/postgres", &format!("/{}", db.name));
    let contexts = melodee_persistence::PolicyStore
        .load_test_contexts(&db.pool, 1)
        .await
        .expect("load bounded CLI policy test context");
    assert_eq!(contexts.len(), 1);

    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args([
            "policy",
            "test",
            "--limit",
            "1",
            "--actor",
            "cli-policy-test-admin",
        ])
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .env("MELODEE__POLICY__RULE_SET_VERSION", "1")
        .env("MELODEE__POLICY__BATCH_SIZE", "1")
        .env("MELODEE__POLICY__CONCURRENCY", "1")
        .output()
        .expect("run bounded policy test command");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("policy output is UTF-8");
    assert!(stdout.contains("test: evaluated=1 errors=0"));
    assert!(stdout.contains("existing_dispositions: {\"none\": 1}"));

    let row: (
        i32,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT total_releases_evaluated, disposition_counts,
                matched_rule_counts, existing_disposition_counts, ran_by
           FROM policy_test_runs",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read durable policy test summary");
    assert_eq!(row.0, 1);
    for counts in [&row.1, &row.3] {
        let buckets = counts.as_array().expect("summary counts are arrays");
        assert!(!buckets.is_empty());
        assert!(buckets.iter().all(|bucket| {
            bucket
                .as_array()
                .and_then(|pair| pair.get(1))
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0)
        }));
    }
    assert_eq!(row.1, serde_json::json!([["accept", 1]]));
    assert_eq!(row.2, serde_json::json!([]));
    assert_eq!(row.3, serde_json::json!([[null, 1]]));
    assert_eq!(
        row.4,
        serde_json::json!({
            "authority": "administrator",
            "actor": "cli-policy-test-admin",
        })
    );

    fixture.assert_media_paths_absent();
    db.drop().await;
}
