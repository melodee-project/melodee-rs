//! R4 freeze/baseline integration suite (ADR-0149): immutable baseline
//! capture with redacted configuration fingerprint, freeze gating, abort on
//! source writes during the freeze window, and the acknowledgement-gated
//! cutover-ready transition.

#![allow(clippy::expect_used)]

mod legacy_support;

use legacy_support::{
    SEED_MIXED, create_source, create_target, drop_db, map_arg, run_legacy, run_legacy_failing,
    source_args,
};

/// Imports the mixed source and verifies to `delta-ready`; returns the run id.
async fn import_and_verify(source_url: &str, target_url: &str, target: &sqlx::PgPool) -> String {
    let mut args = vec!["import".to_string()];
    args.extend(source_args(source_url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(target_url, &args);
    let run_id: String =
        sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
            .fetch_one(target)
            .await
            .expect("run id");
    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id.clone()];
    verify_args.extend(source_args(source_url));
    let stdout = run_legacy(target_url, &verify_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");
    run_id
}

#[tokio::test]
async fn baseline_records_immutable_snapshot_with_redacted_fingerprint() {
    let source = create_source("baseline", SEED_MIXED).await;
    let target = create_target("baseline").await;

    let mut plan_args = vec!["plan".to_string(), "--json".to_string()];
    plan_args.extend(source_args(&source.url));
    plan_args.push("--library-path-map".to_string());
    plan_args.push(map_arg());
    let stdout = run_legacy(&target.url, &plan_args);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("plan JSON");
    let run_id = report["run_id"].as_str().expect("run id").to_string();

    let mut args = vec!["baseline".to_string(), "--run".to_string(), run_id.clone()];
    args.extend(source_args(&source.url));
    run_legacy(&target.url, &args);

    let notes: serde_json::Value = sqlx::query_scalar(
        "SELECT notes FROM legacy_migration.runs WHERE id = $1::uuid",
    )
    .bind(&run_id)
    .fetch_one(&target.pool)
    .await
    .expect("run notes");
    let baseline = &notes["baseline"];
    assert_eq!(
        baseline["schema_fingerprint"].as_str(),
        Some("20260714155936_AddDeleteSourceResidueAfterIngestSetting")
    );
    assert_eq!(baseline["table_count"].as_i64(), Some(52));
    assert!(baseline["snapshot_marker"].as_str().is_some_and(|m| m.starts_with("rows:")));
    // Redaction: the settings fingerprint is keys only — the seeded key is
    // present, and no value material appears anywhere in the baseline.
    let keys = baseline["configuration_fingerprint"]["settings_keys"]
        .as_array()
        .expect("settings keys");
    assert!(keys.iter().any(|k| k == "library.scan.enabled"));
    let serialized = baseline.to_string();
    assert!(
        !serialized.contains("true\","),
        "baseline must not carry setting values: {serialized}"
    );

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

#[tokio::test]
async fn freeze_gates_on_state_and_acknowledgement() {
    let source = create_source("freeze", SEED_MIXED).await;
    let target = create_target("freeze").await;

    // Freeze before verify is an error (run is still importing).
    let mut args = vec!["import".to_string()];
    args.extend(source_args(&source.url));
    args.push("--library-path-map".to_string());
    args.push(map_arg());
    run_legacy(&target.url, &args);
    let run_id: String =
        sqlx::query_scalar("SELECT id::text FROM legacy_migration.runs LIMIT 1")
            .fetch_one(&target.pool)
            .await
            .expect("run id");
    let mut freeze_args = vec!["freeze".to_string(), "--run".to_string(), run_id.clone()];
    freeze_args.extend(source_args(&source.url));
    let stderr = run_legacy_failing(&target.url, &freeze_args);
    assert!(stderr.contains("delta-ready"), "stderr was: {stderr}");

    // Verify the same run to delta-ready (no second import).
    let mut verify_args = vec!["verify".to_string(), "--run".to_string(), run_id.clone()];
    verify_args.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &verify_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");

    // Freeze without acknowledgement: evidence recorded, state stays
    // delta-ready.
    let mut freeze_args = vec!["freeze".to_string(), "--run".to_string(), run_id.clone()];
    freeze_args.extend(source_args(&source.url));
    freeze_args.push("--library-path-map".to_string());
    freeze_args.push(map_arg());
    let stdout = run_legacy(&target.url, &freeze_args);
    assert!(stdout.contains("delta-ready"), "stdout was: {stdout}");

    // --acknowledge-cutover requires --note.
    let mut bad = vec![
        "freeze".to_string(),
        "--run".to_string(),
        run_id.clone(),
        "--acknowledge-cutover".to_string(),
    ];
    bad.extend(source_args(&source.url));
    let stderr = run_legacy_failing(&target.url, &bad);
    assert!(stderr.contains("--note"), "stderr was: {stderr}");

    // Acknowledged freeze: cutover-ready with durable evidence.
    let mut ack = vec![
        "freeze".to_string(),
        "--run".to_string(),
        run_id.clone(),
        "--acknowledge-cutover".to_string(),
        "--note".to_string(),
        "rehearsal drill: freeze verified by operator".to_string(),
    ];
    ack.extend(source_args(&source.url));
    let stdout = run_legacy(&target.url, &ack);
    assert!(stdout.contains("cutover-ready"), "stdout was: {stdout}");

    let state: String =
        sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1::uuid")
            .bind(&run_id)
            .fetch_one(&target.pool)
            .await
            .expect("run state");
    assert_eq!(state, "cutover-ready");
    let notes: serde_json::Value = sqlx::query_scalar(
        "SELECT notes FROM legacy_migration.runs WHERE id = $1::uuid",
    )
    .bind(&run_id)
    .fetch_one(&target.pool)
    .await
    .expect("run notes");
    assert_eq!(
        notes["freeze"]["acknowledgement"].as_str(),
        Some("rehearsal drill: freeze verified by operator")
    );
    assert!(notes["freeze"]["snapshot_marker"].as_str().is_some());

    // A cutover-ready run no longer accepts deltas (monotonicity).
    let mut delta_args = vec!["delta".to_string(), "--run".to_string(), run_id.clone()];
    delta_args.extend(source_args(&source.url));
    delta_args.push("--library-path-map".to_string());
    delta_args.push(map_arg());
    let stderr = run_legacy_failing(&target.url, &delta_args);
    assert!(stderr.contains("delta-ready"), "stderr was: {stderr}");

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}

/// A legacy write observed between the two freeze markers aborts the pass
/// and leaves the run delta-ready. Deterministic: the freeze window is held
/// open by a slow 2000-artist delta at batch size 1 while the test watches
/// the target for the first imported row before writing to the source.
#[tokio::test]
async fn freeze_aborts_when_source_writes_during_window() {
    let mut seed = String::from(SEED_MIXED);
    seed.push_str(
        r#"INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
           SELECT 100000 + g, gen_random_uuid(), 'Delta Artist ' || g, 'delta artist ' || g, '',
                  0, 0, false, 1, 0, 0, 0, 0,
                  now() - interval '1 day', now() + interval '1 day'
           FROM generate_series(1, 2000) g;"#,
    );
    let source = create_source("freezefail", &seed).await;
    let target = create_target("freezefail").await;

    let run_id = import_and_verify(&source.url, &target.url, &target.pool).await;

    // Bump the source so the next delta has 2000 artists to import.
    sqlx::query(r#"UPDATE "Artists" SET "LastUpdatedAt" = now() WHERE "Id" >= 100001"#)
        .execute(&source.pool)
        .await
        .expect("bump delta rows");

    let target_pool = target.pool.clone();
    let source_pool = source.pool.clone();
    let writer = tokio::spawn(async move {
        // The freeze window opens with the marker read, then the delta runs.
        // Wait until the delta has visibly started applying display-field
        // updates (revision > 1), then write to the source inside the window.
        for _ in 0..600 {
            let updated: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM artists WHERE revision > 1",
            )
            .fetch_one(&target_pool)
            .await
            .expect("watch target");
            if updated > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        sqlx::query(
            r#"INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
               VALUES (999999,'eeeeeeee-ffff-ffff-ffff-ffffffffffff','Freeze Intruder','freeze intruder','',0,0,false,1,0,0,0,0,now(),now())"#,
        )
        .execute(&source_pool)
        .await
        .expect("intruder write");
    });

    let mut freeze_args = vec![
        "freeze".to_string(),
        "--run".to_string(),
        run_id.clone(),
        "--batch-size".to_string(),
        "1".to_string(),
    ];
    freeze_args.extend(source_args(&source.url));
    let freeze_task =
        tokio::task::spawn_blocking(move || run_legacy_failing(&target.url, &freeze_args));
    writer.await.expect("writer join");
    let stderr = freeze_task.await.expect("freeze join");
    assert!(
        stderr.contains("source writes observed during the freeze window"),
        "stderr was: {stderr}"
    );

    let state: String =
        sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1::uuid")
            .bind(&run_id)
            .fetch_one(&target.pool)
            .await
            .expect("run state");
    assert_eq!(state, "delta-ready", "aborted freeze leaves run delta-ready");

    drop_db(&source.name).await;
    drop_db(&target.name).await;
}
