#![allow(clippy::expect_used)]

mod common;

#[tokio::test]
async fn fresh_installations_are_distinct_and_repeated_reads_are_stable() {
    let first = common::TestDb::create_unmigrated("install_a").await;
    melodee_persistence::migrate(&first.pool)
        .await
        .expect("fresh first installation");
    let second = common::TestDb::create_unmigrated("install_b").await;
    melodee_persistence::migrate(&second.pool)
        .await
        .expect("fresh second installation");
    let id = melodee_persistence::installation_id(&first.pool)
        .await
        .expect("identity");
    assert!(!id.is_nil());
    assert_eq!(
        id,
        melodee_persistence::installation_id(&first.pool)
            .await
            .expect("stable identity")
    );
    assert_ne!(
        id,
        melodee_persistence::installation_id(&second.pool)
            .await
            .expect("distinct identity")
    );
    assert!(
        sqlx::query("INSERT INTO server_installation_identity (singleton) VALUES (true)")
            .execute(&first.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO server_installation_identity (singleton) VALUES (false)")
            .execute(&first.pool)
            .await
            .is_err()
    );
    // A missing singleton is an error, never an implicit identity rotation.
    sqlx::query("DELETE FROM server_installation_identity")
        .execute(&first.pool)
        .await
        .expect("remove test row");
    assert!(
        melodee_persistence::installation_id(&first.pool)
            .await
            .is_err()
    );
    first.drop().await;
    second.drop().await;
}
