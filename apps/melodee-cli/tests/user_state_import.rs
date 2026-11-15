//! End-to-end portable user-state import (ADR-0200).
//!
//! Spawns the real CLI binary against a freshly migrated database, seeds a
//! catalog and a user, imports an export directory, and then proves the
//! re-run contract: newer rows are added, older rows cannot downgrade local
//! state, local playlist edits survive, and a dry run writes nothing.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkerId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

const STREAMS: [&str; 11] = [
    "users.ndjson",
    "entity-states.ndjson",
    "track-plays.ndjson",
    "playlists.ndjson",
    "playlist-items.ndjson",
    "smart-playlists.ndjson",
    "bookmarks.ndjson",
    "groups.ndjson",
    "group-members.ndjson",
    "playback-settings.ndjson",
    "equalizer-presets.ndjson",
];

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct ExportBuilder {
    streams: BTreeMap<&'static str, Vec<Value>>,
}

impl ExportBuilder {
    fn new() -> Self {
        Self {
            streams: STREAMS
                .iter()
                .map(|name| (*name, Vec::new()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn push(mut self, stream: &'static str, value: Value) -> Self {
        self.streams.entry(stream).or_default().push(value);
        self
    }

    fn write(&self, directory: &Path) {
        let mut counts = BTreeMap::new();
        let mut checksums = BTreeMap::new();
        for (name, rows) in &self.streams {
            let mut bytes = Vec::new();
            for row in rows {
                bytes.extend_from_slice(row.to_string().as_bytes());
                bytes.push(b'\n');
            }
            std::fs::write(directory.join(name), &bytes).expect("stream write");
            counts.insert(name.to_string(), json!(rows.len()));
            checksums.insert(name.to_string(), json!(sha256_hex(&bytes)));
        }
        let manifest = json!({
            "schemaVersion": 1,
            "generator": { "name": "integration-test", "version": "1.0.0" },
            "generatedAt": "2024-06-01T00:00:00Z",
            "counts": counts,
            "checksums": checksums,
            "referenceOnly": {
                "Shares": { "count": 2, "reason": "bearer secrets are not portable" }
            }
        });
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("manifest write");
    }
}

async fn seed_catalog(db: &TestDb, ordinal: u32, title: &str, artist: &str) -> (Uuid, Uuid) {
    let store = CatalogStore;
    let fixture = seed_published_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: artist,
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let worker = WorkerId::new();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
        .await
        .expect("claim catalog fixture");
    assert_eq!(claimed.len(), 1);
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("user-state-import-v1", "catalog-test"),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("import catalog fixture");
    let release_public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("release public id");
    let track_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1 LIMIT 1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track public id");
    (release_public, track_public)
}

async fn seed_user(db: &TestDb, public_id: Uuid, username: &str) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users (
            id, public_api_id, username, normalized_username, email, normalized_email,
            password_hash, password_hash_algorithm, stream_signing_key,
            is_admin, is_editor, enabled, locked, blacklisted, auth_revision,
            created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,'bcrypt',$8,false,false,true,false,false,1,$9,$9)",
    )
    .bind(id)
    .bind(public_id)
    .bind(username)
    .bind(username.to_uppercase())
    .bind(format!("{username}@example.test"))
    .bind(format!("{}@EXAMPLE.TEST", username.to_uppercase()))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("user-state-import-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

fn run_import(database_url: &str, directory: &Path, extra: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(["users", "import-state", "--input"])
        .arg(directory)
        .args(extra)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("spawn melodee users import-state");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn track_state(user: Uuid, track: Uuid, starred: bool, play_count: i64, updated: &str) -> Value {
    json!({
        "userPublicId": user,
        "entity": "track",
        "entityPublicId": track,
        "starred": starred,
        "hated": false,
        "rating": 5,
        "pinned": false,
        "playCount": play_count,
        "lastPlayedAt": null,
        "createdAt": "2024-01-01T00:00:00Z",
        "updatedAt": updated,
    })
}

fn export_for_user(user: Uuid) -> ExportBuilder {
    ExportBuilder::new().push(
        "users.ndjson",
        json!({
            "publicId": user,
            "username": "listener",
            "email": "listener@example.test",
            "createdAt": "2024-01-01T00:00:00Z",
            "updatedAt": "2024-02-01T00:00:00Z",
        }),
    )
}

#[tokio::test]
async fn partial_catalog_import_retries_the_same_export_when_more_music_arrives() {
    let db = TestDb::create("user_state_growing").await;
    let database_url = common::database_url(&db.name);
    let user_public = Uuid::now_v7();
    let (_, available) = seed_catalog(&db, 720, "Available", "Import Artist").await;
    let arriving_later = Uuid::now_v7();
    let (inactive_release, inactive) = seed_catalog(&db, 721, "Inactive", "Import Artist").await;
    sqlx::query("UPDATE releases SET status='deprecated' WHERE public_api_id=$1")
        .bind(inactive_release)
        .execute(&db.pool)
        .await
        .expect("deactivate release");

    let playlist = Uuid::now_v7();
    let mut export = export_for_user(user_public).push(
        "playlists.ndjson",
        json!({
            "publicId": playlist,
            "ownerUserPublicId": user_public,
            "name": "Imported",
            "isPublic": false,
            "createdAt": "2024-01-01T00:00:00Z",
            "updatedAt": "2024-02-01T00:00:00Z",
        }),
    );
    for (position, track) in [available, arriving_later, inactive]
        .into_iter()
        .enumerate()
    {
        let mut state = track_state(user_public, track, position == 0, 7, "2024-02-01T00:00:00Z");
        state["hated"] = json!(position != 0);
        state["lastPlayedAt"] = json!("2024-02-01T12:00:00Z");
        export = export
            .push("entity-states.ndjson", state)
            .push(
                "track-plays.ndjson",
                json!({
                    "userPublicId": user_public,
                    "trackPublicId": track,
                    "playedAt": "2024-02-01T12:00:00Z",
                    "durationSeconds": 180,
                    "completed": true,
                }),
            )
            .push(
                "playlist-items.ndjson",
                json!({ "playlistPublicId": playlist, "trackPublicId": track, "position": position }),
            )
            .push(
                "bookmarks.ndjson",
                json!({
                    "userPublicId": user_public,
                    "trackPublicId": track,
                    "position": 1000,
                    "createdAt": "2024-01-01T00:00:00Z",
                    "updatedAt": "2024-02-01T00:00:00Z",
                }),
            );
    }
    let directory = tempfile::tempdir().expect("export directory");
    export.write(directory.path());

    let (ok, stdout, stderr) =
        run_import(&database_url, directory.path(), &["--dry-run", "--json"]);
    assert!(ok, "partial catalog dry run failed: {stderr}");
    let plan: Value = serde_json::from_str(&stdout).expect("plan json");
    assert_eq!(plan["usersPlannedCreated"], 1);
    assert_eq!(plan["statesCreated"], 1, "only matched states are planned");
    assert_eq!(plan["playsInserted"], 1);
    assert_eq!(plan["playlistItemsAdded"], 1);
    assert_eq!(plan["bookmarksWritten"], 1);
    for key in [
        "statesUnmatched",
        "playsUnmatched",
        "playlistItemsUnmatched",
        "bookmarksUnmatched",
    ] {
        assert_eq!(plan[key], 2, "{key}");
    }
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM native_users WHERE public_api_id=$1")
        .bind(user_public)
        .fetch_one(&db.pool)
        .await
        .expect("users after dry run");
    assert_eq!(users, 0);

    let (ok, stdout, stderr) = run_import(&database_url, directory.path(), &[]);
    assert!(ok, "partial catalog import failed: {stderr}");
    assert!(stdout.contains("2 unmatched"), "{stdout}");
    assert!(stdout.contains("Re-run the same export"), "{stdout}");
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM native_users WHERE public_api_id=$1")
        .bind(user_public)
        .fetch_one(&db.pool)
        .await
        .expect("created user");
    let state: (bool, bool, i64) = sqlx::query_as(
        "SELECT starred, hated, play_count FROM native_track_user_states WHERE user_id=$1 AND track_id=$2",
    )
    .bind(user_id)
    .bind(available)
    .fetch_one(&db.pool)
    .await
    .expect("matched state");
    assert_eq!(state, (true, false, 7));

    // A newer local dislike survives replaying the older export.
    sqlx::query(
        "UPDATE native_track_user_states SET starred=false, hated=true, play_count=10,
         updated_at='2024-03-01T00:00:00Z' WHERE user_id=$1 AND track_id=$2",
    )
    .bind(user_id)
    .bind(available)
    .execute(&db.pool)
    .await
    .expect("local edit");

    let (ok, stdout, stderr) = run_import(
        &database_url,
        directory.path(),
        &["--allow-unmatched", "--json"],
    );
    assert!(ok, "identical partial import failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["statesCreated"], 0);
    assert_eq!(summary["statesUnchanged"], 1);
    assert_eq!(summary["playsInserted"], 0);
    assert_eq!(summary["playsDuplicate"], 1);
    assert_eq!(summary["playlistItemsAdded"], 0);

    // Simulate newly imported music: preserve its exported public id at INSERT,
    // with playable media from the published fixture. Public ids are immutable.
    let added_track = Uuid::now_v7();
    let mut tx = db.pool.begin().await.expect("new catalog transaction");
    sqlx::query(
        "INSERT INTO tracks (
            id, public_api_id, release_id, audio_identity_id, playable_media_asset_id,
            ordinal, title, normalized_title, sort_title, created_at, updated_at
         ) SELECT $1, $2, release_id, audio_identity_id, playable_media_asset_id,
                  ordinal + 1, 'Added later', 'added later', 'Added later', created_at, updated_at
           FROM tracks WHERE public_api_id=$3",
    )
    .bind(added_track)
    .bind(arriving_later)
    .bind(available)
    .execute(&mut *tx)
    .await
    .expect("preserve imported public id");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id, media_asset_id, purpose, created_at)
         SELECT id, playable_media_asset_id, 'playable', created_at FROM tracks WHERE id=$1",
    )
    .bind(added_track)
    .execute(&mut *tx)
    .await
    .expect("link playable media");
    tx.commit().await.expect("new catalog commit");
    sqlx::query("UPDATE releases SET status='active' WHERE public_api_id=$1")
        .bind(inactive_release)
        .execute(&db.pool)
        .await
        .expect("activate release");

    let (ok, stdout, stderr) = run_import(
        &database_url,
        directory.path(),
        &["--require-all-matches", "--json"],
    );
    assert!(ok, "import after catalog growth failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["statesCreated"], 2);
    assert_eq!(summary["statesUnchanged"], 1);
    assert_eq!(summary["playsInserted"], 2);
    assert_eq!(summary["playsDuplicate"], 1);
    assert_eq!(summary["playlistItemsAdded"], 2);
    assert_eq!(summary["playlistItemsDuplicate"], 1);
    for key in [
        "statesUnmatched",
        "playsUnmatched",
        "playlistItemsUnmatched",
        "bookmarksUnmatched",
    ] {
        assert_eq!(summary[key], 0, "{key}");
    }
    let states: Vec<(Uuid, bool, bool, i64)> = sqlx::query_as(
        "SELECT track_id, starred, hated, play_count FROM native_track_user_states WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_all(&db.pool)
    .await
    .expect("all matched states");
    assert_eq!(states.len(), 3);
    assert!(states.contains(&(available, false, true, 10)));
    assert!(states.contains(&(added_track, false, true, 7)));
    assert!(states.contains(&(inactive, false, true, 7)));
    let plays: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("play history total");
    assert_eq!(plays, 3);
    let bookmarks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM opensubsonic_bookmarks WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("bookmark total");
    assert_eq!(bookmarks, 3);
    db.drop().await;
}

#[tokio::test]
async fn reruns_preserve_local_flags_and_merge_play_maxima_independently_of_updated_at() {
    let db = TestDb::create("user_state_maxima").await;
    let database_url = common::database_url(&db.name);
    let user = Uuid::now_v7();
    let user_id = seed_user(&db, user, "listener").await;
    let (_, track) = seed_catalog(&db, 723, "Maxima", "Import Artist").await;
    let directory = tempfile::tempdir().expect("export directory");
    let mut state = track_state(user, track, true, 7, "2024-02-01T00:00:00Z");
    state["lastPlayedAt"] = json!("2024-02-01T12:00:00Z");
    export_for_user(user)
        .push("entity-states.ndjson", state)
        .write(directory.path());
    let (ok, _, stderr) = run_import(&database_url, directory.path(), &[]);
    assert!(ok, "initial import failed: {stderr}");

    for (local_updated_at, local_play_count) in [
        ("2024-02-01T00:00:00Z", 3_i64),
        ("2024-03-01T00:00:00Z", 3),
        ("2024-02-01T00:00:00Z", 7),
        ("2024-03-01T00:00:00Z", 7),
    ] {
        sqlx::query(
            "UPDATE native_track_user_states SET starred=false, hated=true, rating=1, pinned=true,
             play_count=$4, last_played_at=NULL, updated_at=$3::text::timestamptz
             WHERE user_id=$1 AND track_id=$2",
        )
        .bind(user_id)
        .bind(track)
        .bind(local_updated_at)
        .bind(local_play_count)
        .execute(&db.pool)
        .await
        .expect("local flag edit");
        let (ok, stdout, stderr) = run_import(&database_url, directory.path(), &["--json"]);
        assert!(ok, "merge failed: {stderr}");
        let summary: Value = serde_json::from_str(&stdout).expect("summary json");
        assert_eq!(summary["statesUpdated"], 1);
        let state: (bool, bool, i16, bool, i64, String, String) = sqlx::query_as(
            "SELECT starred, hated, rating, pinned, play_count,
             to_char(last_played_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),
             to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')
             FROM native_track_user_states WHERE user_id=$1 AND track_id=$2",
        )
        .bind(user_id)
        .bind(track)
        .fetch_one(&db.pool)
        .await
        .expect("merged state");
        assert_eq!(
            state,
            (
                false,
                true,
                1,
                true,
                7,
                "2024-02-01T12:00:00Z".into(),
                local_updated_at.into()
            )
        );

        let (ok, stdout, stderr) = run_import(&database_url, directory.path(), &["--json"]);
        assert!(ok, "repeat merge failed: {stderr}");
        let summary: Value = serde_json::from_str(&stdout).expect("summary json");
        assert_eq!(summary["statesUpdated"], 0);
        assert_eq!(summary["statesUnchanged"], 1);
    }
    db.drop().await;
}

#[tokio::test]
async fn strict_catalog_matching_checks_every_stream_before_creating_accounts() {
    let db = TestDb::create("user_state_strict").await;
    let database_url = common::database_url(&db.name);
    let user = Uuid::now_v7();
    let missing = Uuid::now_v7();
    for stream in [
        "entity-states.ndjson",
        "track-plays.ndjson",
        "playlist-items.ndjson",
        "bookmarks.ndjson",
    ] {
        let mut export = export_for_user(user);
        let row = match stream {
            "entity-states.ndjson" => track_state(user, missing, true, 2, "2024-02-01T00:00:00Z"),
            "track-plays.ndjson" => json!({
                "userPublicId": user, "trackPublicId": missing, "playedAt": "2024-02-01T00:00:00Z",
            }),
            "playlist-items.ndjson" => {
                let playlist = Uuid::now_v7();
                export = export.push("playlists.ndjson", json!({
                    "publicId": playlist, "ownerUserPublicId": user, "name": "Missing track",
                    "isPublic": false, "createdAt": "2024-01-01T00:00:00Z", "updatedAt": "2024-02-01T00:00:00Z",
                }));
                json!({ "playlistPublicId": playlist, "trackPublicId": missing, "position": 0 })
            }
            _ => json!({
                "userPublicId": user, "trackPublicId": missing, "position": 1,
                "createdAt": "2024-01-01T00:00:00Z", "updatedAt": "2024-02-01T00:00:00Z",
            }),
        };
        let directory = tempfile::tempdir().expect("strict export directory");
        export.push(stream, row).write(directory.path());
        for extra in [
            vec!["--require-all-matches"],
            vec!["--require-all-matches", "--dry-run"],
        ] {
            let (ok, _, stderr) = run_import(&database_url, directory.path(), &extra);
            assert!(!ok, "strict matching accepted {stream}");
            assert!(stderr.contains("missing or not live"), "{stream}: {stderr}");
        }
        let users: i64 =
            sqlx::query_scalar("SELECT count(*) FROM native_users WHERE public_api_id=$1")
                .bind(user)
                .fetch_one(&db.pool)
                .await
                .expect("users after rejected strict import");
        assert_eq!(users, 0, "strict rejection must precede writes: {stream}");
    }
    db.drop().await;
}

#[tokio::test]
async fn import_is_additive_idempotent_and_re_runnable() {
    let db = TestDb::create("user_state_import").await;
    let database_url = common::database_url(&db.name);
    let user_public = Uuid::now_v7();
    let user_id = seed_user(&db, user_public, "listener").await;
    let (_release_public, first_track) =
        seed_catalog(&db, 700, "Import Record", "Import Artist").await;
    let (_second_release, second_track) =
        seed_catalog(&db, 701, "Import Two", "Import Artist").await;

    let first_dir = tempfile::tempdir().expect("export dir");
    ExportBuilder::new()
        .push(
            "users.ndjson",
            json!({
                "publicId": user_public,
                "username": "listener",
                "email": "listener@example.test",
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "entity-states.ndjson",
            track_state(user_public, first_track, true, 2, "2024-02-01T00:00:00Z"),
        )
        .push(
            "track-plays.ndjson",
            json!({
                "userPublicId": user_public,
                "trackPublicId": first_track,
                "playedAt": "2024-02-01T12:00:00Z",
                "durationSeconds": 180,
                "completed": true,
            }),
        )
        .push(
            "playlists.ndjson",
            json!({
                "publicId": "018f0000-0000-7000-8000-00000000aa01",
                "ownerUserPublicId": user_public,
                "name": "Liked",
                "isPublic": false,
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "playlist-items.ndjson",
            json!({
                "playlistPublicId": "018f0000-0000-7000-8000-00000000aa01",
                "position": 0,
                "trackPublicId": first_track,
            }),
        )
        .write(first_dir.path());

    let (ok, stdout, stderr) = run_import(&database_url, first_dir.path(), &["--json"]);
    assert!(ok, "import failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["statesCreated"], 1);
    assert_eq!(summary["playsInserted"], 1);
    assert_eq!(summary["playlistsCreated"], 1);
    assert_eq!(summary["playlistItemsAdded"], 1);
    assert_eq!(summary["referenceOnly"]["Shares"], 2);

    let state: (bool, i64, i16) = sqlx::query_as(
        "SELECT starred, play_count, rating FROM native_track_user_states
          WHERE user_id=$1 AND track_id=$2",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&first_track.to_string()).expect("uuid"))
    .fetch_one(&db.pool)
    .await
    .expect("state row");
    assert_eq!(state, (true, 2, 5));

    // A local edit after the first import must survive the next run.
    let playlist_id: Uuid =
        sqlx::query_scalar("SELECT id FROM native_playlists WHERE public_api_id=$1")
            .bind(Uuid::parse_str("018f0000-0000-7000-8000-00000000aa01").expect("uuid"))
            .fetch_one(&db.pool)
            .await
            .expect("playlist id");

    // Re-run the *same* export: nothing duplicates.
    let (ok, stdout, stderr) = run_import(&database_url, first_dir.path(), &["--json"]);
    assert!(ok, "second import failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["statesCreated"], 0);
    assert_eq!(summary["statesUnchanged"], 1);
    assert_eq!(summary["playsInserted"], 0);
    assert_eq!(summary["playsDuplicate"], 1);
    assert_eq!(summary["playlistItemsAdded"], 0);
    assert_eq!(summary["playlistItemsDuplicate"], 1);

    // Refresh the export (as an operator would after finding new likes):
    // a newer state for the first track, a brand-new track like, one new
    // play, and one new playlist item.
    let second_dir = tempfile::tempdir().expect("second export dir");
    ExportBuilder::new()
        .push(
            "users.ndjson",
            json!({
                "publicId": user_public,
                "username": "listener",
                "email": "listener@example.test",
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "entity-states.ndjson",
            track_state(user_public, first_track, true, 5, "2024-03-01T00:00:00Z"),
        )
        .push(
            "entity-states.ndjson",
            track_state(user_public, second_track, true, 1, "2024-03-01T00:00:00Z"),
        )
        .push(
            "track-plays.ndjson",
            json!({
                "userPublicId": user_public,
                "trackPublicId": first_track,
                "playedAt": "2024-02-01T12:00:00Z",
                "durationSeconds": 180,
                "completed": true,
            }),
        )
        .push(
            "track-plays.ndjson",
            json!({
                "userPublicId": user_public,
                "trackPublicId": second_track,
                "playedAt": "2024-03-01T12:00:00Z",
                "durationSeconds": 200,
                "completed": true,
            }),
        )
        .push(
            "playlists.ndjson",
            json!({
                "publicId": "018f0000-0000-7000-8000-00000000aa01",
                "ownerUserPublicId": user_public,
                "name": "Liked",
                "isPublic": false,
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-03-01T00:00:00Z",
            }),
        )
        .push(
            "playlist-items.ndjson",
            json!({
                "playlistPublicId": "018f0000-0000-7000-8000-00000000aa01",
                "position": 0,
                "trackPublicId": first_track,
            }),
        )
        .push(
            "playlist-items.ndjson",
            json!({
                "playlistPublicId": "018f0000-0000-7000-8000-00000000aa01",
                "position": 1,
                "trackPublicId": second_track,
            }),
        )
        .write(second_dir.path());

    let (ok, stdout, stderr) = run_import(&database_url, second_dir.path(), &["--json"]);
    assert!(ok, "refresh import failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["statesCreated"], 1);
    assert_eq!(summary["statesUpdated"], 1);
    assert_eq!(summary["playsInserted"], 1);
    assert_eq!(summary["playlistItemsAdded"], 1);
    assert_eq!(summary["playlistItemsDuplicate"], 1);

    let play_count: i64 = sqlx::query_scalar(
        "SELECT play_count FROM native_track_user_states WHERE user_id=$1 AND track_id=$2",
    )
    .bind(user_id)
    .bind(first_track)
    .fetch_one(&db.pool)
    .await
    .expect("play count");
    assert_eq!(play_count, 5, "play counts are monotonic maxima");
    let items: Vec<(i32, Uuid)> = sqlx::query_as(
        "SELECT position, track_id FROM native_playlist_items WHERE playlist_id=$1 ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(&db.pool)
    .await
    .expect("playlist items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].0, 0);
    assert_eq!(items[1].0, 1);

    // Dry run of a further refresh writes nothing.
    let third_dir = tempfile::tempdir().expect("third export dir");
    ExportBuilder::new()
        .push(
            "users.ndjson",
            json!({
                "publicId": user_public,
                "username": "listener",
                "email": "listener@example.test",
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "entity-states.ndjson",
            track_state(user_public, first_track, true, 9, "2024-04-01T00:00:00Z"),
        )
        .write(third_dir.path());
    let (ok, stdout, stderr) =
        run_import(&database_url, third_dir.path(), &["--dry-run", "--json"]);
    assert!(ok, "dry run failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["dryRun"], true);
    let untouched: i64 = sqlx::query_scalar(
        "SELECT play_count FROM native_track_user_states WHERE user_id=$1 AND track_id=$2",
    )
    .bind(user_id)
    .bind(first_track)
    .fetch_one(&db.pool)
    .await
    .expect("play count after dry run");
    assert_eq!(untouched, 5);

    db.drop().await;
}

#[tokio::test]
async fn default_import_creates_missing_accounts_unprivileged_and_idempotently() {
    let db = TestDb::create("user_state_import_create_users").await;
    let database_url = common::database_url(&db.name);
    let user_public = Uuid::now_v7();
    let (_release_public, track_public) =
        seed_catalog(&db, 710, "Create Record", "Create Artist").await;
    let exported_hash = "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS";

    let export_dir = tempfile::tempdir().expect("export dir");
    ExportBuilder::new()
        .push(
            "users.ndjson",
            json!({
                "publicId": user_public,
                "username": "newcomer",
                "email": "newcomer@example.test",
                "isAdmin": true,
                "isEditor": true,
                "credential": { "algorithm": "bcrypt", "hash": exported_hash },
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "entity-states.ndjson",
            track_state(user_public, track_public, true, 1, "2024-02-01T00:00:00Z"),
        )
        .write(export_dir.path());

    // A dry run reports the planned creation and writes nothing.
    let (ok, stdout, stderr) =
        run_import(&database_url, export_dir.path(), &["--dry-run", "--json"]);
    assert!(ok, "dry run failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["usersPlannedCreated"], 1);
    let created_by_dry_run: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_users WHERE public_api_id=$1")
            .bind(user_public)
            .fetch_one(&db.pool)
            .await
            .expect("count users after dry run");
    assert_eq!(created_by_dry_run, 0, "dry run must not create accounts");

    // Creating missing accounts is the default: importing user data must work
    // without extra flags.
    let (ok, stdout, stderr) = run_import(&database_url, export_dir.path(), &["--json"]);
    assert!(ok, "default import failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["usersCreated"], 1);
    assert_eq!(summary["statesCreated"], 1);

    let (is_admin, is_editor, enabled, password_hash): (bool, bool, bool, String) = sqlx::query_as(
        "SELECT is_admin, is_editor, enabled, password_hash FROM native_users
              WHERE public_api_id=$1",
    )
    .bind(user_public)
    .fetch_one(&db.pool)
    .await
    .expect("created user row");
    assert!(!is_admin, "created accounts are unprivileged by default");
    assert!(!is_editor, "created accounts are unprivileged by default");
    assert!(enabled);
    assert_eq!(password_hash, exported_hash);

    // Re-running the same export is idempotent and keeps the account.
    let (ok, stdout, stderr) = run_import(&database_url, export_dir.path(), &["--json"]);
    assert!(ok, "re-run failed: {stderr}");
    let summary: Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["usersCreated"], 0);
    assert_eq!(summary["usersUnchanged"], 1);
    assert_eq!(summary["statesUnchanged"], 1);

    // Opting out still fails closed with an actionable message.
    let unknown = Uuid::now_v7();
    let opt_out_dir = tempfile::tempdir().expect("opt-out export");
    ExportBuilder::new()
        .push(
            "users.ndjson",
            json!({
                "publicId": unknown,
                "username": "unknown",
                "email": "unknown@example.test",
                "createdAt": "2024-01-01T00:00:00Z",
                "updatedAt": "2024-02-01T00:00:00Z",
            }),
        )
        .push(
            "entity-states.ndjson",
            track_state(unknown, track_public, true, 2, "2024-02-01T00:00:00Z"),
        )
        .write(opt_out_dir.path());
    let (ok, _stdout, stderr) =
        run_import(&database_url, opt_out_dir.path(), &["--no-create-users"]);
    assert!(!ok, "opt-out must not create accounts");
    assert!(
        stderr.contains("--no-create-users"),
        "error should name the switch: {stderr}"
    );
    for extra in [
        vec![
            "--no-create-users",
            "--allow-unmatched",
            "--dry-run",
            "--json",
        ],
        vec!["--no-create-users", "--allow-unmatched", "--json"],
    ] {
        let (ok, stdout, stderr) = run_import(&database_url, opt_out_dir.path(), &extra);
        assert!(ok, "skipping missing users failed: {stderr}");
        let summary: Value = serde_json::from_str(&stdout).expect("summary json");
        assert_eq!(
            summary["usersUnmatched"].as_array().expect("users").len(),
            1
        );
        assert_eq!(summary["statesUnmatched"], 1);
        assert_eq!(summary["statesCreated"], 0);
        assert_eq!(summary["usersCreated"], 0);
    }

    db.drop().await;
}
