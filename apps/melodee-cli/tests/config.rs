//! PostgreSQL-backed CLI integration tests for `melodee config list|validate|export|import`.
//! (PRD-CONFIG-001/002, PRD-SETUP-001). Secrets are always redacted; import
//! previews by default and fails closed on redacted markers and unknown
//! keys.

#![allow(clippy::expect_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::process::Command;

use common::TestDb;
use melodee_persistence::{SECRET_REDACTION_MARKER, SettingsStore};
use serde_json::json;

/// Environment keys a host shell could leak into a config test run.
const SCRUBBED_ENV: &[&str] = &[
    "MELODEE__CONFIG_FILE",
    "MELODEE__EMAIL__ENABLED",
    "MELODEE__EMAIL__FROM_NAME",
    "MELODEE__EMAIL__FROM_EMAIL",
    "MELODEE__EMAIL__SMTP_HOST",
    "MELODEE__EMAIL__SMTP_PORT",
    "MELODEE__EMAIL__SMTP_USERNAME",
    "MELODEE__EMAIL__SMTP_PASSWORD",
    "MELODEE__EMAIL__SMTP_USE_SSL",
    "MELODEE__EMAIL__SMTP_USE_START_TLS",
    "MELODEE__INGEST__ENABLED",
    "MELODEE__INGEST__SOURCES_JSON",
    "MELODEE__INGEST__SOURCE_ROOT",
    "MELODEE__STORAGE__ROOT",
    "MELODEE__SEARCH__ENABLED",
    "MELODEE__SEARCH__ROOT",
    "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD",
];

fn run_cli(database_url: &str, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url);
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    let output = command.output().expect("run config CLI command");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI output is UTF-8")
}

fn run_cli_failing(database_url: &str, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url);
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    let output = command.output().expect("run config CLI command");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stderr).expect("CLI stderr is UTF-8")
}

fn db_url(db: &TestDb) -> String {
    common::database_url(&db.name)
}

#[tokio::test]
async fn config_list_shows_layered_values_with_secrets_redacted() {
    let db = TestDb::create("cli_config_list").await;
    let database_url = db_url(&db);

    SettingsStore::new()
        .set(
            &db.pool,
            "email.smtpPassword",
            &json!("super-secret-password"),
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("set secret");

    let stdout = run_cli(&database_url, &["config", "list"]);
    // Defaults and DB-sourced rows are listed with their source layer.
    assert!(stdout.contains("magic.enabled"), "stdout: {stdout}");
    assert!(stdout.contains("default"), "stdout: {stdout}");
    assert!(
        stdout.contains("database"),
        "database-sourced row: {stdout}"
    );
    // The secret is the fixed marker, never the value.
    assert!(stdout.contains(SECRET_REDACTION_MARKER), "stdout: {stdout}");
    assert!(
        !stdout.contains("super-secret-password"),
        "stdout: {stdout}"
    );
    db.drop().await;
}

#[tokio::test]
async fn config_export_and_import_round_trip_with_preview_default() {
    let db = TestDb::create("cli_config_roundtrip").await;
    let database_url = db_url(&db);

    SettingsStore::new()
        .set(
            &db.pool,
            "system.siteName",
            &json!("CLI Example"),
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("set");
    SettingsStore::new()
        .set(
            &db.pool,
            "email.smtpPassword",
            &json!("super-secret-password"),
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("set secret");

    // Export to a file.
    let export_path = std::env::temp_dir().join(format!(
        "melodee-wp08-export-{}.json",
        uuid::Uuid::now_v7().simple()
    ));
    let stdout = run_cli(
        &database_url,
        &[
            "config",
            "export",
            "--output",
            export_path.to_str().expect("utf-8 path"),
        ],
    );
    assert!(stdout.contains("exported"), "stdout: {stdout}");
    let exported = std::fs::read_to_string(&export_path).expect("read export");
    assert!(exported.contains(SECRET_REDACTION_MARKER));
    assert!(!exported.contains("super-secret-password"));

    let stderr = run_cli_failing(
        &database_url,
        &[
            "config",
            "export",
            "--output",
            export_path.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        stderr.contains("cannot write"),
        "existing export destination must be refused: {stderr}"
    );

    #[cfg(unix)]
    {
        let external = std::env::temp_dir().join(format!(
            "melodee-wp08-export-target-{}.json",
            uuid::Uuid::now_v7().simple()
        ));
        let link = std::env::temp_dir().join(format!(
            "melodee-wp08-export-link-{}.json",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::write(&external, b"keep me").expect("external target");
        std::os::unix::fs::symlink(&external, &link).expect("export symlink");
        let stderr = run_cli_failing(
            &database_url,
            &[
                "config",
                "export",
                "--output",
                link.to_str().expect("utf-8 path"),
            ],
        );
        assert!(
            stderr.contains("cannot write"),
            "symlink must be refused: {stderr}"
        );
        assert_eq!(
            std::fs::read(&external).expect("external unchanged"),
            b"keep me"
        );
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&external);
    }

    #[cfg(unix)]
    {
        let import_link = std::env::temp_dir().join(format!(
            "melodee-wp08-import-link-{}.json",
            uuid::Uuid::now_v7().simple()
        ));
        std::os::unix::fs::symlink(&export_path, &import_link).expect("import symlink");
        let stderr = run_cli_failing(
            &database_url,
            &[
                "config",
                "import",
                import_link.to_str().expect("utf-8 path"),
            ],
        );
        assert!(
            stderr.contains("cannot read"),
            "config import must reject a symlink input: {stderr}"
        );
        std::fs::remove_file(&import_link).expect("remove import symlink");
    }

    // Import into a fresh database: preview first (default), then apply.
    let target = TestDb::create("cli_config_import_target").await;
    let target_url = db_url(&target);
    let path_arg = export_path.to_str().expect("utf-8 path");
    let stdout = run_cli(&target_url, &["config", "import", path_arg]);
    assert!(stdout.contains("preview only"), "stdout: {stdout}");
    assert!(stdout.contains("system.siteName"), "stdout: {stdout}");
    assert!(
        stdout.contains("secrets_skipped:   [\"email.smtpPassword\"]"),
        "stdout: {stdout}"
    );
    assert!(
        SettingsStore::new()
            .get(&target.pool, "system.siteName")
            .await
            .expect("get")
            .is_none(),
        "preview writes nothing"
    );

    let stdout = run_cli(&target_url, &["config", "import", path_arg, "--apply"]);
    assert!(stdout.contains("import applied"), "stdout: {stdout}");
    let site = SettingsStore::new()
        .get(&target.pool, "system.siteName")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        site.value,
        melodee_persistence::SettingValueView::Value(json!("CLI Example"))
    );
    assert!(
        SettingsStore::new()
            .get(&target.pool, "email.smtpPassword")
            .await
            .expect("get")
            .is_none(),
        "redacted marker is never stored"
    );
    let _ = std::fs::remove_file(&export_path);
    target.drop().await;
    db.drop().await;
}

#[tokio::test]
async fn config_import_rejects_unknown_keys_and_markers_fail_closed() {
    let db = TestDb::create("cli_config_reject").await;
    let database_url = db_url(&db);
    let path = std::env::temp_dir().join(format!(
        "melodee-wp08-reject-{}.json",
        uuid::Uuid::now_v7().simple()
    ));

    // Unknown key on apply without --allow-unknown fails closed.
    std::fs::write(
        &path,
        serde_json::to_string(&json!({
            "format": "melodee-config-export/v1",
            "settings": { "custom.unknown": "x" }
        }))
        .expect("serialize"),
    )
    .expect("write doc");
    let stderr = run_cli_failing(
        &database_url,
        &["config", "import", path.to_str().expect("utf-8"), "--apply"],
    );
    assert!(stderr.contains("unknown"), "stderr: {stderr}");

    // The redaction marker for a non-secret key is rejected at preview too.
    std::fs::write(
        &path,
        serde_json::to_string(&json!({
            "format": "melodee-config-export/v1",
            "settings": { "system.siteName": SECRET_REDACTION_MARKER }
        }))
        .expect("serialize"),
    )
    .expect("write doc");
    let stderr = run_cli_failing(
        &database_url,
        &["config", "import", path.to_str().expect("utf-8")],
    );
    assert!(stderr.contains("marker"), "stderr: {stderr}");

    // Invalid JSON input fails closed.
    std::fs::write(&path, "not json").expect("write broken");
    let stderr = run_cli_failing(
        &database_url,
        &["config", "import", path.to_str().expect("utf-8")],
    );
    assert!(stderr.contains("not valid JSON"), "stderr: {stderr}");
    let _ = std::fs::remove_file(&path);
    db.drop().await;
}

#[tokio::test]
async fn config_validate_gates_on_required_checks() {
    let db = TestDb::create("cli_config_validate").await;
    let database_url = db_url(&db);

    // Fresh database: no administrator → required check fails → non-zero.
    let stderr = run_cli_failing(&database_url, &["config", "validate"]);
    assert!(
        stderr.contains("required preflight checks are failing"),
        "stderr: {stderr}"
    );

    // Bootstrap the first admin (the documented first-run step), then
    // validation passes (advisory checks may fail without blocking).
    let mut command = Command::new(env!("CARGO_BIN_EXE_melodee-cli"));
    command.args([
        "users",
        "bootstrap-admin",
        "--username",
        "config-admin",
        "--email",
        "config-admin@example.test",
    ]);
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    command
        .env(melodee_persistence::ENV_DATABASE_URL, &database_url)
        .env(
            "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD",
            "Mel0deeR0cks-Bootstrap!",
        );
    let output = command.output().expect("run bootstrap-admin");
    assert!(
        output.status.success(),
        "bootstrap-admin failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = run_cli(&database_url, &["config", "validate"]);
    assert!(stdout.contains("database"), "stdout: {stdout}");
    assert!(stdout.contains("admin_user"), "stdout: {stdout}");
    assert!(stdout.contains("can_complete=true"), "stdout: {stdout}");
    db.drop().await;
}
