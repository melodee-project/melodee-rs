//! Administrator-facing effective configuration. Host-owned settings have no write path.
use super::settings_error;
use crate::admin_access::{AdminAccessState, AdminPrincipal};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use melodee_persistence::{
    SECRET_REDACTION_MARKER, SettingKind, SettingsError,
    SettingsError::InvalidValue,
    SettingsResolver, SettingsStore, env_var_name, known_setting,
    runtime_settings::{self as runtime, Control, StartupValues},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    key: String,
    env_name: String,
    source: &'static str,
    value: Value,
    saved_value: Value,
    saved_source: &'static str,
    pending_restart: bool,
    can_reset: bool,
    editable: bool,
    secret: bool,
    usage: &'static str,
    kind: &'static str,
    options: &'static [&'static str],
}

fn shape(kind: SettingKind) -> (&'static str, &'static [&'static str]) {
    match kind {
        SettingKind::Bool => ("boolean", &[]),
        SettingKind::Int { .. } => ("integer", &[]),
        SettingKind::Float { .. } => ("number", &[]),
        SettingKind::Text => ("text", &[]),
        SettingKind::Enum(options) => ("choice", options),
    }
}

fn runtime_value(
    key: &str,
    default: Option<&str>,
    overrides: Option<&StartupValues>,
) -> (Option<String>, &'static str) {
    if let Some(value) = overrides.and_then(|values| values.get(key)) {
        return (Some(value.clone()), "database");
    }
    if let Ok(value) = std::env::var(key) {
        return (Some(value), "environment");
    }
    match key {
        "MELODEE__SERVER__BIND" if let Ok(value) = std::env::var("MELODEE_BIND") => {
            (Some(value), "environment")
        }
        "MELODEE__LOGGING__LEVEL" if let Ok(value) = std::env::var("RUST_LOG") => {
            (Some(value), "environment")
        }
        "MELODEE__NATIVE__PUBLIC_ORIGIN" => (
            Some(format!("http://{}", crate::system::configured_bind())),
            "default",
        ),
        "MELODEE__OPENSUBSONIC__PODCASTS_ENABLED" => {
            runtime_value("MELODEE__PODCAST__ENABLED", Some("true"), overrides)
        }
        _ => (
            default.map(str::to_owned),
            if default.is_some() {
                "default"
            } else {
                "unset"
            },
        ),
    }
}

fn startup_entry(def: &runtime::RuntimeDefinition, saved: &StartupValues) -> Entry {
    let &(key, default, control) = def;
    let (current, source) = runtime_value(key, default, runtime::active());
    let (next, saved_source) = runtime_value(key, default, Some(saved));
    let pending_restart = current != next || source != saved_source;
    let secret = matches!(
        key,
        "MELODEE__DATABASE__URL" | "MELODEE__NATIVE__AUTH__JWT_KEY"
    );
    let display = |raw: Option<String>| match raw {
        Some(_) if secret => json!(SECRET_REDACTION_MARKER),
        Some(s) => json!(s),
        None => Value::Null,
    };
    let (kind, options) = match control {
        Control::Flag => ("boolean", &[][..]),
        Control::Number(..) => ("integer", &[][..]),
        Control::Choice(options) => ("choice", options),
        _ => ("text", &[][..]),
    };
    Entry {
        key: if key == runtime::PODCAST_ENABLED {
            runtime::PODCAST_REGISTRY_KEY
        } else {
            key
        }
        .into(),
        env_name: key.into(),
        source,
        value: display(current),
        saved_value: display(next),
        saved_source,
        pending_restart,
        can_reset: saved.contains_key(key),
        editable: !matches!(control, Control::Host),
        secret,
        usage: "startup",
        kind,
        options,
    }
}

async fn entries(pool: &PgPool) -> Result<Vec<Entry>, SettingsError> {
    let saved = runtime::load(pool).await?;
    let mut result: Vec<_> = runtime::DEFINITIONS
        .iter()
        .map(|def| startup_entry(def, &saved))
        .collect();
    for entry in SettingsResolver::from_env(pool)?.list_effective().await? {
        let setting = entry.setting;
        let key = &setting.key;
        if key == runtime::PODCAST_REGISTRY_KEY || runtime::reserved_alias(key) {
            continue;
        }
        let known = known_setting(key);
        let secret = setting.is_secret;
        let source = if setting.value.is_none() {
            "unset"
        } else {
            setting.source.as_str()
        };
        let value = setting.to_redacted_json();
        let (kind, options) = known
            .map(|k| shape(k.kind))
            .unwrap_or((if secret { "text" } else { "json" }, &[]));
        let usage = match crate::settings_inventory::setting_inventory(key) {
            Some(e) if !e.inert_reason.is_empty() => "legacy",
            Some(e) if !e.consumer.is_empty() => "registry",
            _ => "custom",
        };
        result.push(Entry {
            env_name: env_var_name(key),
            can_reset: entry.admin_override,
            editable: !runtime::host_registry_key(key)
                && (known.is_some()
                    || entry.has_row
                    || setting.source == melodee_persistence::SettingSource::ConfigFile),
            key: key.clone(),
            source,
            value: value.clone(),
            saved_value: value,
            saved_source: source,
            pending_restart: false,
            secret,
            usage,
            kind,
            options,
        });
    }
    Ok(result)
}

pub(super) async fn list(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
) -> Response {
    match entries(&state.pool).await {
        Ok(items) => Json(items).into_response(),
        Err(e) => settings_error(e),
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Change {
    Set { value: Value },
    Reset,
}

fn invalid(key: &str, reason: impl Into<String>) -> SettingsError {
    InvalidValue {
        key: key.into(),
        reason: reason.into(),
    }
}

fn validate_startup(
    key: &str,
    values: StartupValues,
) -> Result<Option<melodee_domain::PolicyRuleSet>, SettingsError> {
    runtime::preview(values, || {
        let group = key.split("__").nth(1);
        macro_rules! check {
            ($config:path) => {
                <$config>::from_env().map_err(|e| invalid(key, e.to_string()))?
            };
        }
        match group {
            Some("WORK") => {
                check!(melodee_persistence::WorkEngineConfig);
            }
            Some("INGEST" | "INSPECTION" | "NORMALIZATION" | "VALIDATION" | "PUBLICATION") => {
                let cfg = check!(melodee_ingestion::IngestionConfig);
                if key == melodee_ingestion::ENV_INGEST_ENABLED {
                    cfg.storage_roots()
                        .map_err(|e| invalid(key, e.to_string()))?;
                }
            }
            Some("PODCAST") => {
                check!(crate::podcast_worker::PodcastConfig);
            }
            Some("CATALOG") => {
                check!(melodee_catalog::CatalogConfig);
            }
            Some("SEARCH") => {
                check!(melodee_search::SearchConfig);
            }
            Some("ENRICHMENT") => {
                check!(crate::enrichment_worker::EnrichmentWorkerConfig);
            }
            Some("DISCOVERY") => {
                check!(crate::discovery_worker::DiscoveryWorkerConfig);
            }
            Some("POLICY") if key.contains("__WORKER__") => {
                check!(crate::policy_worker::PolicyWorkerConfig);
            }
            Some("POLICY") => {
                let cfg = check!(melodee_policy::PolicyConfig);
                return melodee_policy::default_rule_set(&cfg)
                    .map(Some)
                    .map_err(|e| invalid(key, e.to_string()));
            }
            _ => {}
        }
        Ok(None)
    })
}

async fn write(
    pool: &PgPool,
    actor: uuid::Uuid,
    key: &str,
    change: Change,
) -> Result<(), SettingsError> {
    let key = if key == runtime::PODCAST_REGISTRY_KEY {
        runtime::PODCAST_ENABLED
    } else {
        key
    };
    if runtime::host_registry_key(key)
        || runtime::reserved_alias(key) && runtime::definition(key).is_none()
    {
        return Err(invalid(key, "managed on the deployment host"));
    }
    let mut tx = pool.begin().await?;
    // Cross-key invariants (retry windows, search limits and immutable policies) see concurrent saves in order.
    sqlx::query("SELECT pg_advisory_xact_lock(1296387140, 51)")
        .execute(&mut *tx)
        .await?;
    let reset = matches!(change, Change::Reset);
    if let Some(&(name, _, control)) = runtime::definition(key) {
        if matches!(control, Control::Host) {
            return Err(invalid(key, "managed on the deployment host"));
        }
        let mut next = runtime::load(&mut *tx).await?;
        match change {
            Change::Set { value } => {
                let text = runtime::text_value(&value)
                    .filter(|v| control.accepts(v))
                    .ok_or_else(|| invalid(key, "value outside the supported type or range"))?;
                sqlx::query("INSERT INTO server_startup_overrides(key,value,updated_by) VALUES($1,$2,$3) ON CONFLICT(key) DO UPDATE SET value=EXCLUDED.value,updated_by=EXCLUDED.updated_by,updated_at=now()")
                    .bind(name).bind(&text).bind(actor).execute(&mut *tx).await?;
                next.insert(name.into(), text);
            }
            Change::Reset => {
                sqlx::query("DELETE FROM server_startup_overrides WHERE key=$1")
                    .bind(name)
                    .execute(&mut *tx)
                    .await?;
                next.remove(name);
            }
        }
        if let Some(rule_set) = validate_startup(name, next)? {
            let rules: Option<Value> = sqlx::query_scalar(
                "SELECT rules FROM policy_rule_sets WHERE name=$1 AND version=$2",
            )
            .bind(&rule_set.name)
            .bind(i64::from(rule_set.version))
            .fetch_optional(&mut *tx)
            .await?;
            if rules.is_some_and(|rules| rules != json!(rule_set.rules)) {
                return Err(invalid(
                    key,
                    "save a new policy rule-set version before changing its rules",
                ));
            }
        }
    } else {
        let stored = SettingsStore::new().read_raw_value(&mut *tx, key).await?;
        let known = known_setting(key).is_some();
        if !known && stored.is_none() && !SettingsResolver::from_env(pool)?.configured_in_file(key)
        {
            return Err(SettingsError::InvalidKey("unknown server setting"));
        }
        let protect_custom =
            !known && (stored.is_none() || std::env::var_os(env_var_name(key)).is_some());
        match change {
            Change::Set { value } => {
                SettingsStore::new()
                    .set(
                        &mut *tx,
                        key,
                        &value,
                        Some(protect_custom),
                        time::OffsetDateTime::now_utc(),
                    )
                    .await?;
                sqlx::query("UPDATE server_settings SET admin_preferred=true WHERE key=$1")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
            Change::Reset => {
                sqlx::query("DELETE FROM server_settings WHERE key=$1 AND admin_preferred")
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    if key.starts_with("email.") {
        let mut resolver = SettingsResolver::from_env(pool)?;
        resolver.snapshot_group(&mut *tx, "email.").await?;
        crate::notifications::EmailSettings::from_resolved(&resolver)
            .await
            .map_err(|e| invalid(key, e.to_string()))?;
    }
    tx.commit().await?;
    tracing::info!(%actor,setting=key,reset,"administrator changed configuration");
    Ok(())
}

pub(super) async fn update(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(key): Path<String>,
    Json(change): Json<Change>,
) -> Response {
    match write(&state.pool, admin.public_api_id, &key, change).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => settings_error(e),
    }
}
