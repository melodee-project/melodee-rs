//! settings/onboarding persistence acceptance (ADR-0134 §3,
//! PRD-SETUP-001/002, PRD-CONFIG-001..003, PRD-AUTH-007): migration shape,
//! round-trip, secret-flag enforcement, redaction behavior of list/get,
//! singleton enforcement, bounds, layered resolution precedence, and the
//! export/import preview/apply contract.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    CONFIG_EXPORT_FORMAT, KNOWN_SETTINGS, OnboardingStore, SECRET_REDACTION_MARKER, SettingSource,
    SettingValueView, SettingsError, SettingsResolver, SettingsStore, apply_config_import,
    export_config, known_setting, migration_status, plan_config_import,
};
use serde_json::json;
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn migration_shape_and_sql_backstop_bounds() {
    let db = TestDb::create("settings_shape").await;

    // Both tables exist empty; the onboarding singleton admits at most one row.
    let (settings_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM server_settings")
        .fetch_one(&db.pool)
        .await
        .expect("settings count");
    assert_eq!(settings_count, 0);

    sqlx::query("INSERT INTO onboarding_state (id) VALUES (true)")
        .execute(&db.pool)
        .await
        .expect("singleton insert");
    assert!(
        sqlx::query("INSERT INTO onboarding_state (id) VALUES (true)")
            .execute(&db.pool)
            .await
            .is_err(),
        "second singleton row violates the PK"
    );
    assert!(
        sqlx::query("INSERT INTO onboarding_state (id) VALUES (false)")
            .execute(&db.pool)
            .await
            .is_err(),
        "id = false violates the singleton CHECK"
    );

    // The migration CHECKs are the fail-closed backstop against raw SQL.
    for bad_sql in [
        "INSERT INTO server_settings (key, value) VALUES ('  ', '1')",
        "INSERT INTO server_settings (key, value) VALUES (repeat('k', 256), '1')",
        "INSERT INTO server_settings (key, value) VALUES ('k', to_jsonb(repeat('v', 65537)))",
    ] {
        assert!(
            sqlx::query(bad_sql).execute(&db.pool).await.is_err(),
            "{bad_sql}"
        );
    }

    // Migration currency is visible to the onboarding preflight.
    let status = migration_status(&db.pool).await.expect("migration status");
    assert!(status.is_current(), "{status:?}");
    assert_eq!(status.applied, status.latest_available);
    db.drop().await;
}

#[tokio::test]
async fn settings_round_trip_redaction_and_secret_flags() {
    let db = TestDb::create("settings_roundtrip").await;
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();

    // Non-secret known key: kind-validated, readable in full.
    let setting = store
        .set(&db.pool, "magic.enabled", &json!(false), None, now)
        .await
        .expect("set magic.enabled");
    assert_eq!(setting.value, SettingValueView::Value(json!(false)));
    assert!(!setting.is_secret);

    // A known secret key is force-flagged secret even when the caller does
    // not ask, and every read representation redacts it.
    let secret = store
        .set(
            &db.pool,
            "email.smtpPassword",
            &json!("super-secret"),
            None,
            now,
        )
        .await
        .expect("set secret");
    assert!(secret.is_secret, "known secrets are force-flagged");
    assert_eq!(secret.value, SettingValueView::Redacted);

    let fetched = store
        .get(&db.pool, "email.smtpPassword")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(fetched.value, SettingValueView::Redacted);
    let listed = store.list(&db.pool, 50, 0).await.expect("list");
    let listed_secret = listed
        .iter()
        .find(|row| row.key == "email.smtpPassword")
        .expect("listed");
    assert_eq!(listed_secret.value, SettingValueView::Redacted);
    // The serializable representation is the fixed marker, never the value.
    assert_eq!(
        listed_secret.value.to_json(),
        json!(SECRET_REDACTION_MARKER)
    );

    // The raw accessor (internal resolver/import use) still sees the value.
    let (raw, is_secret) = store
        .read_raw_value(&db.pool, "email.smtpPassword")
        .await
        .expect("raw")
        .expect("row");
    assert_eq!(raw, json!("super-secret"));
    assert!(is_secret);

    // Kind validation is fail-closed at the store boundary too.
    assert!(matches!(
        store
            .set(&db.pool, "defaults.batchSize", &json!(100), None, now)
            .await,
        Err(SettingsError::InvalidValue { .. })
    ));
    assert!(matches!(
        store
            .set(&db.pool, "magic.enabled", &json!("yes"), None, now)
            .await,
        Err(SettingsError::InvalidValue { .. })
    ));
    // The redaction marker is never accepted as a value.
    assert!(matches!(
        store
            .set(
                &db.pool,
                "magic.enabled",
                &json!(SECRET_REDACTION_MARKER),
                None,
                now,
            )
            .await,
        Err(SettingsError::RedactedMarker(_))
    ));
    // Bounds: blank/over-long keys and over-long values.
    assert!(matches!(
        store.set(&db.pool, "  ", &json!(1), None, now).await,
        Err(SettingsError::InvalidKey(_))
    ));
    let long_key = "k".repeat(256);
    assert!(matches!(
        store.set(&db.pool, &long_key, &json!(1), None, now).await,
        Err(SettingsError::InvalidKey(_))
    ));
    let long_value = json!("v".repeat(65536));
    assert!(matches!(
        store
            .set(&db.pool, "system.siteName", &long_value, None, now)
            .await,
        Err(SettingsError::InvalidValue { .. })
    ));
    db.drop().await;
}

#[tokio::test]
async fn delete_is_restricted_to_custom_keys() {
    let db = TestDb::create("settings_delete").await;
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();

    // Custom (non-registry) keys round-trip and are deletable; operators may
    // flag them secret.
    let custom = store
        .set(
            &db.pool,
            "custom.pluginToggle",
            &json!("on"),
            Some(true),
            now,
        )
        .await
        .expect("set custom");
    assert!(custom.is_secret);
    assert_eq!(custom.value, SettingValueView::Redacted);
    store
        .delete(&db.pool, "custom.pluginToggle")
        .await
        .expect("delete custom");
    assert!(
        store
            .get(&db.pool, "custom.pluginToggle")
            .await
            .expect("get")
            .is_none()
    );

    // Known registry keys fail closed on delete (override with a value
    // instead of silently reverting to the built-in default).
    store
        .set(&db.pool, "system.siteName", &json!("Example"), None, now)
        .await
        .expect("set known");
    assert!(matches!(
        store.delete(&db.pool, "system.siteName").await,
        Err(SettingsError::KnownKeyNotDeletable(_))
    ));
    db.drop().await;
}

#[tokio::test]
async fn onboarding_state_singleton_and_completion() {
    let db = TestDb::create("settings_onboarding").await;
    let store = OnboardingStore::new();

    // Absent row: not completed, no evidence.
    let state = store.get_state(&db.pool).await.expect("state");
    assert!(!state.is_completed());
    assert!(state.evidence.is_none());
    assert!(!store.is_completed(&db.pool).await.expect("is_completed"));

    // Evidence accumulates before completion (configuration imports).
    store
        .record_config_import(
            &db.pool,
            json!({"importedAt": "2026-08-14T00:00:00Z", "actor": "ops", "added": 2}),
        )
        .await
        .expect("record import");
    let state = store.get_state(&db.pool).await.expect("state");
    assert!(!state.is_completed(), "evidence alone does not complete");
    assert_eq!(
        state.evidence.as_ref().expect("evidence")["configImports"]
            .as_array()
            .expect("array")
            .len(),
        1
    );

    // Completion writes the durable marker with evidence.
    let now = OffsetDateTime::now_utc();
    let completed = store
        .complete(
            &db.pool,
            "0192abcd-0000-7000-8000-000000000000",
            json!({"checks": [{"name": "database", "passed": true}]}),
            now,
        )
        .await
        .expect("complete");
    assert!(completed.is_completed());
    // timestamptz stores microsecond precision.
    assert!(
        (completed.completed_at.expect("completed_at") - now).abs() < Duration::microseconds(1)
    );
    assert!(store.is_completed(&db.pool).await.expect("is_completed"));

    // The audit keeps only the bounded tail.
    for index in 0..25 {
        store
            .record_config_import(&db.pool, json!({"seq": index}))
            .await
            .expect("record");
    }
    let state = store.get_state(&db.pool).await.expect("state");
    let entries = state.evidence.as_ref().expect("evidence")["configImports"]
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(entries.len(), 20, "bounded audit trail");
    assert_eq!(entries.first().expect("first")["seq"], json!(5));
    assert_eq!(entries.last().expect("last")["seq"], json!(24));

    // Singleton shape backstops: invalid completion metadata is rejected in
    // code before SQL.
    assert!(matches!(
        store.complete(&db.pool, "  ", json!({}), now).await,
        Err(SettingsError::InvalidOnboarding(_))
    ));
    assert!(matches!(
        store.complete(&db.pool, "ops", json!([1, 2]), now).await,
        Err(SettingsError::InvalidOnboarding(_))
    ));
    db.drop().await;
}

#[tokio::test]
async fn resolution_layers_env_over_file_over_database_over_default() {
    let db = TestDb::create("settings_layers").await;
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();
    // Injected environment (process env mutation is unsafe under edition
    // 2024 and racy across parallel tests).
    let no_env = &|_: &str| None;

    // Default layer: no row, no env, no file.
    let resolver = SettingsResolver::with_env_reader(&db.pool, no_env);
    let resolved = resolver.resolve("magic.enabled").await.expect("resolve");
    assert_eq!(resolved.value, Some(json!(true)));
    assert_eq!(resolved.source, SettingSource::Default);
    assert!(!resolved.is_secret);

    // Database layer beats the default.
    store
        .set(&db.pool, "magic.enabled", &json!(false), None, now)
        .await
        .expect("set");
    let resolved = resolver.resolve("magic.enabled").await.expect("resolve");
    assert_eq!(resolved.value, Some(json!(false)));
    assert_eq!(resolved.source, SettingSource::Database);

    // Config-file layer beats the database.
    let file_path = std::env::temp_dir().join(format!(
        "melodee-wp08-config-{}.json",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::write(&file_path, r#"{"magic.enabled": true}"#).expect("write config file");
    let resolver =
        SettingsResolver::with_config_file(&db.pool, &file_path, no_env).expect("file resolver");
    let resolved = resolver.resolve("magic.enabled").await.expect("resolve");
    assert_eq!(resolved.value, Some(json!(true)));
    assert_eq!(resolved.source, SettingSource::ConfigFile);
    // A malformed file fails closed.
    std::fs::write(&file_path, "not json").expect("write broken file");
    assert!(matches!(
        SettingsResolver::with_config_file(&db.pool, &file_path, no_env),
        Err(SettingsError::ConfigFile(_))
    ));

    #[cfg(unix)]
    {
        let link_path = file_path.with_extension("link.json");
        std::os::unix::fs::symlink(&file_path, &link_path).expect("symlink config file");
        assert!(matches!(
            SettingsResolver::with_config_file(&db.pool, &link_path, no_env),
            Err(SettingsError::ConfigFile(_))
        ));
        std::fs::remove_file(&link_path).expect("remove config-file symlink");
    }
    let _ = std::fs::remove_file(&file_path);

    // Environment beats everything (legacy overlay direction); env text is
    // parsed per kind and fails closed on a type mismatch.
    let env = &|name: &str| match name {
        "MELODEE__MAGIC__ENABLED" => Some("true".to_string()),
        "MELODEE__DEFAULTS__BATCHSIZE" => Some("not-an-int".to_string()),
        _ => None,
    };
    let resolver = SettingsResolver::with_env_reader(&db.pool, env);
    let resolved = resolver.resolve("magic.enabled").await.expect("resolve");
    assert_eq!(resolved.value, Some(json!(true)));
    assert_eq!(resolved.source, SettingSource::Environment);
    assert!(matches!(
        resolver.resolve("defaults.batchSize").await,
        Err(SettingsError::InvalidValue { .. })
    ));

    // resolve_all covers the registry plus custom rows, redaction-safe.
    store
        .set(&db.pool, "email.smtpPassword", &json!("s3cret"), None, now)
        .await
        .expect("set secret");
    store
        .set(&db.pool, "custom.toggle", &json!(true), None, now)
        .await
        .expect("set custom");
    let all = SettingsResolver::with_env_reader(&db.pool, no_env)
        .resolve_all()
        .await
        .expect("resolve all");
    assert_eq!(all.len(), KNOWN_SETTINGS.len() + 1);
    let secret = all
        .iter()
        .find(|entry| entry.key == "email.smtpPassword")
        .expect("secret entry");
    assert!(secret.is_secret);
    assert_eq!(secret.to_redacted_json(), json!(SECRET_REDACTION_MARKER));
    let custom = all
        .iter()
        .find(|entry| entry.key == "custom.toggle")
        .expect("custom entry");
    assert_eq!(custom.value, Some(json!(true)));
    db.drop().await;
}

#[tokio::test]
async fn export_redacts_and_import_preview_apply_are_fail_closed() {
    let db = TestDb::create("settings_export_import").await;
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();

    store
        .set(&db.pool, "system.siteName", &json!("Example"), None, now)
        .await
        .expect("set site name");
    store
        .set(
            &db.pool,
            "email.smtpPassword",
            &json!("super-secret"),
            None,
            now,
        )
        .await
        .expect("set secret");
    store
        .set(&db.pool, "custom.toggle", &json!(true), None, now)
        .await
        .expect("set custom");

    // Export: known keys carry DB values or defaults; every secret is the
    // fixed marker; the warning header is present.
    let document = export_config(&db.pool, now).await.expect("export");
    assert_eq!(document.format, CONFIG_EXPORT_FORMAT);
    assert!(document.warning.contains("REDACTED"));
    assert_eq!(
        document.exported_at,
        now.format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339")
    );
    let settings = &document.settings;
    assert_eq!(settings["system.siteName"], json!("Example"));
    assert_eq!(settings["magic.enabled"], json!(true), "built-in default");
    assert_eq!(
        settings["email.smtpPassword"],
        json!(SECRET_REDACTION_MARKER)
    );
    assert_eq!(settings["custom.toggle"], json!(true));
    // Secret keys never leak: the serialized document contains no value.
    let serialized = serde_json::to_string(&document).expect("serialize");
    assert!(!serialized.contains("super-secret"));
    // Unset secret keys with no stored row are absent entirely.
    assert!(!settings.contains_key("security.secretKey"));
    // Compromised/placeholder legacy defaults are never exported.
    assert!(!settings.contains_key("encryption.privateKey"));
    assert!(!settings.contains_key("jellyfin.token.pepper"));

    // Preview against a fresh target database.
    let target = TestDb::create("settings_import_target").await;
    let document_value = serde_json::to_value(&document).expect("document value");
    let stored = store.list_raw(&target.pool).await.expect("list raw");
    let plan = plan_config_import(&document_value, &stored).expect("plan");
    assert!(plan.added.contains(&"system.siteName".to_string()));
    assert_eq!(plan.secrets_skipped, vec!["email.smtpPassword"]);
    assert_eq!(plan.unknown, vec!["custom.toggle"]);
    assert!(plan.warnings.iter().any(|w| w.contains("re-enter")));
    assert!(plan.secrets_reentered.is_empty());

    // Apply without allow-unknown fails closed and writes nothing.
    assert!(matches!(
        apply_config_import(&target.pool, &document_value, &plan, false, "ops", now).await,
        Err(SettingsError::InvalidDocument(_))
    ));
    assert!(
        store
            .get(&target.pool, "system.siteName")
            .await
            .expect("get")
            .is_none(),
        "rejected apply is atomic: nothing written"
    );

    // Apply with unknown admitted: rows written, secrets skipped, audit
    // evidence recorded atomically in the onboarding singleton.
    apply_config_import(&target.pool, &document_value, &plan, true, "ops", now)
        .await
        .expect("apply");
    let site = store
        .get(&target.pool, "system.siteName")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(site.value, SettingValueView::Value(json!("Example")));
    assert!(
        store
            .get(&target.pool, "email.smtpPassword")
            .await
            .expect("get")
            .is_none(),
        "redacted secret marker is never stored"
    );
    assert!(
        store
            .get(&target.pool, "custom.toggle")
            .await
            .expect("get")
            .is_some()
    );
    let state = OnboardingStore::new()
        .get_state(&target.pool)
        .await
        .expect("state");
    let imports = state.evidence.as_ref().expect("evidence")["configImports"]
        .as_array()
        .expect("array")
        .clone();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["actor"], json!("ops"));
    assert_eq!(imports[0]["unknownAdmitted"], json!(true));
    assert!(
        !state.is_completed(),
        "import evidence does not complete onboarding"
    );

    // A document carrying the marker for a non-secret key is rejected even
    // at plan time (fail closed).
    let mut tampered = document_value.clone();
    tampered["settings"]["system.siteName"] = json!(SECRET_REDACTION_MARKER);
    assert!(matches!(
        plan_config_import(&tampered, &stored),
        Err(SettingsError::RedactedMarker(_))
    ));

    // A kind violation is rejected with the key named (never the value).
    let mut invalid = document_value.clone();
    invalid["settings"]["validation.maximumAlbumYear"] = json!("2150");
    assert!(matches!(
        plan_config_import(&invalid, &stored),
        Err(SettingsError::InvalidValue { key, .. }) if key == "validation.maximumAlbumYear"
    ));
    target.drop().await;
    db.drop().await;
}

#[tokio::test]
async fn onboarding_bypass_evidence_query_shape() {
    // The bypass heuristic itself lives in the server layer; here the
    // persistence-facing evidence queries are exercised: catalog rows and
    // imported known settings rows are both detectable.
    let db = TestDb::create("settings_bypass_evidence").await;
    let (releases_empty,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM releases)")
        .fetch_one(&db.pool)
        .await
        .expect("releases check");
    assert!(!releases_empty);
    SettingsStore::new()
        .set(
            &db.pool,
            "system.siteName",
            &json!("Migrated"),
            None,
            OffsetDateTime::now_utc() + Duration::seconds(1),
        )
        .await
        .expect("set known");
    let (known_rows,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM server_settings WHERE key = 'system.siteName')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("known row check");
    assert!(known_rows);
    assert!(known_setting("system.siteName").is_some());
    db.drop().await;
}
