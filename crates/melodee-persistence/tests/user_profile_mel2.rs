//! MEL-2 PostgreSQL account/profile persistence evidence.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    AuthStore, ProfileUpdate, RegistrationRequest, UserProfileError, UserProfileStore,
};
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn registration_profile_update_audit_and_restart_durability() {
    let db = TestDb::create("mel2_profile_persistence").await;
    let store = UserProfileStore::new();
    let hash = melodee_auth::hash_password(b"correct horse battery staple").expect("hash");
    let user = store
        .register(
            &db.pool,
            &RegistrationRequest {
                username: "mel2-user",
                email: "mel2-user@example.test",
                password_hash: &hash,
                stream_signing_key: "stream-signing-key",
                now: OffsetDateTime::UNIX_EPOCH,
            },
        )
        .await
        .expect("register");
    assert!(
        user.is_admin,
        "the first registered user is the bootstrap admin"
    );
    assert_eq!(user.time_zone_id, "UTC");

    let updated = store
        .update(
            &db.pool,
            user.id,
            &ProfileUpdate {
                username: Some("mel2-renamed".to_owned()),
                email: Some("renamed@example.test".to_owned()),
                description: Some(Some("C# description survives".to_owned())),
                time_zone_id: Some("America/Chicago".to_owned()),
                preferred_language: Some(Some("en-US".to_owned())),
                preferred_theme: Some(Some("dark".to_owned())),
            },
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
        )
        .await
        .expect("profile update");
    assert_eq!(updated.username, "mel2-renamed");
    assert_eq!(
        updated.description.as_deref(),
        Some("C# description survives")
    );
    assert_eq!(updated.time_zone_id, "America/Chicago");
    assert_eq!(updated.preferred_language.as_deref(), Some("en-US"));
    assert_eq!(updated.preferred_theme.as_deref(), Some("dark"));
    assert!(
        updated.is_admin,
        "profile data cannot remove administrator authority"
    );

    let invalid = store
        .update(
            &db.pool,
            user.id,
            &ProfileUpdate {
                time_zone_id: Some("../../etc/passwd".to_owned()),
                ..ProfileUpdate::default()
            },
            OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
        )
        .await;
    assert!(matches!(
        invalid,
        Err(UserProfileError::InvalidValue("time zone id"))
    ));

    let persisted = AuthStore::new()
        .user_by_id(&db.pool, user.id)
        .await
        .expect("load after transaction")
        .expect("persisted user");
    assert_eq!(persisted.email, "renamed@example.test");
    let (audit_count, changed_fields): (i64, Vec<String>) = sqlx::query_as(
        "SELECT count(*)::bigint, changed_fields
           FROM native_user_profile_audits
          WHERE user_id=$1 AND event='profile_updated'
          GROUP BY changed_fields",
    )
    .bind(user.id)
    .fetch_one(&db.pool)
    .await
    .expect("profile audit");
    assert_eq!(audit_count, 1);
    assert!(changed_fields.iter().any(|field| field == "description"));
    assert!(changed_fields.iter().any(|field| field == "timeZoneId"));
    assert!(!format!("{changed_fields:?}").contains("C# description survives"));
    db.drop().await;
}

#[tokio::test]
async fn second_registration_is_not_an_admin() {
    let db = TestDb::create("mel2_second_registration").await;
    let store = UserProfileStore::new();
    let hash = melodee_auth::hash_password(b"correct horse battery staple").expect("hash");
    for (username, email) in [
        ("first", "first@example.test"),
        ("second", "second@example.test"),
    ] {
        let user = store
            .register(
                &db.pool,
                &RegistrationRequest {
                    username,
                    email,
                    password_hash: &hash,
                    stream_signing_key: "stream-signing-key",
                    now: OffsetDateTime::now_utc(),
                },
            )
            .await
            .expect("register");
        assert_eq!(user.is_admin, username == "first");
    }
    let admins: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM native_users WHERE is_admin")
            .fetch_one(&db.pool)
            .await
            .expect("admin count");
    assert_eq!(admins, 1);
    db.drop().await;
}
