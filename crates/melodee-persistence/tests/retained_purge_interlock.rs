//! Durable same-occurrence purge and retention exclusion.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use melodee_domain::ReleaseWorkItem;
use melodee_persistence::WorkItemStore;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn purge_guard_and_retention_are_mutually_exclusive_across_transactions() {
    let db = common::TestDb::create("retained_purge").await;
    let now = OffsetDateTime::now_utc();
    let item = ReleaseWorkItem::discovered("same".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &item).await.unwrap();
    let fence = Uuid::now_v7();
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO inbound_occurrence_purge_guards(work_item_id,source_library_id,source_path,guard_owner,fence_token) VALUES($1,NULL,'same','worker',$2)").bind(item.id).bind(fence).execute(&mut *tx).await.unwrap();
    let pool = db.pool.clone();
    let item_id = item.id;
    let waiting = tokio::spawn(async move {
        sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'same','generation',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())").bind(item_id).bind(format!("v3:{}","a".repeat(64))).execute(&pool).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO inbound_retained_sources%')").fetch_one(&db.pool).await.unwrap();if blocked {break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    tx.commit().await.unwrap();
    let moved = sqlx::query("UPDATE release_work_items SET current_path='redirected' WHERE id=$1")
        .bind(item.id)
        .execute(&db.pool)
        .await;
    assert!(
        matches!(moved,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55006"))
    );
    let duplicate=sqlx::query("INSERT INTO inbound_occurrence_purge_guards(work_item_id,source_library_id,source_path,guard_owner,fence_token) VALUES($1,NULL,'same','other',$2)").bind(Uuid::now_v7()).bind(Uuid::now_v7()).execute(&db.pool).await;
    assert!(
        matches!(duplicate,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55006"))
    );
    assert!(
        matches!(waiting.await.unwrap(),Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55006"))
    );
    assert!(
        sqlx::query(
            "UPDATE inbound_occurrence_purge_guards SET guard_owner='other' WHERE work_item_id=$1"
        )
        .bind(item.id)
        .execute(&db.pool)
        .await
        .is_err()
    );
    sqlx::query(
        "DELETE FROM inbound_occurrence_purge_guards WHERE work_item_id=$1 AND fence_token=$2",
    )
    .bind(item.id)
    .bind(fence)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'same','generation',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())").bind(item.id).bind(format!("v3:{}","a".repeat(64))).execute(&db.pool).await.unwrap();
    let rejected=sqlx::query("INSERT INTO inbound_occurrence_purge_guards(work_item_id,source_library_id,source_path,guard_owner,fence_token) VALUES($1,NULL,'same','worker',$2)").bind(item.id).bind(Uuid::now_v7()).execute(&db.pool).await;
    assert!(
        matches!(rejected,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55006"))
    );
    sqlx::query("INSERT INTO inbound_occurrence_purge_guards(work_item_id,source_library_id,source_path,guard_owner,fence_token) VALUES($1,NULL,'different','worker',$2)").bind(Uuid::now_v7()).bind(Uuid::now_v7()).execute(&db.pool).await.unwrap();
    db.drop().await;
}

#[tokio::test]
async fn competing_live_handoff_blocks_retention_but_historical_and_own_do_not() {
    let db = common::TestDb::create("retained_handoff").await;
    let now = OffsetDateTime::now_utc();
    let old = ReleaseWorkItem::discovered("same".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &old).await.unwrap();
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,intended_at) VALUES($1,'stage-old','generation','intent',clock_timestamp())").bind(old.id).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE release_work_items SET inbound_active=false WHERE id=$1")
        .bind(old.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let new = ReleaseWorkItem::discovered("same".to_owned(), None, Uuid::now_v7(), now);
    WorkItemStore.create(&db.pool, &new).await.unwrap();
    let insert = "INSERT INTO inbound_retained_sources(work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at) VALUES($1,'same','generation',$2,clock_timestamp(),clock_timestamp(),clock_timestamp())";
    let digest = format!("v3:{}", "a".repeat(64));
    let blocked = sqlx::query(insert)
        .bind(new.id)
        .bind(&digest)
        .execute(&db.pool)
        .await;
    assert!(
        matches!(blocked,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55006"))
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET status='inbound_removed',method='verified_copy',tree_hash=repeat('a',64),staged_at=clock_timestamp(),inbound_removed_at=clock_timestamp() WHERE work_item_id=$1").bind(old.id).execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,intended_at) VALUES($1,'stage-new','generation','intent',clock_timestamp())").bind(new.id).execute(&db.pool).await.unwrap();
    sqlx::query(insert)
        .bind(new.id)
        .bind(&digest)
        .execute(&db.pool)
        .await
        .unwrap();
    db.drop().await;
}
