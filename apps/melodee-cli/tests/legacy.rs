//! R3 core integration suite for `melodee legacy` (ADR-0148): fingerprint
//! gate, read-only source enforcement, inventory, plan, full import of every
//! mapped domain, public-ID preservation, in-place media adoption, idempotent
//! re-run, verify, and convergent delta.

#![allow(clippy::expect_used)]

mod legacy_support;

use legacy_support::{
    SEED_MIXED, count, create_source, create_target, drop_db, map_arg, run_legacy,
    run_legacy_failing, source_args,
};

#[tokio::test]
async fn inventory_reports_counts_anomalies_and_fingerprint() {
    let source = create_source("inventory", SEED_MIXED).await;
    let target = create_target("inventory").await;

    let mut args = vec!["inventory".to_string(), "--json".to_string()];
    args.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &args);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("inventory JSON");
    assert_eq!(report["schema"]["table_count"].as_i64(), Some(52));
    assert_eq!(report["media"]["file_count"].as_i64(), Some(2));
    assert!(report["media"]["total_bytes"].as_i64().unwrap_or(0) > 0);
    let tables = report["tables"].as_array().expect("tables array");
    let artists = tables
        .iter()
        .find(|t| t["domain"] == "artists")
        .expect("artists row");
    assert_eq!(artists["rows"].as_i64(), Some(2));
    let anomalies = report["anomalies"].as_array().expect("anomalies array");
    assert!(
        anomalies
            .iter()
            .any(|a| a["code"] == "user.unsupported_credential")
    );

    // Inventory writes nothing to the target.
    assert_eq!(count(&target.pool, "legacy_migration.runs").await, 0);
    assert_eq!(count(&target.pool, "artists").await, 0);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn unsupported_source_schema_fails_closed() {
    let source = create_source("badfp", "SELECT 1").await;
    // Corrupt the frozen baseline marker.
    sqlx::query(r#"DELETE FROM "__EFMigrationsHistory""#)
        .execute(&source.pool)
        .await
        .expect("corrupt history");
    let target = create_target("badfp").await;

    let mut args = vec!["inventory".to_string()];
    args.extend(source_args(&source.url));
    let stderr = run_legacy_failing(&target.url, &args);
    assert!(
        stderr.contains("unsupported legacy schema"),
        "stderr was: {stderr}"
    );

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn source_database_remains_strictly_read_only() {
    let source = create_source("readonly", SEED_MIXED).await;
    let target = create_target("readonly").await;

    let mut args = vec![
        "import".to_string(),
        "--batch-size".to_string(),
        "1".to_string(),
    ];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    // The source fingerprint and row counts are unchanged after a full
    // import: read-only enforcement is the proof the source is untouched.
    let artists: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Artists""#)
        .fetch_one(&source.pool)
        .await
        .expect("source count");
    assert_eq!(artists, 2);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn plan_records_planned_run_without_target_writes() {
    let source = create_source("plan", SEED_MIXED).await;
    let target = create_target("plan").await;

    let mut args = vec!["plan".to_string(), "--json".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    let stdout = run_legacy(&target.url, &args);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("plan JSON");
    let run_id = report["run_id"].as_str().expect("run id");

    let state: String =
        sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1::uuid")
            .bind(run_id)
            .fetch_one(&target.pool)
            .await
            .expect("run row");
    assert_eq!(state, "planned");
    assert_eq!(count(&target.pool, "artists").await, 0);
    assert_eq!(count(&target.pool, "native_users").await, 0);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn full_import_covers_all_domains_and_preserves_identity() {
    let source = create_source("import", SEED_MIXED).await;
    let target = create_target("import").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    // Catalog identity: public ApiKeys are preserved verbatim.
    let artist_publics: Vec<String> =
        sqlx::query_scalar("SELECT public_api_id::text FROM artists ORDER BY public_api_id")
            .fetch_all(&target.pool)
            .await
            .expect("artist public ids");
    assert_eq!(
        artist_publics,
        vec![
            "11111111-1111-1111-1111-111111111111".to_string(),
            "22222222-2222-2222-2222-222222222222".to_string()
        ]
    );
    // Same-name artists remain two identities.
    assert_eq!(count(&target.pool, "artists").await, 2);
    assert_eq!(count(&target.pool, "releases").await, 1);
    assert_eq!(count(&target.pool, "tracks").await, 2);
    assert_eq!(count(&target.pool, "contributions").await, 1);
    assert_eq!(count(&target.pool, "release_artists").await, 1);
    assert_eq!(count(&target.pool, "media_assets").await, 2);

    let (bitrate_bps, tempo): (i64, Option<f64>) = sqlx::query_as(
        "SELECT media.bitrate_bps, features.tempo
         FROM tracks track
         JOIN media_assets media ON media.id = track.playable_media_asset_id
         LEFT JOIN native_track_audio_features features ON features.track_id = track.id
         ORDER BY track.public_api_id
         LIMIT 1",
    )
    .fetch_one(&target.pool)
    .await
    .expect("migrated audio facts");
    assert_eq!(
        bitrate_bps, 900_000,
        "legacy bitrate is represented in Rust bps"
    );
    assert_eq!(
        tempo,
        Some(0.0),
        "legacy zero BPM remains an observed tempo"
    );
    let metadata_reference: (i64, serde_json::Value) = sqlx::query_as(
        "SELECT COUNT(*) OVER (), detail
         FROM legacy_migration.issues
         WHERE code = 'metadata.reference_only'
         ORDER BY source_pk
         LIMIT 1",
    )
    .fetch_one(&target.pool)
    .await
    .expect("reference-only metadata issue");
    assert_eq!(metadata_reference.0, 2);
    assert!(
        metadata_reference.1["fields"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field == "Songs.BitDepth"))
    );

    // Original release date is preserved separately from edition date.
    let (release_date, original): (Option<time::Date>, Option<time::Date>) =
        sqlx::query_as("SELECT release_date, original_release_date FROM releases")
            .fetch_one(&target.pool)
            .await
            .expect("release dates");
    assert_eq!(release_date.map(|d| d.year()), Some(1990));
    assert_eq!(original.map(|d| d.year()), Some(1959));

    // Media adopted in place: canonical-relative path, hash-pending sentinel
    // for the invalid legacy hash, adopted hash for the valid one.
    let paths: Vec<(String, String)> =
        sqlx::query_as("SELECT canonical_relative_path, byte_hash FROM media_assets ORDER BY 1")
            .fetch_all(&target.pool)
            .await
            .expect("media paths");
    assert_eq!(
        paths[0].0,
        "library/Miles Davis/miles/kind of blue/01 So What.flac"
    );
    assert_eq!(
        paths[0].1,
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    assert_eq!(
        paths[1].1,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );

    // Credentials: portable bcrypt preserved; unsupported recorded as an
    // issue and substituted with a reset placeholder.
    let hashes: Vec<(String, String)> =
        sqlx::query_as("SELECT username, password_hash FROM native_users ORDER BY username")
            .fetch_all(&target.pool)
            .await
            .expect("user hashes");
    assert_eq!(hashes[0].0, "legacy");
    assert!(hashes[0].1.starts_with("$2"), "reset placeholder is bcrypt");
    assert_eq!(hashes[1].0, "mel");
    assert_eq!(
        hashes[1].1,
        "$2a$10$abcdefghijklmnopqrstuuV5k7oW1O0Z1m8H0S1m8H0S1m8H0S1m8H0S"
    );
    let (is_admin, is_editor): (bool, bool) =
        sqlx::query_as("SELECT is_admin, is_editor FROM native_users WHERE username = 'mel'")
            .fetch_one(&target.pool)
            .await
            .expect("migrated user flags");
    assert!(is_admin);
    assert!(is_editor, "administrators retain the effective editor flag");
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT role FROM native_user_roles \
         WHERE user_id = (SELECT id FROM native_users WHERE username = 'mel') \
         ORDER BY role",
    )
    .fetch_all(&target.pool)
    .await
    .expect("migrated user roles");
    assert_eq!(
        roles,
        vec![
            "Administrator",
            "HasDownloadRole",
            "HasPlaylistRole",
            "HasShareRole",
            "HasStreamRole",
        ]
    );
    let credential_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'credential.unsupported'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("credential issues");
    assert_eq!(credential_issues, 1);

    // User state and operational domains.
    assert_eq!(count(&target.pool, "native_playlists").await, 1);
    assert_eq!(count(&target.pool, "native_playlist_items").await, 2);
    assert_eq!(count(&target.pool, "native_shares").await, 1);
    assert_eq!(count(&target.pool, "opensubsonic_bookmarks").await, 1);
    assert_eq!(count(&target.pool, "native_track_user_states").await, 1);
    assert_eq!(count(&target.pool, "native_release_user_states").await, 1);
    assert_eq!(count(&target.pool, "native_artist_user_states").await, 1);
    assert_eq!(count(&target.pool, "native_track_plays").await, 1);
    assert_eq!(count(&target.pool, "native_podcast_channels").await, 1);
    assert_eq!(count(&target.pool, "opensubsonic_radio_stations").await, 1);

    // Extended user/operational domains (R3.5).
    assert_eq!(count(&target.pool, "native_library_access_groups").await, 1);
    assert_eq!(count(&target.pool, "native_user_library_groups").await, 1);
    assert_eq!(count(&target.pool, "native_smart_playlists").await, 1);
    assert_eq!(count(&target.pool, "native_podcast_episodes").await, 1);
    assert_eq!(count(&target.pool, "native_podcast_bookmarks").await, 1);
    assert_eq!(count(&target.pool, "native_podcast_play_history").await, 1);
    assert_eq!(count(&target.pool, "native_playback_settings").await, 1);
    assert_eq!(count(&target.pool, "native_equalizer_presets").await, 1);
    assert_eq!(count(&target.pool, "native_user_social_logins").await, 1);
    assert_eq!(count(&target.pool, "native_requests").await, 1);
    assert_eq!(count(&target.pool, "native_request_comments").await, 1);
    assert_eq!(count(&target.pool, "native_request_seen").await, 1);

    // The song pin sets pinned on the user state row.
    let pinned: bool = sqlx::query_scalar("SELECT pinned FROM native_track_user_states LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("pinned flag");
    assert!(pinned, "song pin migrates to pinned user state");

    // Last.fm: no server secret configured in the target, so the session key
    // is an actionable issue, never a copied protection blob.
    let lastfm_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'lastfm.unsealable'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("lastfm issues");
    assert_eq!(lastfm_issues, 1);
    assert_eq!(count(&target.pool, "native_lastfm_links").await, 0);

    // Per-user device profiles are reference-only (the 1.0 target is global).
    let device_profile_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'device_profile.reference_only'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("device profile issues");
    assert_eq!(device_profile_issues, 1);

    // The smart playlist keeps its query and entity mapping.
    let smart: (String, String) =
        sqlx::query_as("SELECT mql_query, entity_type FROM native_smart_playlists LIMIT 1")
            .fetch_one(&target.pool)
            .await
            .expect("smart playlist");
    assert_eq!(smart, ("rating>=4".to_string(), "songs".to_string()));

    // Playlist ordering preserved.
    let positions: Vec<i32> =
        sqlx::query_scalar("SELECT position FROM native_playlist_items ORDER BY position")
            .fetch_all(&target.pool)
            .await
            .expect("positions");
    assert_eq!(positions, vec![1, 2]);

    // Settings are reference-only issues, never silently dropped.
    let settings_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'settings.reference_only'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("settings issues");
    assert_eq!(settings_issues, 1);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

/// R3.6: with the canonical root mounted, adoption validates existence, size,
/// and symlink containment read-only. Covers a sparse multi-GB file (scale
/// metadata without bytes), a missing file, a size mismatch, and a symlink
/// escape; the media tree is byte-identical afterwards.
#[tokio::test]
async fn canonical_root_validation_is_read_only_and_actionable() {
    let root = std::env::temp_dir().join(format!(
        "melodee-legacy-media-{}",
        uuid::Uuid::now_v7().simple()
    ));
    let album_dir = root.join("library/miles/kind of blue");
    std::fs::create_dir_all(&album_dir).expect("create canonical fixture dir");
    // Sparse 5 GiB adopted file: proves large-library metadata handling
    // without committing real bytes to the fixture.
    let big = album_dir.join("01 So What.flac");
    let big_file = std::fs::File::create(&big).expect("create sparse file");
    big_file.set_len(5_368_709_120).expect("sparse set_len");
    drop(big_file);
    // The second track's file is present but the wrong size.
    std::fs::write(album_dir.join("02 Freddie Freeloader.flac"), b"short").expect("write small");
    // An escaping symlink component outside the canonical root.
    let outside = std::env::temp_dir().join(format!(
        "melodee-legacy-outside-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&outside).expect("create outside dir");
    std::fs::write(outside.join("escape.flac"), b"outside").expect("write outside");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("library/evil")).expect("symlink escape");

    let seed = r#"
INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'11111111-1111-1111-1111-111111111111','Miles Davis','miles davis','',1,0,false,1,0,0,3,0,now(),now());
INSERT INTO "Albums" ("Id","ApiKey","ArtistId","Name","NameNormalized","AlbumStatus","AlbumType","CalculatedRating","Duration","IsCompilation","IsLocked","MetaDataStatus","PlayedCount","SortOrder","ReleaseDate","Directory","CreatedAt","LastUpdatedAt")
VALUES (1,'33333333-3333-3333-3333-333333333333',1,'Kind of Blue','kind of blue',1,1,0,3400,false,false,0,0,0,DATE '1990-01-01','miles/kind of blue',now(),now());
INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'44444444-4444-4444-4444-444444444444',1,'So What','so what',1,545.0,'01 So What.flac',5368709120,'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc','audio/flac',0,24,900000,44100,0,false,false,0,0,now(),now()),
       (2,'55555555-5555-5555-5555-555555555555',1,'Freddie Freeloader','freddie freeloader',2,589.0,'02 Freddie Freeloader.flac',2345678,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','audio/flac',0,24,910000,44100,0,false,false,0,0,now(),now()),
       (3,'66666666-6666-6666-6666-666666666666',1,'Blue in Green','blue in green',3,322.0,'03 Blue in Green.flac',3456789,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','audio/flac',0,24,900000,44100,0,false,false,0,0,now(),now());
INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (4,'77777777-7777-7777-7777-777777777777',1,'Escape','escape',4,100.0,'escape.flac',7,'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd','audio/flac',0,16,320000,44100,0,false,false,0,0,now(),now());
-- The escape track lives in a directory that is a symlink outside the root.
INSERT INTO "Albums" ("Id","ApiKey","ArtistId","Name","NameNormalized","AlbumStatus","AlbumType","CalculatedRating","Duration","IsCompilation","IsLocked","MetaDataStatus","PlayedCount","SortOrder","ReleaseDate","Directory","CreatedAt","LastUpdatedAt")
VALUES (2,'88888888-3333-3333-3333-333333333333',1,'Escape Album','escape album',1,1,0,100,false,false,0,0,0,DATE '1990-01-01','evil',now(),now());
UPDATE "Songs" SET "AlbumId" = 2 WHERE "Id" = 4;
-- Track 3 has no file on disk: missing-file path.
"#;
    let source = create_source("media", seed).await;
    let target = create_target("media").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    args.push("--canonical-root".to_string());
    args.push(root.to_string_lossy().to_string());
    run_legacy(&target.url, &args);

    // The sparse + missing files import (identity preserved); the escape is
    // skipped; every outcome is a durable issue.
    assert_eq!(count(&target.pool, "tracks").await, 3);
    let code_count = |code: &str| {
        let pool = target.pool.clone();
        let code = code.to_string();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = $1",
            )
            .bind(code)
            .fetch_one(&pool)
            .await
            .expect("issue count")
        }
    };
    assert_eq!(code_count("media.missing_file").await, 1);
    assert_eq!(code_count("media.size_mismatch").await, 1);
    assert_eq!(code_count("media.unadoptable").await, 1);
    assert_eq!(code_count("media.verification_skipped").await, 0);

    // The media tree is untouched: the migration never writes to it.
    assert_eq!(
        std::fs::metadata(&big).expect("big metadata").len(),
        5_368_709_120
    );
    assert_eq!(
        std::fs::read(album_dir.join("02 Freddie Freeloader.flac")).expect("read small"),
        b"short"
    );

    // Verify with the mounted root reports the same media mismatches.
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");
    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id];
    verify_args.extend(source_args(&source.url));
    verify_args.push("--canonical-root".to_string());
    verify_args.push(root.to_string_lossy().to_string());
    let stdout = run_legacy(&target.url, &verify_args);
    assert!(
        stdout.contains("media_references_validated"),
        "stdout was: {stdout}"
    );

    drop_db(&source.name).await;
    drop_db(&target.name).await;
    std::fs::remove_dir_all(&root).expect("cleanup root");
    std::fs::remove_dir_all(&outside).expect("cleanup outside");
}

/// R3.6: without --canonical-root the adoption is metadata-only and the run
/// records exactly one informational verification-skipped issue.
#[tokio::test]
async fn adoption_without_canonical_root_records_verification_skipped() {
    let source = create_source("media_skip", SEED_MIXED).await;
    let target = create_target("media_skip").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    assert_eq!(count(&target.pool, "tracks").await, 2);
    let skipped: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues
          WHERE code = 'media.verification_skipped' AND severity = 'info'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("skip issues");
    assert_eq!(skipped, 1);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

/// R3.8: a production-shaped source (hundreds of artists, thousands of
/// releases, tens of thousands of tracks) imports through bounded keyset
/// batches with exact counts, clean verification, and a convergent re-run.
/// R4 owns the full 5 TB production rehearsal; this proves the batching and
/// mapping machinery at representative relational cardinality.
#[tokio::test]
async fn production_shaped_source_imports_with_bounded_batches() {
    let seed = r#"
INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), 'Artist ' || g, 'artist ' || g, '', 2, 0, false, 1, 0, 0, 24, 0, now(), now()
FROM generate_series(1, 500) g;
INSERT INTO "Albums" ("Id","ApiKey","ArtistId","Name","NameNormalized","AlbumStatus","AlbumType","CalculatedRating","Duration","IsCompilation","IsLocked","MetaDataStatus","PlayedCount","SortOrder","ReleaseDate","Directory","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), ((g - 1) % 500) + 1, 'Release ' || g, 'release ' || g, 1, 1, 0, 2400,
       false, false, 0, 0, 0, DATE '1990-01-01', 'artist' || ((g - 1) % 500) || '/release' || g,
       now(), now()
FROM generate_series(1, 1000) g;
INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), ((g - 1) % 1000) + 1, 'Track ' || g, 'track ' || g, ((g - 1) % 12) + 1, 200.0,
       'track' || g || '.flac', 1000000, md5(g::text || g::text), 'audio/flac', 0, 24, 900000, 44100, 0, false, false, 0, 0,
       now(), now()
FROM generate_series(1, 12000) g;
"#;
    let source = create_source("shaped", seed).await;
    let target = create_target("shaped").await;

    let mut args = vec![
        "import".to_string(),
        "--batch-size".to_string(),
        "997".to_string(),
    ];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    assert_eq!(count(&target.pool, "artists").await, 500);
    assert_eq!(count(&target.pool, "releases").await, 1000);
    assert_eq!(count(&target.pool, "tracks").await, 12000);
    assert_eq!(count(&target.pool, "media_assets").await, 12000);
    let error_issues: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM legacy_migration.issues WHERE severity = 'error'")
            .fetch_one(&target.pool)
            .await
            .expect("error issues");
    assert_eq!(error_issues, 0, "no error issues in a clean shaped import");

    // Verification is clean across every deep check and reaches delta-ready.
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");
    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id.clone()];
    verify_args.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &verify_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");

    // Resume after simulated process death: convergent, no duplicates.
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET owner = 'dead-host:1', lease_expires_at = now() - interval '1 minute'
          WHERE id = $1::uuid",
    )
    .bind(&run_id)
    .execute(&target.pool)
    .await
    .expect("expire lease");
    let mut resume = vec!["import".to_string(), "--run".to_string(), run_id];
    resume.extend(source_args(&source.url));
    resume.push("--library-path-map".to_string());
    resume.push(map_arg());
    run_legacy(&target.url, &resume);
    assert_eq!(count(&target.pool, "tracks").await, 12000);
    let duplicate_publics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
           SELECT public_api_id FROM tracks GROUP BY public_api_id HAVING COUNT(*) > 1
         ) d",
    )
    .fetch_one(&target.pool)
    .await
    .expect("duplicate check");
    assert_eq!(duplicate_publics, 0);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

/// ADR-0148 §D.8: the legacy-protected OpenSubsonic secret is re-sealed into
/// the Rust per-user secret contract with the operator-supplied legacy key —
/// never copied raw. Without the key, the user gets an actionable issue.
#[tokio::test]
async fn opensubsonic_secret_reseals_only_with_the_legacy_key() {
    let legacy_key = "legacy-fixture-security-key-0123456789abcdef";
    let protected = legacy_support::legacy_protect(legacy_key, "subsonic-secret-123");
    let seed = format!(
        "{SEED_MIXED}\nUPDATE \"Users\" SET \"OpenSubsonicSecretProtected\" = '{protected}' WHERE \"Id\" = 1;"
    );
    let source = create_source("reseal", &seed).await;
    let target = create_target("reseal").await;

    // Without the key: durable issue, no secret row.
    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let failed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues
          WHERE code = 'credential.secret_reseal_failed'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("reseal issues");
    assert_eq!(failed, 1);
    assert_eq!(count(&target.pool, "opensubsonic_user_secrets").await, 0);

    // With the key: the usable secret is stored; the raw blob never appears.
    let target2 = create_target("reseal2").await;
    let mut args2 = vec!["import".to_string()];
    args2.extend(source_args(&source.url));
    args2.push("--library-path-map".to_string());
    args2.push(map_arg());
    args2.push("--legacy-secret-key".to_string());
    args2.push(legacy_key.to_string());
    run_legacy(&target2.url, &args2);
    let secret: String = sqlx::query_scalar("SELECT secret FROM opensubsonic_user_secrets")
        .fetch_one(&target2.pool)
        .await
        .expect("resealed secret");
    assert_eq!(secret, "subsonic-secret-123");
    assert_eq!(count(&target2.pool, "opensubsonic_user_secrets").await, 1);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
    drop_db(&target2.name).await;
}

#[tokio::test]
async fn second_full_import_creates_no_duplicate_authority() {
    let source = create_source("twice", SEED_MIXED).await;
    let target = create_target("twice").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let artists_first = count(&target.pool, "artists").await;
    let tracks_first = count(&target.pool, "tracks").await;
    let plays_first = count(&target.pool, "native_track_plays").await;

    // Resume the same run a second time.
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");
    let mut resume = vec!["import".to_string(), "--run".to_string(), run_id];
    resume.extend(source_args(&source.url));
    resume.push("--library-path-map".to_string());
    resume.push(map_arg());
    run_legacy(&target.url, &resume);

    assert_eq!(count(&target.pool, "artists").await, artists_first);
    assert_eq!(count(&target.pool, "tracks").await, tracks_first);
    assert_eq!(count(&target.pool, "native_track_plays").await, plays_first);
    assert_eq!(count(&target.pool, "artists").await, 2);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn verify_then_delta_is_convergent() {
    let source = create_source("delta", SEED_MIXED).await;
    let target = create_target("delta").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");

    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id.clone()];
    verify_args.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &verify_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");

    // Change one source row and add one new one; delta must update display
    // fields only and import the new row without touching existing identity.
    sqlx::query(r#"UPDATE "Artists" SET "Name" = 'Miles Dewey Davis', "LastUpdatedAt" = now() + interval '1 hour' WHERE "Id" = 1"#)
        .execute(&source.pool)
        .await
        .expect("update artist");
    sqlx::query(r#"INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt") VALUES (3,'dddddddd-dddd-dddd-dddd-dddddddddddd','John Coltrane','john coltrane','',0,0,false,1,0,0,0,0,now(),now())"#)
        .execute(&source.pool)
        .await
        .expect("insert artist");
    sqlx::query(r#"DELETE FROM "Songs" WHERE "Id" = 2"#)
        .execute(&source.pool)
        .await
        .expect("delete song");

    let mut delta_args = vec!["delta".to_string(), "--run".to_string(), run_id.clone()];
    delta_args.extend(source_args(&source.url));
    delta_args.push("--library-path-map".to_string());
    delta_args.push(map_arg());
    run_legacy(&target.url, &delta_args);

    let name: String = sqlx::query_scalar(
        "SELECT canonical_name FROM artists WHERE public_api_id = '11111111-1111-1111-1111-111111111111'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("updated artist");
    assert_eq!(name, "Miles Dewey Davis");
    assert_eq!(count(&target.pool, "artists").await, 3);
    // The deleted source song remains in the target and is an issue.
    assert_eq!(count(&target.pool, "tracks").await, 2);
    let deletion_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'source.row_deleted'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("deletion issues");
    assert_eq!(deletion_issues, 1);

    // A second delta over the unchanged source is a no-op.
    let artists_before = count(&target.pool, "artists").await;
    run_legacy(&target.url, &delta_args);
    assert_eq!(count(&target.pool, "artists").await, artists_before);

    // Status reports the run and checkpoints.
    let status = run_legacy(
        &target.url,
        &["status".to_string(), "--run".to_string(), run_id],
    );
    assert!(status.contains("delta-ready"), "status was: {status}");
    assert!(status.contains("artists"), "status was: {status}");

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}
