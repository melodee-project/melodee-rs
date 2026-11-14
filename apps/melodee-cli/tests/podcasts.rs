//! CLI evidence: `melodee podcasts stuck|recover` lists and recovers
//! stuck/orphaned podcast downloads through the same durable lease predicates
//! and owned-file reconciliation the podcast worker uses (PRD-CLI-002: a CLI
//! command never bypasses leases or ownership controls).

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;
use uuid::Uuid;

async fn seed_owner(db: &TestDb, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,$3,upper($3),$4,upper($4),$5,'bcrypt','podcast-cli-key',now(),now())",
    )
    .bind(id)
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(format!("{name}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .execute(&db.pool)
    .await
    .expect("insert user");
    id
}

async fn seed_episode(db: &TestDb, channel: i64, feed_item_id: &str, title: &str) -> (i64, Uuid) {
    let public_id = Uuid::now_v7();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,\
         enclosure_url,created_at,updated_at) \
         VALUES($1,$2,$3,$4,'https://media.example.test/episode.mp3',now(),now()) RETURNING id",
    )
    .bind(public_id)
    .bind(channel)
    .bind(feed_item_id)
    .bind(title)
    .fetch_one(&db.pool)
    .await
    .expect("insert episode");
    (id, public_id)
}

fn run_cli(database_url: &str, storage_root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .env("MELODEE__PODCAST__STORAGE_ROOT", storage_root)
        .output()
        .expect("run podcasts CLI command");
    let stdout = String::from_utf8(output.stdout).expect("CLI output is UTF-8");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[tokio::test]
async fn podcasts_recovery_requeues_only_expired_leases_and_removes_only_orphans() {
    let db = TestDb::create("cli_podcasts_recovery").await;
    let owner = seed_owner(&db, "podcast-cli-owner").await;
    let channel = sqlx::query_scalar::<_, i64>(
        "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,created_at,updated_at) \
         VALUES($1,$2,'https://feeds.example.test/show','CLI Recovery Show',now(),now()) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert channel");

    let storage = tempfile::tempdir().expect("podcast storage");
    std::fs::write(
        storage.path().join(".melodee-podcast-root"),
        b"melodee-podcast-root-v1\n",
    )
    .expect("ownership marker");

    // Stuck: Downloading under a lease that expired one second ago.
    let (stuck_id, stuck_public) = seed_episode(&db, channel, "cli-stuck", "Stuck Episode").await;
    let stuck_worker = Uuid::now_v7();
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloading',download_lease_owner=$2,\
         download_lease_expires_at=now()-interval '1 second',download_attempt_count=2 WHERE id=$1",
    )
    .bind(stuck_id)
    .bind(stuck_worker)
    .execute(&db.pool)
    .await
    .expect("expire lease");
    let stuck_tmp = storage
        .path()
        .join(format!("{stuck_public}.{stuck_worker}.tmp"));
    std::fs::write(&stuck_tmp, b"partial stuck download").expect("stuck temp");

    // Live: Downloading under a healthy lease — recovery must never touch it.
    let (live_id, live_public) = seed_episode(&db, channel, "cli-live", "Live Episode").await;
    let live_worker = Uuid::now_v7();
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloading',download_lease_owner=$2,\
         download_lease_expires_at=now()+interval '1 hour' WHERE id=$1",
    )
    .bind(live_id)
    .bind(live_worker)
    .execute(&db.pool)
    .await
    .expect("live lease");
    let live_tmp = storage
        .path()
        .join(format!("{live_public}.{live_worker}.tmp"));
    std::fs::write(&live_tmp, b"live in-flight download").expect("live temp");

    // Failed: permanently failed after exhausting attempts.
    let (failed_id, _) = seed_episode(&db, channel, "cli-failed", "Failed Episode").await;
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Failed',download_attempt_count=5,\
         download_error='outbound podcast request failed' WHERE id=$1",
    )
    .bind(failed_id)
    .execute(&db.pool)
    .await
    .expect("failed state");

    // Downloaded: referenced media file must survive reconciliation.
    let (downloaded_id, downloaded_public) =
        seed_episode(&db, channel, "cli-downloaded", "Downloaded Episode").await;
    let media_path = storage.path().join(format!("{downloaded_public}.media"));
    std::fs::write(&media_path, b"finished podcast media").expect("media file");
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloaded',local_path=$2,\
         downloaded_bytes=21,downloaded_at=now() WHERE id=$1",
    )
    .bind(downloaded_id)
    .bind(media_path.to_string_lossy().as_ref())
    .execute(&db.pool)
    .await
    .expect("downloaded state");

    // Orphan: no database row references this temp file.
    let orphan_tmp = storage
        .path()
        .join(format!("{}.{}.tmp", Uuid::now_v7(), Uuid::now_v7()));
    std::fs::write(&orphan_tmp, b"abandoned partial").expect("orphan temp");

    let database_url = common::database_url(&db.name);

    // Listing is read-only and reports exactly the stuck + failed rows.
    let stdout = run_cli(&database_url, storage.path(), &["podcasts", "stuck"]);
    assert!(stdout.contains("Stuck Episode"), "stdout: {stdout}");
    assert!(stdout.contains("Failed Episode"), "stdout: {stdout}");
    assert!(!stdout.contains("Live Episode"), "stdout: {stdout}");
    assert!(!stdout.contains("Downloaded Episode"), "stdout: {stdout}");
    assert!(
        stdout.contains("-- 2 stuck download(s)"),
        "stdout: {stdout}"
    );

    // Recovery without --retry-failed requeues only the expired lease and
    // reconciles only unreferenced files.
    let stdout = run_cli(&database_url, storage.path(), &["podcasts", "recover"]);
    assert!(
        stdout.contains("recovered 1 stuck download(s)"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("orphan reconciliation: removed 2 file(s)"),
        "stdout: {stdout}"
    );

    let stuck_state: (String, i32, Option<Uuid>, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT download_status,download_attempt_count,download_lease_owner,download_lease_expires_at \
         FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(stuck_id)
    .fetch_one(&db.pool)
    .await
    .expect("stuck state");
    assert_eq!(stuck_state.0, "Queued");
    assert_eq!(stuck_state.1, 2, "attempt history is preserved");
    assert_eq!(stuck_state.2, None);
    assert_eq!(stuck_state.3, None);

    let live_state: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT download_status,download_lease_owner FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(live_id)
    .fetch_one(&db.pool)
    .await
    .expect("live state");
    assert_eq!(
        live_state,
        ("Downloading".to_owned(), Some(live_worker)),
        "a live worker's lease is never stolen"
    );

    let failed_state: (String, i32) = sqlx::query_as(
        "SELECT download_status,download_attempt_count FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(failed_id)
    .fetch_one(&db.pool)
    .await
    .expect("failed state");
    assert_eq!(failed_state, ("Failed".to_owned(), 5));

    assert!(!stuck_tmp.exists(), "expired-lease temp is an orphan");
    assert!(!orphan_tmp.exists(), "unreferenced temp is removed");
    assert!(live_tmp.exists(), "live in-flight temp survives");
    assert!(media_path.exists(), "referenced media survives");

    // With --retry-failed the failed row gets a fresh bounded retry budget.
    let stdout = run_cli(
        &database_url,
        storage.path(),
        &["podcasts", "recover", "--retry-failed"],
    );
    assert!(
        stdout.contains("recovered 1 stuck download(s)"),
        "stdout: {stdout}"
    );
    let retried: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT download_status,download_attempt_count,download_error \
         FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(failed_id)
    .fetch_one(&db.pool)
    .await
    .expect("retried state");
    assert_eq!(retried, ("Queued".to_owned(), 0, None));

    db.drop().await;
}
