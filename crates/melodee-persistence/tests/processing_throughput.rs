//! Exercise the same intake, claim and reporting paths the production workers use.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod common;
use common::{T0, TestDb, test_config};
use melodee_domain::ReleaseWorkItem;
use melodee_persistence::{
    WorkItemStore, WorkerId,
    processing::{MonitorLimits, bulk_report, register_bulk, stage_health},
};
use time::Duration;
use uuid::Uuid;

async fn arrive(db: &TestDb, path: &str, source: Option<Uuid>, minute: i64) -> Uuid {
    let item = ReleaseWorkItem::discovered(
        path.to_owned(),
        source,
        Uuid::now_v7(),
        T0 + Duration::minutes(minute),
    );
    WorkItemStore
        .create_or_get(&db.pool, &item)
        .await
        .unwrap()
        .0
        .id
}

#[tokio::test]
async fn ordinary_gets_three_slots_and_bulk_one_across_one_item_polls() {
    let db = TestDb::create("fair_slots").await;
    let names = vec!["bulk1".into(), "bulk2".into()];
    register_bulk(&db.pool, None, &names).await.unwrap();
    assert!(register_bulk(&db.pool, None, &names).await.is_err());
    let other = arrive(&db, "bulk1", Some(Uuid::now_v7()), 0).await;
    let ids: Vec<_> = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM release_work_items WHERE bulk_submission_id IS NOT NULL AND id=$1",
    )
    .bind(other)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(ids.is_empty());
    for name in [
        "bulk1", "bulk2", "new1", "new2", "new3", "new4", "new5", "new6",
    ] {
        arrive(&db, name, None, 0).await;
    }
    sqlx::query(
        "UPDATE release_work_items SET state='normalizing' WHERE source_library_id IS NULL",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let mut cfg = test_config();
    cfg.claim_batch_size = 1;
    let mut lanes = Vec::new();
    for _ in 0..8 {
        let rows = WorkItemStore
            .claim_normalizing(&db.pool, &WorkerId::new(), &cfg, T0)
            .await
            .unwrap();
        lanes.push(rows[0].source_path.starts_with("bulk"));
    }
    assert_eq!(
        lanes,
        [false, false, false, true, false, false, false, true]
    );
    db.drop().await;
}

#[tokio::test]
async fn stage_stall_and_recovery_use_handoffs_and_exclude_future_retries_and_manual_holds() {
    let db = TestDb::create("stage_monitor").await;
    let id = arrive(&db, "slow release", None, 0).await;
    sqlx::query("UPDATE release_work_items SET state='normalizing',updated_at=$2 WHERE id=$1")
        .bind(id)
        .bind(T0)
        .execute(&db.pool)
        .await
        .unwrap();
    let limits = MonitorLimits {
        max_attempts: 3,
        quiet_seconds: 60.,
        marker_seconds: 0.,
    };
    let check = |minutes| stage_health(&db.pool, T0 + Duration::minutes(minutes), limits);
    assert!(!check(14).await.unwrap().iter().any(|s| s.stalled));
    let rows = check(15).await.unwrap();
    let stage = rows.iter().find(|s| s.stalled).unwrap();
    assert_eq!(stage.oldest_release.as_deref(), Some("slow release"));
    let now = T0 + Duration::minutes(16);
    sqlx::query("INSERT INTO release_work_item_events(work_item_id,event_type,from_state,to_state,created_at,correlation_id) VALUES ($1,'transitioned','normalizing','validating',$2,$1)").bind(id).bind(now).execute(&db.pool).await.unwrap();
    let rows = check(17).await.unwrap();
    let stage = rows.iter().find(|s| s.stage == "normalization").unwrap();
    assert!(!stage.stalled);
    assert_eq!(stage.completed_hour, 1);
    assert_eq!(stage.last_completion, Some(now));
    sqlx::query("UPDATE release_work_items SET state='failed',next_attempt_at=$2 WHERE id=$1")
        .bind(id)
        .bind(T0 + Duration::days(2))
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(!check(1440).await.unwrap().iter().any(|s| s.stalled));
    sqlx::query(
        "UPDATE release_work_items SET state='needs_review',next_attempt_at=NULL WHERE id=$1",
    )
    .bind(id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(!check(3000).await.unwrap().iter().any(|s| s.stalled));
    db.drop().await;
}

#[tokio::test]
async fn cohort_report_freezes_the_overlap_window_and_counts_all_outcomes() {
    let db = TestDb::create("cohort_window").await;
    let batch = register_bulk(&db.pool, None, &["archive".into(), "bad".into()])
        .await
        .unwrap();
    let first = arrive(&db, "archive", None, 0).await;
    let ordinary = arrive(&db, "today", None, 1).await;
    assert_eq!(
        bulk_report(&db.pool, batch).await.unwrap()["cohorts"]["bulk"]["arrived"],
        1
    );
    let bad = arrive(&db, "bad", None, 2).await;
    for (id, min) in [(first, 60), (ordinary, 20)] {
        sqlx::query("UPDATE release_work_items SET state='indexing',state_reason='policy.disposition.accept',updated_at=$2 WHERE id=$1").bind(id).bind(T0+Duration::minutes(min)).execute(&db.pool).await.unwrap();
    }
    sqlx::query("UPDATE release_work_items SET state='needs_review',updated_at=$2 WHERE id=$1")
        .bind(bad)
        .bind(T0 + Duration::minutes(5))
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM release_work_items WHERE id=$1")
        .bind(bad)
        .execute(&db.pool)
        .await
        .unwrap();
    arrive(&db, "late", None, 75).await;
    let report = bulk_report(&db.pool, batch).await.unwrap();
    let ordinary = &report["cohorts"]["ordinary"];
    assert_eq!(ordinary["total"], 1);
    assert_eq!(ordinary["playable_within_30_minutes"], 1);
    assert_eq!(ordinary["slowest_playable_release"], "today");
    assert_eq!(report["cohorts"]["bulk"]["manual_holds"], 1);
    assert_eq!(report["elapsed_seconds"].as_f64(), Some(3600.));
    db.drop().await;
}
