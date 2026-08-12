//! Phase 7 native-v1 genre preference persistence acceptance.

#![allow(clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;

use melodee_persistence::{NativeUserStateMutation, NativeUserStateStore};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn native_genre_preferences_preserve_display_value_and_independent_flags() {
    let db = common::TestDb::create("phase7_genre_state").await;
    let user_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO native_users (
           id, public_api_id, username, normalized_username, email, normalized_email,
           password_hash, password_hash_algorithm, stream_signing_key, created_at, updated_at
         ) VALUES ($1, $2, 'genre-user', 'GENRE-USER', 'genre@example.test',
           'GENRE@EXAMPLE.TEST', 'hash', 'bcrypt', 'stream-key', $3, $3)",
    )
    .bind(user_id)
    .bind(Uuid::now_v7())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert user");

    let starred = NativeUserStateStore::mutate_genre(
        &db.pool,
        user_id,
        "Post-Rock",
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star genre");
    assert!(starred.starred);
    assert!(!starred.hated);

    let hated = NativeUserStateStore::mutate_genre(
        &db.pool,
        user_id,
        "post-rock",
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate same normalized genre");
    assert!(hated.starred);
    assert!(hated.hated);
    assert_eq!(hated.genre_name, "post-rock");

    assert_eq!(
        NativeUserStateStore::genre_preferences(
            &db.pool,
            user_id,
            NativeUserStateMutation::Starred(true),
        )
        .await
        .expect("starred genres"),
        vec!["post-rock"]
    );
    assert_eq!(
        NativeUserStateStore::genre_preferences(
            &db.pool,
            user_id,
            NativeUserStateMutation::Hated(true),
        )
        .await
        .expect("hated genres"),
        vec!["post-rock"]
    );
    db.drop().await;
}
