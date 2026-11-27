//! Exact inbound occurrence identity under hash collisions and concurrency.
#![allow(clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_domain::ReleaseWorkItem;
use melodee_persistence::WorkItemStore;
use uuid::Uuid;

#[tokio::test]
async fn hash_collisions_preserve_exact_identity_and_concurrent_idempotency() {
    let db = common::TestDb::create("occurrence_hash_collision").await;
    // Force every key into one bucket in this disposable database. Equality
    // must still use the complete path, including for concurrent inserts.
    sqlx::query("CREATE OR REPLACE FUNCTION ingest_occurrence_hash(path text) RETURNS bytea LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE RETURN decode(repeat('00',32),'hex')")
        .execute(&db.pool).await.expect("force collision");
    sqlx::query("REINDEX INDEX release_work_items_discovery_path_hash_idx")
        .execute(&db.pool)
        .await
        .expect("reindex forced hash");
    let a = ReleaseWorkItem::discovered("alpha".to_owned(), None, Uuid::now_v7(), common::T0);
    let b = ReleaseWorkItem::discovered("beta".to_owned(), None, Uuid::now_v7(), common::T0);
    let duplicate =
        ReleaseWorkItem::discovered("alpha".to_owned(), None, Uuid::now_v7(), common::T0);
    let store = WorkItemStore;
    let (first, second, third) = tokio::join!(
        store.create_or_get(&db.pool, &a),
        store.create_or_get(&db.pool, &b),
        store.create_or_get(&db.pool, &duplicate),
    );
    let first = first.expect("first");
    let second = second.expect("second");
    let third = third.expect("duplicate");
    assert_ne!(first.0.id, second.0.id);
    assert_eq!(first.0.id, third.0.id);
    assert_ne!(
        first.1, third.1,
        "only one identical occurrence is inserted"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_work_items")
        .fetch_one(&db.pool)
        .await
        .expect("count");
    assert_eq!(count, 2);
    assert!(
        WorkItemStore.create(&db.pool, &duplicate).await.is_err(),
        "direct insertion cannot bypass exact uniqueness"
    );
    db.drop().await;
}

#[tokio::test]
async fn long_path_keys_are_stored_and_compared_in_full() {
    let db = common::TestDb::create("occurrence_long_key").await;
    let path = format!("//melodee-unix-hex:{}", "ab".repeat(4_000));
    let item = ReleaseWorkItem::discovered(path.clone(), None, Uuid::now_v7(), common::T0);
    let (created, inserted) = WorkItemStore
        .create_or_get(&db.pool, &item)
        .await
        .expect("long occurrence");
    assert!(inserted);
    let found = WorkItemStore
        .get_by_source_identity(&db.pool, None, &path)
        .await
        .expect("lookup")
        .expect("found");
    assert_eq!(found.id, created.id);
    assert_eq!(found.source_path, path);
    db.drop().await;
}

#[tokio::test]
async fn legacy_and_literal_unicode_coexist_without_weakening_normal_uniqueness() {
    let db = common::TestDb::create("occurrence_legacy_unicode").await;
    let store = WorkItemStore;
    let old = ReleaseWorkItem::discovered("bad-�".to_owned(), None, Uuid::now_v7(), common::T0);
    let (old, _) = store.create_or_get(&db.pool, &old).await.expect("old item");
    sqlx::query("UPDATE release_work_items SET discovery_path_format=0 WHERE id=$1")
        .bind(old.id)
        .execute(&db.pool)
        .await
        .expect("legacy format");
    let literal_a =
        ReleaseWorkItem::discovered("bad-�".to_owned(), None, Uuid::now_v7(), common::T0);
    let literal_b =
        ReleaseWorkItem::discovered("bad-�".to_owned(), None, Uuid::now_v7(), common::T0);
    let (first, second) = tokio::join!(
        store.create_or_get(&db.pool, &literal_a),
        store.create_or_get(&db.pool, &literal_b)
    );
    let first = first.expect("first literal");
    let second = second.expect("second literal");
    assert_eq!(first.0.id, second.0.id);
    assert_ne!(first.0.id, old.id);
    assert_ne!(first.1, second.1);
    assert!(
        sqlx::query("UPDATE release_work_items SET discovery_path_format=1 WHERE id=$1")
            .bind(old.id)
            .execute(&db.pool)
            .await
            .is_err(),
        "operator cannot accidentally merge competing occurrences"
    );
    db.drop().await;
}
