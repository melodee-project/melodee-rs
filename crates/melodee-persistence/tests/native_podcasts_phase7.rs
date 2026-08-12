//! Phase 7 podcast PostgreSQL integrity and tenancy acceptance.

#![allow(clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;

use time::OffsetDateTime;
use uuid::Uuid;

async fn user(db: &common::TestDb, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,$3,upper($3),$4,upper($4),$5,'bcrypt','podcast-key',$6,$6)",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(format!("{name}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert user");
    id
}

#[tokio::test]
async fn podcast_schema_is_tenant_scoped_bounded_and_cascades_playback_state() {
    let db = common::TestDb::create("phase7_podcasts").await;
    let owner = user(&db, "podcast-owner").await;
    let other = user(&db, "podcast-other").await;
    let channel_public = Uuid::from_u128(0x1020_3040_5060_4070_8090_a0b0_c0d0_e0f0);
    let channel: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) \
         VALUES($1,$2,'https://example.test/feed','Test Channel',now(),now()) RETURNING id",
    )
    .bind(channel_public)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("channel");
    assert!(
        sqlx::query("UPDATE native_podcast_channels SET public_api_id=$2 WHERE id=$1")
            .bind(channel)
            .bind(Uuid::now_v7())
            .execute(&db.pool)
            .await
            .is_err(),
        "public IDs cannot be rewritten"
    );
    assert!(
        sqlx::query(
            "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) \
             VALUES($1,$2,'https://example.test/feed','Duplicate',now(),now())",
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .execute(&db.pool)
        .await
        .is_err(),
        "exact feed retry cannot duplicate a subscription"
    );
    assert!(
        sqlx::query(
            "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) \
             VALUES($1,$2,'HTTPS://EXAMPLE.TEST/FEED','Case Duplicate',now(),now())",
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .execute(&db.pool)
        .await
        .is_err(),
        "case-only feed variants cannot duplicate a subscription"
    );
    let episode: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,duration_ms,created_at,updated_at) \
         VALUES($1,$2,'fixture-episode','Episode',60000,now(),now()) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("episode");
    sqlx::query(
        "INSERT INTO native_podcast_play_history(
            user_id,episode_id,played_at,client,is_now_playing,last_heartbeat_at
         ) VALUES($1,$2,now(),'fixture',true,now())",
    )
    .bind(owner)
    .bind(episode)
    .execute(&db.pool)
    .await
    .expect("active playback");
    assert!(
        sqlx::query(
            "INSERT INTO native_podcast_play_history(
                user_id,episode_id,played_at,client,is_now_playing,last_heartbeat_at
             ) VALUES($1,$2,now(),'second fixture',true,now())",
        )
        .bind(owner)
        .bind(episode)
        .execute(&db.pool)
        .await
        .is_err(),
        "one user cannot own two active podcast heartbeat rows"
    );
    assert!(
        sqlx::query(
            "INSERT INTO native_podcast_bookmarks(user_id,episode_id,position_seconds,created_at,updated_at) \
             VALUES($1,$2,-1,now(),now())",
        )
        .bind(owner)
        .bind(episode)
        .execute(&db.pool)
        .await
        .is_err(),
        "negative resume positions fail closed"
    );
    sqlx::query(
        "INSERT INTO native_podcast_bookmarks(user_id,episode_id,position_seconds,created_at,updated_at) \
         VALUES($1,$2,20,now(),now()),($3,$2,30,now(),now())",
    )
    .bind(owner)
    .bind(episode)
    .bind(other)
    .execute(&db.pool)
    .await
    .expect("tenant bookmarks");
    sqlx::query("DELETE FROM native_podcast_channels WHERE id=$1")
        .bind(channel)
        .execute(&db.pool)
        .await
        .expect("delete channel");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM native_podcast_bookmarks")
            .fetch_one(&db.pool)
            .await
            .expect("bookmark count"),
        0,
        "channel deletion cascades only derived podcast state"
    );
    db.drop().await;
}
