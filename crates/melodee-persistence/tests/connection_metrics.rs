//! Checkout wait and hold observations must cover different intervals.
#![allow(clippy::expect_used)]

mod common;

use common::{TestDb, test_config};
use melodee_persistence::{WorkItemStore, WorkerId, metrics};

fn sample(name: &str) -> metrics::MetricSnapshot {
    metrics::snapshot()
        .into_iter()
        .find(|entry| entry.name == name)
        .expect("metric")
}

#[tokio::test]
async fn checkout_wait_is_distinct_from_connection_hold() {
    let db = TestDb::create("checkout_metric").await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(common::database_options(&common::admin_url(), &db.name))
        .await
        .expect("limited pool");
    let held = pool.acquire().await.expect("reserve only connection");
    let before_acquire = sample("inspection_claim_acquire");
    let before_hold = sample("inspection_claim_hold");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let task_pool = pool.clone();
    let task = tokio::spawn(async move {
        let _ = started_tx.send(());
        WorkItemStore
            .claim(
                &task_pool,
                &WorkerId::new(),
                &test_config(),
                time::OffsetDateTime::now_utc(),
            )
            .await
    });
    started_rx.await.expect("claim started");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        sample("inspection_claim_hold").count,
        before_hold.count,
        "waiting to check out a connection does not record a hold"
    );
    drop(held);
    assert!(task.await.expect("join claim").expect("claim").is_empty());
    let after_acquire = sample("inspection_claim_acquire");
    let after_hold = sample("inspection_claim_hold");
    assert_eq!(after_acquire.count - before_acquire.count, 1);
    assert_eq!(after_hold.count - before_hold.count, 1);
    assert!(
        after_acquire.sum_micros - before_acquire.sum_micros >= 100_000,
        "checkout must include waiting for the reserved connection"
    );
    pool.close().await;
    db.drop().await;
}
