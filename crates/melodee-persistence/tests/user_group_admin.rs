//! operator user/group administration persistence acceptance
//! (PRD-CLI-001, PRD-CLI-002, PRD-AUTH-004): fail-closed first-admin
//! bootstrap, and the access-group lifecycle the CLI exposes.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_auth::{PasswordHashAlgorithm, verify_password};
use melodee_persistence::{
    AuthLookup, AuthStore, AuthStoreError, BootstrapAdminRequest, GroupAdminError, GroupAdminStore,
};
use time::OffsetDateTime;
use uuid::Uuid;

async fn bootstrap(
    db: &TestDb,
    username: &str,
    password: &str,
) -> melodee_persistence::StoredNativeUser {
    let hash = bcrypt::hash(password, 4).expect("test bcrypt hash");
    AuthStore::new()
        .bootstrap_admin(
            &db.pool,
            &BootstrapAdminRequest {
                username,
                email: &format!("{username}@example.test"),
                password_hash: &hash,
                stream_signing_key: "bootstrap-stream-key",
                now: OffsetDateTime::now_utc(),
            },
        )
        .await
        .expect("bootstrap admin")
}

#[tokio::test]
async fn bootstrap_admin_creates_first_admin_then_fails_closed() {
    let db = TestDb::create("user_bootstrap").await;

    let admin = bootstrap(&db, "first-admin", "Correct-Horse-1!").await;
    assert!(admin.is_admin);
    assert!(admin.enabled);
    assert!(!admin.locked && !admin.blacklisted);
    assert!(admin.roles.iter().any(|role| role == "Administrator"));
    assert!(admin.roles.iter().any(|role| role == "HasStreamRole"));

    // The account authenticates through the normal login path immediately.
    let record = AuthStore::new()
        .user_for_auth(
            &db.pool,
            AuthLookup::Username(&melodee_persistence::normalize_login("first-admin")),
        )
        .await
        .expect("auth lookup")
        .expect("admin exists");
    assert!(record.user.can_authenticate());
    assert!(
        verify_password(
            b"Correct-Horse-1!",
            PasswordHashAlgorithm::Bcrypt,
            record.password_hash.expose_secret(),
        )
        .expect("verify")
    );

    // Fail closed: a second bootstrap is refused, leaving exactly one admin.
    let error = {
        let hash = bcrypt::hash("Other-Password-2!", 4).expect("test bcrypt hash");
        AuthStore::new()
            .bootstrap_admin(
                &db.pool,
                &BootstrapAdminRequest {
                    username: "second-admin",
                    email: "second-admin@example.test",
                    password_hash: &hash,
                    stream_signing_key: "bootstrap-stream-key-2",
                    now: OffsetDateTime::now_utc(),
                },
            )
            .await
            .expect_err("second bootstrap must fail closed")
    };
    assert!(matches!(error, AuthStoreError::AdminExists));
    let admins: (i64,) = sqlx::query_as("SELECT count(*) FROM native_users WHERE is_admin")
        .fetch_one(&db.pool)
        .await
        .expect("count admins");
    assert_eq!(admins.0, 1);

    // Blank identity material never reaches the database.
    let error = AuthStore::new()
        .bootstrap_admin(
            &db.pool,
            &BootstrapAdminRequest {
                username: "  ",
                email: "x@example.test",
                password_hash: "hash",
                stream_signing_key: "key",
                now: OffsetDateTime::now_utc(),
            },
        )
        .await
        .expect_err("blank username rejected");
    assert!(matches!(error, AuthStoreError::InvalidBootstrapRequest));

    db.drop().await;
}

#[tokio::test]
async fn group_lifecycle_fails_closed_with_members_and_bindings() {
    let db = TestDb::create("group_admin_lifecycle").await;
    let store = GroupAdminStore;
    let now = OffsetDateTime::now_utc();
    let member = bootstrap(&db, "group-member", "Member-Password-3!").await;

    let created = store
        .create_group(&db.pool, "Family", now)
        .await
        .expect("create group");
    assert_eq!(created.member_count, 0);
    assert_eq!(created.binding_count, 0);

    // Exact-name conflict fails closed (operator resolution stays unambiguous).
    assert!(matches!(
        store.create_group(&db.pool, "Family", now).await,
        Err(GroupAdminError::GroupExists)
    ));
    assert!(matches!(
        store.create_group(&db.pool, "   ", now).await,
        Err(GroupAdminError::InvalidGroupName)
    ));

    // Unknown users and groups fail closed before any mutation.
    assert!(matches!(
        store.add_user(&db.pool, "Family", "ghost", now).await,
        Err(GroupAdminError::UserNotFound)
    ));
    assert!(matches!(
        store.add_user(&db.pool, "Ghost", "group-member", now).await,
        Err(GroupAdminError::GroupNotFound)
    ));

    // Membership is idempotent and counted.
    assert!(
        store
            .add_user(&db.pool, "Family", "Group-Member", now)
            .await
            .expect("add member")
    );
    assert!(
        !store
            .add_user(&db.pool, "Family", "group-member", now)
            .await
            .expect("re-add is a no-op")
    );
    let listed = store.list_groups(&db.pool).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].member_count, 1);

    // Delete fails closed while members exist.
    assert!(matches!(
        store.delete_group(&db.pool, "Family").await,
        Err(GroupAdminError::GroupHasMembers)
    ));

    // Bindings also block deletion; removing members alone is not enough.
    let library = Uuid::now_v7();
    assert!(
        store
            .bind_library(&db.pool, "Family", library, now)
            .await
            .expect("bind library")
    );
    assert!(
        !store
            .bind_library(&db.pool, "Family", library, now)
            .await
            .expect("re-bind is a no-op")
    );
    assert!(
        store
            .remove_user(&db.pool, "Family", &member.username, now)
            .await
            .expect("remove member")
    );
    assert!(matches!(
        store.delete_group(&db.pool, "Family").await,
        Err(GroupAdminError::GroupHasBindings)
    ));
    assert!(
        store
            .unbind_library(&db.pool, "Family", library, now)
            .await
            .expect("unbind")
    );

    store
        .delete_group(&db.pool, "Family")
        .await
        .expect("empty group deletes");
    assert!(store.list_groups(&db.pool).await.expect("list").is_empty());

    db.drop().await;
}
