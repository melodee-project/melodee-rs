//! fault-path integration suite for `melodee legacy` (ADR-0148): fencing
//! against concurrent owners, restart convergence after a mid-run kill,
//! orphan references recorded as issues, target public-ID conflicts, and
//! malformed source data.

#![allow(clippy::expect_used)]

mod legacy_support;

use legacy_support::{
    SEED_MIXED, count, create_source, create_source_with_ddl, create_target, drop_db, map_arg,
    run_legacy, run_legacy_failing, source_args,
};

#[tokio::test]
async fn fencing_blocks_a_second_live_owner() {
    let source = create_source("fence", SEED_MIXED).await;
    let target = create_target("fence").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");

    // Simulate a second live process by planting a fresh lease.
    sqlx::query(
        "UPDATE legacy_migration.runs
            SET owner = 'other-host:9999', lease_expires_at = now() + interval '10 minutes'
         WHERE id = $1::uuid",
    )
    .bind(&run_id)
    .execute(&target.pool)
    .await
    .expect("plant lease");

    let mut resume = vec!["import".to_string(), "--run".to_string(), run_id];
    resume.extend(source_args(&source.url));
    resume.push("--library-path-map".to_string());
    resume.push(map_arg());
    let stderr = run_legacy_failing(&target.url, &resume);
    assert!(
        stderr.contains("held by another live owner"),
        "stderr was: {stderr}"
    );

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn stale_lease_is_reclaimable_and_restart_converges() {
    let source = create_source("restart", SEED_MIXED).await;
    let target = create_target("restart").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");
    let artists_first = count(&target.pool, "artists").await;

    // Simulate process death: expired lease, partial state markers intact.
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

    assert_eq!(count(&target.pool, "artists").await, artists_first);
    assert_eq!(count(&target.pool, "tracks").await, 2);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn orphan_references_become_issues_not_aborts() {
    let seed = format!(
        "{SEED_MIXED}\n{}",
        r#"INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
           VALUES (9,'eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee',4242,'Orphan','orphan',1,60.0,'orphan.flac',10,'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee','audio/flac',0,16,320000,44100,0,false,false,0,0,now(),now());"#
    );
    let source = create_source("orphan", &seed).await;
    let target = create_target("orphan").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    // The orphan song is skipped, the run completes, and the issue is durable.
    assert_eq!(count(&target.pool, "tracks").await, 2);
    let issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'catalog.broken_song_album_fk'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("orphan issues");
    assert_eq!(issues, 1);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn target_public_id_conflict_is_an_error_issue_not_overwrite() {
    let source = create_source("conflict", SEED_MIXED).await;
    let target = create_target("conflict").await;

    // Pre-existing target row outside the run sharing a legacy public ID.
    sqlx::query(
        "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, revision, created_at, updated_at)
         VALUES (uuidv7(), '11111111-1111-1111-1111-111111111111', 'Preexisting',
            'preexisting', 'Preexisting', 'unknown', 'verified', 1, now(), now())",
    )
    .execute(&target.pool)
    .await
    .expect("seed conflict");

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    // The pre-existing row is untouched; the conflict is a durable issue.
    let name: String = sqlx::query_scalar(
        "SELECT canonical_name FROM artists WHERE public_api_id = '11111111-1111-1111-1111-111111111111'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("conflict row");
    assert_eq!(name, "Preexisting");
    let issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'target.public_id_conflict'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("conflict issues");
    assert!(
        issues >= 1,
        "expected at least one conflict issue, got {issues}"
    );

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn malformed_media_metadata_becomes_issue_not_unsafe_path() {
    let seed = format!(
        "{SEED_MIXED}\n{}",
        r#"INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
           VALUES (10,'ffffffff-ffff-ffff-ffff-ffffffffffff',1,'Traversal','traversal',3,60.0,'../../../etc/passwd',10,'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff','audio/flac',0,16,320000,44100,0,false,false,0,0,now(),now());"#
    );
    let source = create_source("malformed", &seed).await;
    let target = create_target("malformed").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    assert_eq!(count(&target.pool, "tracks").await, 2);
    let issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.issues WHERE code = 'media.unadoptable'",
    )
    .fetch_one(&target.pool)
    .await
    .expect("unadoptable issues");
    assert_eq!(issues, 1);

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn empty_source_imports_cleanly_and_verifies() {
    let source = create_source("empty", "SELECT 1").await;
    let target = create_target("empty").await;

    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);

    let run_id: String = sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
        .fetch_one(&target.pool)
        .await
        .expect("run id");
    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id];
    verify_args.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &verify_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn old_schema_fingerprint_is_rejected() {
    // A schema without the frozen baseline's final migration row must fail
    // closed even though every table exists.
    let ddl = legacy_support::LEGACY_DDL.replace(
        "('20260714155936_AddDeleteSourceResidueAfterIngestSetting', '10.0.0');",
        "('20200101000000_AncientBaseline', '10.0.0');",
    );
    assert!(ddl.contains("AncientBaseline"), "fixture rewrite applied");
    let source = create_source_with_ddl("oldfp", &ddl, "SELECT 1").await;
    let target = create_target("oldfp").await;

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
