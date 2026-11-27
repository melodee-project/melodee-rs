//! Read-only operator listing preserves exact retained occurrence identities.
#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use common::TestDb;
use std::{collections::BTreeMap, process::Command};
use uuid::Uuid;

fn list(root: &std::path::Path, url: &str, after: Option<&str>) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(melodee_persistence::ENV_DATABASE_URL, url)
        .args(["ingest", "retained-obligations", "--limit", "1"]);
    if let Some(after) = after {
        command.args(["--after", after]);
    }
    let output = command.output().expect("run retained listing");
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON output")
}

#[tokio::test]
async fn retained_listing_preserves_encoded_paths_lease_state_and_keyset_pages() {
    let db = TestDb::create("cli_retained_list").await;
    let temp = tempfile::tempdir().expect("isolated command directory");
    std::fs::write(temp.path().join(".env"), "").expect("prevent ancestor dotenv");
    let paths = [
        "//melodee-unix-hex:ff0a",
        "//melodee-windows-u16:d800",
        "legacy\nname",
    ];
    let mut expected = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        let id = Uuid::now_v7();
        let leased = index == 1;
        sqlx::query("INSERT INTO inbound_retained_sources(work_item_id,source_path,source_path_format,source_generation,content_digest,next_reobserve_at,baseline_change_pending,baseline_unreadable_reason,baseline_unreadable_count,last_retry_reason,retry_count,lease_owner,lease_token,lease_expires_at,created_at,updated_at) VALUES ($1,$2,$3,'fixture','v3:'||repeat('a',64),now(),$4,$5,$6,$7,$8,$9,$10,CASE WHEN $11 THEN now()+interval '1 hour' ELSE NULL END,now(),now())")
            .bind(id).bind(path).bind(if index == 2 {0_i16} else {1_i16})
            .bind(index == 0).bind(if index == 1 {Some("unreadable")} else {None::<&str>})
            .bind(if index == 1 {2_i32} else {0_i32})
            .bind(if index == 0 {Some("awaiting_intake")} else {None::<&str>})
            .bind(if index == 0 {1_i32} else {0_i32})
            .bind(if leased {Some("fixture-worker")} else {None::<&str>})
            .bind(if leased {Some(Uuid::now_v7())} else {None})
            .bind(leased).execute(&db.pool).await.expect("insert obligation");
        expected.insert(id.to_string(), (path.to_string(), leased));
    }
    let before: Vec<(Uuid, time::OffsetDateTime)> = sqlx::query_as(
        "SELECT work_item_id,updated_at FROM inbound_retained_sources ORDER BY work_item_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read before");
    let url = common::database_url(&db.name);
    let mut after: Option<String> = None;
    let mut found = BTreeMap::new();
    let mut completed = false;
    for _ in 0..=paths.len() {
        let page = list(temp.path(), &url, after.as_deref());
        let rows = page["items"].as_array().expect("items array");
        assert!(rows.len() <= 1);
        for row in rows {
            let id = row["work_item_id"].as_str().expect("work item ID");
            let path = row["source_path"].as_str().expect("stored path");
            assert!(
                row.get("lease_token").is_none(),
                "lease token is not operator output"
            );
            assert!(
                row["next_reobserve_at"].as_str().is_some(),
                "due time must be readable"
            );
            found.insert(
                id.to_owned(),
                (
                    path.to_owned(),
                    row["lease_active"].as_bool().expect("lease status"),
                ),
            );
        }
        match page["nextAfter"].as_str() {
            Some(cursor) => after = Some(cursor.to_owned()),
            None => {
                completed = true;
                break;
            }
        }
    }
    assert!(
        completed,
        "pagination must reach its end within the expected bound"
    );
    assert_eq!(
        found, expected,
        "all exact paths and lease states appear once"
    );
    let after: Vec<(Uuid, time::OffsetDateTime)> = sqlx::query_as(
        "SELECT work_item_id,updated_at FROM inbound_retained_sources ORDER BY work_item_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read after");
    assert_eq!(before, after, "listing must not mutate obligations");
    db.drop().await;
}
