//! migration 0022 + user/group/avatar administration integrity
//! tests (ADR-0134 §2): constraint enforcement, cascade, fail-closed group
//! deletion, ACL predicate effects, and admin user lifecycle.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{
    AuthStore, GroupAdminError, GroupAdminStore, NativeUserAdminUpdate, NativeUserCreate,
    NativeUserImport, UserAdminError, UserAdminStore,
};
use time::OffsetDateTime;
use uuid::Uuid;

const T0: OffsetDateTime = common::T0;

async fn import_user(db: &TestDb, username: &str) -> (Uuid, Uuid) {
    let id = Uuid::now_v7();
    let public_api_id = Uuid::now_v7();
    let normalized = username.to_uppercase();
    let email = format!("{username}@example.test");
    let normalized_email = email.to_uppercase();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id,
                public_api_id,
                username,
                normalized_username: &normalized,
                email: &email,
                normalized_email: &normalized_email,
                password_hash: "$2b$12$BP5HSYGph9ZF1C7Jco2fieFhmgb1LzWDjT5V8ZrL4Pk3C9Drn5lDW",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasStreamRole".to_string()],
                created_at: T0,
                updated_at: T0,
            },
        )
        .await
        .expect("import user");
    (id, public_api_id)
}

#[tokio::test]
async fn avatar_table_enforces_content_type_and_size_bounds() {
    let db = TestDb::create("phase9_avatar_constraints").await;
    let (user_id, _) = import_user(&db, "avatar-constraints").await;

    // Only jpeg/png are accepted.
    let result = sqlx::query(
        "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
         VALUES ($1, 'image/gif', $2, now())",
    )
    .bind(user_id)
    .bind(
        melodee_persistence::asset_files::put(&db.pool, b"x")
            .await
            .expect("store fixture"),
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "gif content type must be rejected");

    // Empty image is rejected.
    let result = sqlx::query(
        "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
         VALUES ($1, 'image/png', '\\x', now())",
    )
    .bind(user_id)
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "empty image must be rejected");

    // Over 1 MiB is rejected.
    let oversized = vec![0x55_u8; 1024 * 1024 + 1];
    let result = sqlx::query(
        "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
         VALUES ($1, 'image/png', $2, now())",
    )
    .bind(user_id)
    .bind(
        melodee_persistence::asset_files::put(&db.pool, &oversized)
            .await
            .expect("store fixture"),
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "oversized image must be rejected");

    // Exactly 1 MiB is accepted.
    let at_bound = vec![0x55_u8; 1024 * 1024];
    sqlx::query(
        "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
         VALUES ($1, 'image/png', $2, now())",
    )
    .bind(user_id)
    .bind(
        melodee_persistence::asset_files::put(&db.pool, &at_bound)
            .await
            .expect("store fixture"),
    )
    .execute(&db.pool)
    .await
    .expect("1 MiB avatar is in bounds");

    // Unknown user is rejected by the FK.
    let result = sqlx::query(
        "INSERT INTO native_user_avatars (user_id, content_type, asset_key, updated_at)
         VALUES ($1, 'image/png', $2, now())",
    )
    .bind(Uuid::now_v7())
    .bind(
        melodee_persistence::asset_files::put(&db.pool, b"x")
            .await
            .expect("store fixture"),
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "orphan avatar must be rejected");
    db.drop().await;
}

#[tokio::test]
async fn avatar_upsert_round_trips_and_cascades_with_user() {
    let db = TestDb::create("phase9_avatar_roundtrip").await;
    let (user_id, _) = import_user(&db, "avatar-roundtrip").await;
    let store = UserAdminStore::new();

    assert!(
        store
            .avatar_by_user_id(&db.pool, user_id)
            .await
            .expect("read")
            .is_none()
    );

    let stored = store
        .set_avatar(&db.pool, user_id, "image/png", &[1, 2, 3], T0)
        .await
        .expect("set avatar");
    assert_eq!(stored.content_type, "image/png");
    assert_eq!(stored.image, vec![1, 2, 3]);
    assert_eq!(stored.updated_at, T0);

    // Upsert replaces bytes and advances updated_at.
    let later = T0 + time::Duration::hours(1);
    let updated = store
        .set_avatar(&db.pool, user_id, "image/jpeg", &[9, 9], later)
        .await
        .expect("replace avatar");
    assert_eq!(updated.content_type, "image/jpeg");
    assert_eq!(updated.image, vec![9, 9]);
    assert_eq!(updated.updated_at, later);

    // Store-level validation rejects bad inputs before the CHECK does.
    assert!(matches!(
        store
            .set_avatar(&db.pool, user_id, "image/webp", &[1], T0)
            .await,
        Err(UserAdminError::InvalidRequest(_))
    ));
    assert!(matches!(
        store
            .set_avatar(&db.pool, user_id, "image/png", &[], T0)
            .await,
        Err(UserAdminError::InvalidRequest(_))
    ));

    // Deleting the user cascades the avatar row.
    sqlx::query("DELETE FROM native_users WHERE id=$1")
        .bind(user_id)
        .execute(&db.pool)
        .await
        .expect("delete user");
    let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM native_user_avatars")
        .fetch_one(&db.pool)
        .await
        .expect("count avatars");
    assert_eq!(remaining.0, 0);
    db.drop().await;
}

#[tokio::test]
async fn group_crud_is_fail_closed_and_bounded() {
    let db = TestDb::create("phase9_group_crud").await;
    let store = GroupAdminStore::new();

    let group = store
        .create_group(&db.pool, "  Listeners  ", T0)
        .await
        .expect("create group");
    assert_eq!(group.name, "Listeners");
    assert_eq!(group.member_count, 0);
    assert_eq!(group.binding_count, 0);

    // Exact-name conflicts and invalid names fail closed.
    assert!(matches!(
        store.create_group(&db.pool, "Listeners", T0).await,
        Err(GroupAdminError::GroupExists)
    ));
    assert!(matches!(
        store.create_group(&db.pool, "   ", T0).await,
        Err(GroupAdminError::InvalidGroupName)
    ));
    let long = "x".repeat(201);
    assert!(matches!(
        store.create_group(&db.pool, &long, T0).await,
        Err(GroupAdminError::InvalidGroupName)
    ));

    let renamed = store
        .rename_group(
            &db.pool,
            group.public_api_id,
            "Audited",
            T0 + time::Duration::minutes(1),
        )
        .await
        .expect("rename");
    assert_eq!(renamed.name, "Audited");
    assert!(renamed.updated_at > group.created_at);
    assert!(matches!(
        store
            .rename_group(&db.pool, Uuid::now_v7(), "Nope", T0)
            .await,
        Err(GroupAdminError::GroupNotFound)
    ));

    // Empty group deletes cleanly; deleting again is GroupNotFound.
    store
        .delete_group_by_public_id(&db.pool, group.public_api_id)
        .await
        .expect("delete empty group");
    assert!(matches!(
        store
            .delete_group_by_public_id(&db.pool, group.public_api_id)
            .await,
        Err(GroupAdminError::GroupNotFound)
    ));
    db.drop().await;
}

#[tokio::test]
async fn group_membership_and_bindings_drive_the_acl_predicate() {
    let db = TestDb::create("phase9_group_acl").await;
    let store = GroupAdminStore::new();
    let auth = AuthStore::new();
    let (_, user_public) = import_user(&db, "acl-member").await;
    let (_, outsider_public) = import_user(&db, "acl-outsider").await;
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM native_users WHERE public_api_id=$1")
        .bind(user_public)
        .fetch_one(&db.pool)
        .await
        .expect("user id");
    let outsider_id: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE public_api_id=$1")
            .bind(outsider_public)
            .fetch_one(&db.pool)
            .await
            .expect("outsider id");
    let library_id = Uuid::now_v7();

    let group = store
        .create_group(&db.pool, "ACL", T0)
        .await
        .expect("create group");

    // No binding: both users retain the captured open access.
    assert!(
        auth.can_access_library(&db.pool, user_id, Some(library_id))
            .await
            .expect("acl")
    );

    assert!(
        store
            .bind_library_by_public_id(&db.pool, group.public_api_id, library_id, T0)
            .await
            .expect("bind library")
    );
    // Binding exists, no membership: both users lose access (fail closed).
    assert!(
        !auth
            .can_access_library(&db.pool, user_id, Some(library_id))
            .await
            .expect("acl")
    );
    // Idempotent rebind reports no change.
    assert!(
        !store
            .bind_library_by_public_id(&db.pool, group.public_api_id, library_id, T0)
            .await
            .expect("rebind is idempotent")
    );

    // Deleting a bound group fails closed.
    assert!(matches!(
        store
            .delete_group_by_public_id(&db.pool, group.public_api_id)
            .await,
        Err(GroupAdminError::GroupHasBindings)
    ));

    assert!(
        store
            .add_member_by_public_ids(&db.pool, group.public_api_id, user_public, T0)
            .await
            .expect("add member")
    );
    assert!(
        auth.can_access_library(&db.pool, user_id, Some(library_id))
            .await
            .expect("acl")
    );
    assert!(
        !auth
            .can_access_library(&db.pool, outsider_id, Some(library_id))
            .await
            .expect("acl")
    );
    // A group with members also cannot be deleted.
    assert!(matches!(
        store
            .delete_group_by_public_id(&db.pool, group.public_api_id)
            .await,
        Err(GroupAdminError::GroupHasMembers)
    ));

    let detail = store
        .group_detail(&db.pool, group.public_api_id)
        .await
        .expect("detail")
        .expect("group exists");
    assert_eq!(detail.members.len(), 1);
    assert_eq!(detail.members[0].public_api_id, user_public);
    assert_eq!(detail.library_ids, vec![library_id]);

    // Removing the last binding restores open access; removing a non-member
    // or non-binding reports false.
    assert!(
        store
            .unbind_library_by_public_id(&db.pool, group.public_api_id, library_id, T0)
            .await
            .expect("unbind")
    );
    assert!(
        auth.can_access_library(&db.pool, outsider_id, Some(library_id))
            .await
            .expect("acl")
    );
    assert!(
        !store
            .unbind_library_by_public_id(&db.pool, group.public_api_id, library_id, T0)
            .await
            .expect("unbind again")
    );
    assert!(
        !store
            .remove_member_by_public_ids(&db.pool, group.public_api_id, outsider_public, T0)
            .await
            .expect("non-member removal is false")
    );
    assert!(
        store
            .remove_member_by_public_ids(&db.pool, group.public_api_id, user_public, T0)
            .await
            .expect("remove member")
    );
    store
        .delete_group_by_public_id(&db.pool, group.public_api_id)
        .await
        .expect("empty group deletes");
    db.drop().await;
}

#[tokio::test]
async fn admin_create_user_rejects_duplicates_and_stores_bcrypt() {
    let db = TestDb::create("phase9_admin_create_user").await;
    let store = UserAdminStore::new();
    let roles = vec!["HasStreamRole".to_string()];

    let user = store
        .create_user(
            &db.pool,
            &NativeUserCreate {
                username: " New User ",
                email: "new@example.test",
                password_hash: "$2b$12$BP5HSYGph9ZF1C7Jco2fieFhmgb1LzWDjT5V8ZrL4Pk3C9Drn5lDW",
                stream_signing_key: "admin-created-stream-key",
                is_admin: false,
                is_editor: true,
                roles: &roles,
            },
            T0,
        )
        .await
        .expect("create user");
    assert_eq!(user.username, "New User");
    assert!(user.enabled);
    assert!(user.is_editor);
    assert!(!user.is_admin);
    assert_eq!(user.roles, roles);
    assert_eq!(user.auth_revision, 1);

    // Duplicate normalized username fails closed.
    assert!(matches!(
        store
            .create_user(
                &db.pool,
                &NativeUserCreate {
                    username: "new user",
                    email: "other@example.test",
                    password_hash: "$2b$12$BP5HSYGph9ZF1C7Jco2fieFhmgb1LzWDjT5V8ZrL4Pk3C9Drn5lDW",
                    stream_signing_key: "another-stream-key",
                    is_admin: false,
                    is_editor: false,
                    roles: &roles,
                },
                T0,
            )
            .await,
        Err(UserAdminError::UserConflict)
    ));

    // Invalid inputs are rejected before insert.
    for (username, hash) in [("", "x"), ("ok", "not-a-bcrypt-hash")] {
        assert!(matches!(
            store
                .create_user(
                    &db.pool,
                    &NativeUserCreate {
                        username,
                        email: "x@example.test",
                        password_hash: hash,
                        stream_signing_key: "k",
                        is_admin: false,
                        is_editor: false,
                        roles: &roles,
                    },
                    T0,
                )
                .await,
            Err(UserAdminError::InvalidRequest(_))
        ));
    }
    db.drop().await;
}

#[tokio::test]
async fn admin_update_user_flags_roles_and_revocation() {
    let db = TestDb::create("phase9_admin_update_user").await;
    let store = UserAdminStore::new();
    let (_, public_api_id) = import_user(&db, "update-target").await;

    // Flag + role update bumps auth_revision and replaces roles.
    let updated = store
        .update_user(
            &db.pool,
            public_api_id,
            &NativeUserAdminUpdate {
                is_admin: Some(true),
                roles: Some(vec!["Administrator".to_string()]),
                ..NativeUserAdminUpdate::default()
            },
            T0 + time::Duration::minutes(1),
        )
        .await
        .expect("update user");
    assert!(updated.is_admin);
    assert_eq!(updated.roles, vec!["Administrator".to_string()]);
    assert_eq!(updated.auth_revision, 2);

    // Disable revokes any refresh families and blocks authentication.
    let disabled = store
        .update_user(
            &db.pool,
            public_api_id,
            &NativeUserAdminUpdate {
                enabled: Some(false),
                ..NativeUserAdminUpdate::default()
            },
            T0 + time::Duration::minutes(2),
        )
        .await
        .expect("disable user");
    assert!(!disabled.enabled);
    assert!(!disabled.can_authenticate());

    // Re-enable restores authentication eligibility.
    let enabled = store
        .update_user(
            &db.pool,
            public_api_id,
            &NativeUserAdminUpdate {
                enabled: Some(true),
                ..NativeUserAdminUpdate::default()
            },
            T0 + time::Duration::minutes(3),
        )
        .await
        .expect("enable user");
    assert!(enabled.can_authenticate());

    assert!(matches!(
        store
            .update_user(
                &db.pool,
                Uuid::now_v7(),
                &NativeUserAdminUpdate::default(),
                T0,
            )
            .await,
        Err(UserAdminError::NotFound)
    ));
    assert!(matches!(
        store
            .update_user(
                &db.pool,
                public_api_id,
                &NativeUserAdminUpdate {
                    roles: Some(vec![" ".to_string()]),
                    ..NativeUserAdminUpdate::default()
                },
                T0,
            )
            .await,
        Err(UserAdminError::InvalidRequest(_))
    ));
    db.drop().await;
}
