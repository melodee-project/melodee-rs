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
    assert_eq!(plan["usersAlreadyOnServer"], 0);
    assert_eq!(plan["exportedPreferences"]["favoriteTracks"], 1);
    assert_eq!(plan["exportedPreferences"]["dislikes"], 2);
    assert_eq!(plan["exportedPreferences"]["ratings"], 3);
    assert_eq!(plan["matchedPreferences"]["favoriteTracks"], 1);
    assert_eq!(plan["matchedPreferences"]["dislikes"], 0);
    assert_eq!(plan["matchedPreferences"]["ratings"], 1);
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

    let (ok, stdout, stderr) = run_import(&database_url, directory.path(), &["--dry-run"]);
    assert!(ok, "text preview failed: {stderr}");
    assert!(
        stdout.contains("0 matched to existing accounts, 1 planned for creation"),
        "{stdout}"
    );
    let favorite_total = stdout
        .lines()
        .find(|line| line.contains("Favorites (likes)"))
        .expect("favorite total");
    assert_eq!(
        favorite_total.split_whitespace().collect::<Vec<_>>(),
        ["Favorites", "(likes)", "1", "1", "0"]
    );
    assert!(stdout.contains("Listening history"), "{stdout}");

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
    assert_eq!(summary["usersAlreadyOnServer"], 1);
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
        assert_eq!(summary["exportedPreferences"]["favoriteTracks"], 1);
        assert_eq!(summary["matchedPreferences"]["favoriteTracks"], 0);
        assert_eq!(summary["statesCreated"], 0);
        assert_eq!(summary["usersCreated"], 0);
    }

    db.drop().await;
}

#[tokio::test]
async fn rebuilt_catalog_remembers_links_as_music_arrives_and_protects_newer_choices() {
    let db = TestDb::create("user_state_rebuilt").await;
    let database_url = common::database_url(&db.name);
    let old_user = Uuid::now_v7();
    let current_user = Uuid::now_v7();
    let user_id = seed_user(&db, current_user, "listener").await;
    let old_first = Uuid::now_v7();
    let old_second = Uuid::now_v7();
    let (release, first) = seed_catalog(&db, 815, "First", "Import Artist").await;
    let artist: Uuid = sqlx::query_scalar("SELECT a.public_api_id FROM artists a JOIN release_artists ra ON ra.artist_id=a.id JOIN releases r ON r.id=ra.release_id WHERE r.public_api_id=$1 LIMIT 1")
        .bind(release).fetch_one(&db.pool).await.expect("published artist");
    let old_release = Uuid::now_v7();
    let old_artist = Uuid::now_v7();
    let dir = tempfile::tempdir().expect("private export");
    let mut snapshot = ExportBuilder::new().push("users.ndjson", json!({
        "publicId":old_user,"username":"listener","email":"former@example.test",
        "enabled":false,"isAdmin":true,"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2099-01-01T00:00:00Z"
    }));
    for (kind, id) in [("artist", old_artist), ("release", old_release)] {
        snapshot = snapshot.push("entity-states.ndjson", json!({
            "userPublicId":old_user,"entity":kind,"entityPublicId":id,"starred":true,"pinned":true,
            "createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"
        }));
    }
    for track in [old_first, old_second] {
        snapshot = snapshot
            .push(
                "entity-states.ndjson",
                track_state(old_user, track, true, 2, "2024-03-01T00:00:00Z"),
            )
            .push(
                "track-plays.ndjson",
                json!({"userPublicId":old_user,"trackPublicId":track,
                "playedAt":"2024-02-15T00:00:00Z","durationSeconds":180,"completed":true}),
            );
    }
    snapshot.write(dir.path());
    let (ok, stdout, err) = run_import(&database_url, dir.path(), &["--dry-run", "--json"]);
    assert!(ok, "{err}");
    let plan: Value = serde_json::from_str(&stdout).expect("conflict preview");
    assert_eq!(plan["usersRequiringReview"], 1);
    assert_eq!(plan["usersPlannedCreated"], 0);
    let (ok, _, err) = run_import(&database_url, dir.path(), &[]);
    assert!(!ok && err.contains("--identity-map"), "{err}");
    let installation = melodee_persistence::installation_id(&db.pool)
        .await
        .expect("target identity");
    let path = dir.path().join("links.json");
    let args = ["--identity-map", path.to_str().expect("map path"), "--json"];
    let link =
        |kind, legacy: Uuid, target: Uuid| json!({"kind":kind,"legacyId":legacy,"targetId":target});
    let save = |installation: Uuid, links: &[Value]| {
        std::fs::write(
            &path,
            json!({"schemaVersion":1,"targetInstallationId":installation,
            "approved":links})
            .to_string(),
        )
        .expect("operator map");
    };
    save(installation, &[]);
    let (ok, stdout, err) = run_import(
        &database_url,
        dir.path(),
        &[
            "--identity-map",
            path.to_str().expect("map path"),
            "--dry-run",
        ],
    );
    assert!(ok, "{err}");
    assert!(stdout.contains(&format!(
        "python3 scripts/reconcile_legacy_user_state.py --input '{}' --identity-map '{}' --review",
        dir.path().display(),
        path.display()
    )));
    assert!(stdout.contains("This export supports account review only"));
    assert!(stdout.contains("press Enter to skip"));
    let mut links = vec![
        link("user", old_user, current_user),
        link("artist", old_artist, artist),
        link("release", old_release, release),
        link("track", old_first, first),
    ];
    save(Uuid::now_v7(), &links);
    let (ok, _, err) = run_import(&database_url, dir.path(), &args);
    assert!(
        !ok && err.contains("different target installation"),
        "{err}"
    );
    links.push(link("track", old_second, first));
    save(installation, &links);
    let (ok, _, err) = run_import(&database_url, dir.path(), &args);
    assert!(!ok && err.contains("one-to-one"), "{err}");
    links.pop();
    save(installation, &links);
    let apply = || {
        let (ok, stdout, stderr) = run_import(&database_url, dir.path(), &args);
        assert!(ok, "{stderr}");
        serde_json::from_str::<Value>(&stdout).expect("summary")
    };
    let first_run = apply();
    assert_eq!(first_run["usersLinked"], 1);
    assert_eq!(first_run["usersCreated"], 0);
    assert_eq!(first_run["statesUnmatched"], 1);
    assert_eq!(first_run["statesCreated"], 3);
    assert_eq!(
        first_run["preferencesByUser"][old_user.to_string()]["matched"]["pins"],
        2
    );
    let profile: (bool, bool) =
        sqlx::query_as("SELECT enabled, is_admin FROM native_users WHERE id=$1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("linked profile");
    assert_eq!(profile, (true, false));
    sqlx::query("UPDATE native_track_user_states SET starred=false, hated=true, rating=2, updated_at=now() WHERE user_id=$1")
        .bind(user_id).execute(&db.pool).await.expect("listener changes their mind");
    let (_, second) = seed_catalog(&db, 816, "Second", "Import Artist").await;
    links.push(link("track", old_second, second));
    save(installation, &links);
    let second_run = apply();
    assert_eq!(second_run["statesUnmatched"], 0);
    assert_eq!(second_run["statesCreated"], 1);
    assert_eq!(second_run["playsInserted"], 1);
    assert_eq!(second_run["playsDuplicate"], 1);
    let third_run = apply();
    assert_eq!(third_run["statesCreated"], 0);
    assert_eq!(third_run["statesUpdated"], 0);
    assert_eq!(third_run["supersededPreferences"]["favoriteTracks"], 1);
    assert_eq!(third_run["supersededPreferences"]["ratings"], 1);
    assert_eq!(third_run["statesUnchanged"], 4);
    assert_eq!(third_run["playsInserted"], 0);
    let flags: Vec<(Uuid,bool,bool,i16)> = sqlx::query_as("SELECT t.public_api_id,s.starred,s.hated,s.rating FROM native_track_user_states s JOIN tracks t ON t.id=s.track_id WHERE user_id=$1")
        .bind(user_id).fetch_all(&db.pool).await.expect("actual preferences");
    assert!(flags.contains(&(first, false, true, 2)));
    assert!(flags.contains(&(second, true, false, 5)));
    db.drop().await;
}

#[tokio::test]
async fn unique_email_links_survive_a_renamed_login_and_reassigned_email() {
    let db = TestDb::create("portable_email").await;
    let url = common::database_url(&db.name);
    let old = Uuid::now_v7();
    let current = Uuid::now_v7();
    let owner = seed_user(&db, current, "listener").await;
    let (_, track) = seed_catalog(&db, 830, "Mapped by email", "Artist").await;
    let dir = tempfile::tempdir().expect("export");
    let mut snapshot = ExportBuilder::new().push("users.ndjson", json!({
        "publicId": old,"username":"old-name","email":" LISTENER@EXAMPLE.TEST ",
        "enabled":false,"isAdmin":true,"createdAt":"2024-01-01T00:00:00Z","updatedAt":"2099-01-01T00:00:00Z"
    })).push("entity-states.ndjson",track_state(old,track,true,1,"2024-02-01T00:00:00Z"));
    snapshot.write(dir.path());
    let run = |flags: &[&str]| {
        let (ok, out, err) = run_import(&url, dir.path(), flags);
        assert!(ok, "{err}");
        serde_json::from_str::<Value>(&out).expect("JSON summary")
    };
    let installation = melodee_persistence::installation_id(&db.pool)
        .await
        .expect("installation");
    let receipt = dir
        .path()
        .join(format!("account-links-{installation}.json"));
    let preview = run(&["--dry-run", "--json", "--no-create-users"]);
    assert_eq!(preview["usersMatchedByEmail"], 1);
    assert_eq!(preview["usersRequiringReview"], 0);
    assert!(!receipt.exists());
    let other_old = Uuid::now_v7();
    snapshot = snapshot.push(
        "users.ndjson",
        json!({"publicId":other_old,"username":"other","email":"listener@example.test",
        "createdAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}),
    );
    snapshot.write(dir.path());
    let ambiguous = run(&["--dry-run", "--json"]);
    assert_eq!(ambiguous["usersMatchedByEmail"], 0);
    assert_eq!(ambiguous["usersRequiringReview"], 2);
    snapshot
        .streams
        .get_mut("users.ndjson")
        .expect("users")
        .pop();
    snapshot.write(dir.path());
    // Matching email and username must not identify two different people.
    let colliding_name = seed_user(&db, Uuid::now_v7(), "old-name").await;
    let split = run(&["--dry-run", "--json"]);
    assert_eq!(split["usersMatchedByEmail"], 0);
    assert_eq!(split["usersRequiringReview"], 1);
    let (ok, _, _) = run_import(&url, dir.path(), &[]);
    assert!(!ok);
    assert!(!receipt.exists());
    sqlx::query(
        "UPDATE native_users SET username='another', normalized_username='ANOTHER' WHERE id=$1",
    )
    .bind(colliding_name)
    .execute(&db.pool)
    .await
    .expect("resolve the username collision");
    let first = run(&["--json"]);
    assert_eq!(first["usersMatchedByEmail"], 1);
    assert_eq!(first["statesCreated"], 1);
    assert!(receipt.is_file());
    let profile: (String, bool, bool) =
        sqlx::query_as("SELECT username,enabled,is_admin FROM native_users WHERE id=$1")
            .bind(owner)
            .fetch_one(&db.pool)
            .await
            .expect("profile");
    assert_eq!(profile, ("listener".into(), true, false));
    sqlx::query("UPDATE native_users SET email='new@example.test',normalized_email='NEW@EXAMPLE.TEST',username='new',normalized_username='NEW' WHERE id=$1")
        .bind(owner).execute(&db.pool).await.expect("listener renames account");
    let other = seed_user(&db, Uuid::now_v7(), "listener").await;
    let repeat = run(&["--json"]);
    assert_eq!(repeat["usersLinked"], 1);
    assert_eq!(repeat["usersMatchedByEmail"], 0);
    assert_eq!(repeat["statesUnchanged"], 1);
    let wrong_owner: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_track_user_states WHERE user_id=$1")
            .bind(other)
            .fetch_one(&db.pool)
            .await
            .expect("unrelated account");
    assert_eq!(wrong_owner, 0);
    db.drop().await;
}

#[tokio::test]
async fn preview_identifies_rebuilt_tracks_by_artist_release_and_title() {
    let db = TestDb::create("user_state_titles").await;
    let url = common::database_url(&db.name);
    let user = Uuid::now_v7();
    let old_track = Uuid::now_v7();
    seed_user(&db, user, "listener").await;
    let (_, track) = seed_catalog(&db, 845, "still hungover", "Ella Langley").await;
    let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE public_api_id=$1")
        .bind(track)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut rating = track_state(user, old_track, true, 0, "2024-03-01T00:00:00Z");
    rating["rating"] = json!(1);
    let evidence = json!({"kind":"track", "publicId":old_track, "artist":"  ELLA   LANGLEY ", "release":"Still Hungover", "title":title.to_uppercase()});
    let snapshot = export_for_user(user)
        .push("entity-states.ndjson", rating.clone())
        .push("catalog-evidence.ndjson", evidence.clone());
    snapshot.write(dir.path());
    let report = |args: &[&str]| {
        let (ok, text, err) = run_import(&url, dir.path(), args);
        assert!(ok, "{err}");
        serde_json::from_str::<Value>(&text).unwrap()
    };
    let installation = melodee_persistence::installation_id(&db.pool)
        .await
        .unwrap();
    let saved_links = dir.path().join(format!("track-links-{installation}.json"));
    let dry = report(&["--dry-run", "--json"]);
    assert_eq!(dry["tracksMatchedByMetadata"], 1);
    assert_eq!(dry["matchedPreferences"]["ratings"], 1);
    assert_eq!(dry["matchedPreferences"]["favoriteTracks"], 1);
    assert!(!saved_links.exists());
    let (ok, text, err) = run_import(&url, dir.path(), &["--dry-run"]);
    assert!(ok, "{err}");
    let ratings = text
        .lines()
        .find(|line| line.contains("Ratings (1 to 5)"))
        .unwrap();
    assert_eq!(
        ratings.split_whitespace().skip(4).collect::<Vec<_>>(),
        ["1", "1", "0"]
    );
    assert!(!text.contains("scripts/reconcile_legacy_user_state.py"));

    // Conflicting legacy identities, and an identically titled track on another release.
    let other = Uuid::now_v7();
    let mut other_evidence = evidence.clone();
    other_evidence["publicId"] = json!(other);
    let mut other_rating = rating.clone();
    other_rating["entityPublicId"] = json!(other);
    let two_sources = || {
        export_for_user(user)
            .push("entity-states.ndjson", rating.clone())
            .push("entity-states.ndjson", other_rating.clone())
            .push(
                "track-plays.ndjson",
                json!({"userPublicId":user,"trackPublicId":old_track,
                "playedAt":"2024-02-15T00:00:00Z","durationSeconds":180,"completed":true}),
            )
            .push("catalog-evidence.ndjson", evidence.clone())
    };
    two_sources()
        .push("catalog-evidence.ndjson", other_evidence.clone())
        .write(dir.path());
    let duplicates = report(&["--dry-run", "--json"]);
    assert_eq!(duplicates["tracksWithAmbiguousMatches"], 2);
    assert_eq!(duplicates["matchedPreferences"]["ratings"], 0);
    other_evidence["release"] = json!("Other Release");
    let growing_export = two_sources().push("catalog-evidence.ndjson", other_evidence);
    growing_export.write(dir.path());
    let unrelated = report(&["--dry-run", "--json"]);
    assert_eq!(unrelated["tracksMatchedByMetadata"], 1);
    assert_eq!(unrelated["matchedPreferences"]["ratings"], 1);
    let (ok, _, _) = run_import(&url, dir.path(), &["--require-all-matches"]);
    assert!(!ok && !saved_links.exists());

    let first = report(&["--json"]);
    assert_eq!(first["statesCreated"], 1);
    assert_eq!(first["statesUnmatched"], 1);
    assert_eq!(first["playsInserted"], 1);
    assert!(saved_links.exists());
    let stored: Value = serde_json::from_slice(&std::fs::read(&saved_links).unwrap()).unwrap();
    assert_eq!(
        stored["approved"][0],
        json!({"kind":"track","legacyId":old_track,"targetId":track})
    );
    let saved: (bool, i16) = sqlx::query_as("SELECT s.starred, s.rating::smallint FROM native_track_user_states s JOIN tracks t ON t.id=s.track_id WHERE t.public_api_id=$1")
        .bind(track).fetch_one(&db.pool).await.unwrap();
    assert_eq!(saved, (true, 1));

    let (_, second_track) = seed_catalog(&db, 846, "still hungover", "Ella Langley").await;
    sqlx::query("UPDATE tracks SET title=$1 WHERE public_api_id=$2")
        .bind(&title)
        .bind(second_track)
        .execute(&db.pool)
        .await
        .unwrap();
    let (_, arriving) = seed_catalog(&db, 847, "Other Release", "Ella Langley").await;
    sqlx::query("UPDATE tracks SET title=$1 WHERE public_api_id=$2")
        .bind(&title)
        .bind(arriving)
        .execute(&db.pool)
        .await
        .unwrap();
    let next = report(&["--json"]);
    assert_eq!(next["statesCreated"], 1);
    assert_eq!(next["statesUnchanged"], 1);
    assert_eq!(next["playsInserted"], 0);
    assert_eq!(next["playsDuplicate"], 1);
    assert_eq!(next["tracksMatchedByMetadata"], 1);
    let rerun = report(&["--json"]);
    assert_eq!(rerun["statesUnchanged"], 2);
    assert_eq!(rerun["tracksMatchedByMetadata"], 0);
    assert_eq!(rerun["tracksWithAmbiguousMatches"], 0);

    // A first-time import into the same catalog must review the two live editions.
    let fresh = tempfile::tempdir().unwrap();
    snapshot.write(fresh.path());
    let (ok, out, err) = run_import(&url, fresh.path(), &["--dry-run", "--json"]);
    assert!(ok, "{err}");
    let ambiguous: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(ambiguous["tracksWithAmbiguousMatches"], 1);
    assert_eq!(ambiguous["matchedPreferences"]["ratings"], 0);
    let reviewed = fresh.path().join("links.json");
    std::fs::write(
        &reviewed,
        json!({"schemaVersion":1,"targetInstallationId":installation,
        "approved":[{"kind":"track","legacyId":old_track,"targetId":second_track}]})
        .to_string(),
    )
    .unwrap();
    let (ok, out, err) = run_import(
        &url,
        fresh.path(),
        &[
            "--json",
            "--dry-run",
            "--identity-map",
            reviewed.to_str().unwrap(),
        ],
    );
    assert!(ok, "{err}");
    let approved: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(approved["matchedPreferences"]["ratings"], 1);
    assert_eq!(approved["tracksWithAmbiguousMatches"], 0);
    db.drop().await;
}
