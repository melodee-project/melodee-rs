//! Operator inventory of server startup settings. Only explicitly editable keys
//! can enter the runtime snapshot. The operating-system environment is never mutated.
use serde_json::Value;
use sqlx::PgPool;
use std::{cell::RefCell, collections::BTreeMap, sync::OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum Control {
    Host,
    Flag,
    Number(i64, i64),
    Choice(&'static [&'static str]),
    Marker,
}
use Control::{Choice, Flag, Host, Marker, Number};
pub type RuntimeDefinition = (&'static str, Option<&'static str>, Control);
#[rustfmt::skip]
pub const DEFINITIONS: &[RuntimeDefinition] = &[
    ("MELODEE__ADMIN_UI__DIST", Some("admin-ui/dist"), Host),
    ("MELODEE__CATALOG__BATCH_SIZE", Some("16"), Number(1, u32::MAX as i64)),
    ("MELODEE__CATALOG__CONCURRENCY", Some("2"), Number(1, u32::MAX as i64)),
    ("MELODEE__CATALOG__ENABLED", Some("true"), Flag),
    ("MELODEE__CATALOG__POLL_INTERVAL_SECONDS", Some("5"), Number(1, u32::MAX as i64)),
    ("MELODEE__CONFIG_FILE", None, Host),
    ("MELODEE__DATABASE__ACQUIRE_TIMEOUT_SECONDS", Some("30"), Host),
    ("MELODEE__DATABASE__IDLE_TIMEOUT_SECONDS", Some("600"), Host),
    ("MELODEE__DATABASE__MAX_CONNECTIONS", Some("32"), Host),
    ("MELODEE__DATABASE__MIN_CONNECTIONS", Some("2"), Host),
    ("MELODEE__DATABASE__URL", None, Host),
    ("MELODEE__DISCOVERY__WORKER__BATCH_SIZE", Some("500"), Number(1, u32::MAX as i64)),
    ("MELODEE__DISCOVERY__WORKER__ENABLED", Some("true"), Flag),
    ("MELODEE__DISCOVERY__WORKER__MAX_BATCHES_PER_PASS", Some("100"), Number(1, u32::MAX as i64)),
    ("MELODEE__DISCOVERY__WORKER__POLL_INTERVAL_SECONDS", Some("300"), Number(1, u32::MAX as i64)),
    ("MELODEE__ENRICHMENT__WORKER__BATCH_SIZE", Some("8"), Number(1, 32)),
    ("MELODEE__ENRICHMENT__WORKER__ENABLED", Some("true"), Flag),
    ("MELODEE__ENRICHMENT__WORKER__LEASE_SECONDS", Some("120"), Number(5, 900)),
    ("MELODEE__ENRICHMENT__WORKER__POLL_INTERVAL_SECONDS", Some("15"), Number(1, 86400)),
    ("MELODEE__INGEST__ENABLED", Some("true"), Flag),
    ("MELODEE__INGEST__READY_MARKER_NAME", Some(".melodee-ready"), Marker),
    ("MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS", Some("0"), Number(0, 4294967295)),
    ("MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS", Some("30"), Number(1, u32::MAX as i64)),
    ("MELODEE__INGEST__SETTLING_QUIET_SECONDS", Some("10"), Number(1, u32::MAX as i64)),
    ("MELODEE__INGEST__SOURCES_JSON", None, Host),
    ("MELODEE__INGEST__SOURCE_LIBRARY_ID", None, Host),
    ("MELODEE__INGEST__SOURCE_LIBRARY_NAME", None, Host),
    ("MELODEE__INGEST__SOURCE_ROOT", None, Host),
    ("MELODEE__INGEST__STAGING_ROOT", None, Host),
    ("MELODEE__INSPECTION__CONCURRENCY", Some("2"), Number(1, u32::MAX as i64)),
    ("MELODEE__INSPECTION__FFPROBE_PATH", Some("ffprobe"), Host),
    ("MELODEE__INSPECTION__FFPROBE_TIMEOUT_SECONDS", Some("30"), Number(1, u32::MAX as i64)),
    ("MELODEE__JELLYFIN__TOKEN_EXPIRES_AFTER_HOURS", Some("168"), Number(1, 8760)),
    ("MELODEE__LOGGING__DIRECTORY", None, Host),
    ("MELODEE__LOGGING__FORMAT", Some("text"), Choice(&["text", "json"])),
    ("MELODEE__LOGGING__LEVEL", Some("info"), Choice(&["trace", "debug", "info", "warn", "error"])),
    ("MELODEE__NATIVE__AUTH__DEVELOPMENT_RESET_TOKENS", Some("false"), Host),
    ("MELODEE__NATIVE__AUTH__GOOGLE__CLIENT_ID", None, Host),
    ("MELODEE__NATIVE__AUTH__GOOGLE__ENABLED", Some("false"), Host),
    ("MELODEE__NATIVE__AUTH__GOOGLE__HOSTED_DOMAINS", None, Host),
    ("MELODEE__NATIVE__AUTH__GOOGLE__JWKS_URL", Some("https://www.googleapis.com/oauth2/v3/certs"), Host),
    ("MELODEE__NATIVE__AUTH__JWT_KEY", None, Host),
    ("MELODEE__NATIVE__AUTH__MAX_SESSION_DAYS", Some("90"), Number(30, 365)),
    ("MELODEE__NATIVE__AUTH__RATE_ATTEMPTS", Some("10"), Number(1, 10000)),
    ("MELODEE__NATIVE__AUTH__RATE_MAX_KEYS", Some("100000"), Number(1, 10000000)),
    ("MELODEE__NATIVE__AUTH__RATE_WINDOW_SECONDS", Some("60"), Number(1, 86400)),
    ("MELODEE__NATIVE__CORS__ALLOWED_ORIGINS", None, Host),
    ("MELODEE__NATIVE__PUBLIC_ORIGIN", None, Host),
    ("MELODEE__NATIVE__STREAM_CONCURRENCY", Some("32"), Number(1, 4096)),
    ("MELODEE__NATIVE__TRUSTED_PROXIES", None, Host),
    ("MELODEE__NORMALIZATION__CONCURRENCY", Some("2"), Number(1, u32::MAX as i64)),
    ("MELODEE__NORMALIZATION__FFMPEG_PATH", Some("ffmpeg"), Host),
    ("MELODEE__NORMALIZATION__FFMPEG_TIMEOUT_SECONDS", Some("300"), Number(1, u32::MAX as i64)),
    ("MELODEE__NORMALIZATION__FPCALC_PATH", Some("fpcalc"), Host),
    ("MELODEE__NORMALIZATION__FPCALC_TIMEOUT_SECONDS", Some("60"), Number(1, u32::MAX as i64)),
    ("MELODEE__NORMALIZATION__WORK_AREA", Some("/tmp/melodee-work"), Host),
    ("MELODEE__OPENSUBSONIC__BASE_URL_HOST", None, Host),
    ("MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS", None, Host),
    ("MELODEE__OPENSUBSONIC__DOWNLOADS_ENABLED", Some("true"), Flag),
    ("MELODEE__OPENSUBSONIC__JUKEBOX_ENABLED", Some("false"), Flag),
    ("MELODEE__OPENSUBSONIC__LEGACY_AUTH_BYPASS", Some("false"), Host),
    ("MELODEE__OPENSUBSONIC__PODCASTS_ENABLED", None, Flag),
    ("MELODEE__OPENSUBSONIC__STORAGE_LIBRARY_ID", Some("6b1c9f4a-2c3e-5f0a-9d8b-7c6a5e4d3f2a"), Host),
    ("MELODEE__PODCAST__DISCOVERY_ENDPOINT", Some("https://itunes.apple.com/"), Host),
    ("MELODEE__PODCAST__ENABLED", Some("true"), Flag),
    ("MELODEE__PODCAST__MAX_DOWNLOAD_BYTES", Some("536870912"), Number(1048576, 4294967296)),
    ("MELODEE__PODCAST__STORAGE_ROOT", None, Host),
    ("MELODEE__PODCAST__WORKER__BATCH_SIZE", Some("4"), Number(1, 32)),
    ("MELODEE__PODCAST__WORKER__ENABLED", Some("false"), Flag),
    ("MELODEE__PODCAST__WORKER__LEASE_SECONDS", Some("300"), Number(60, 3600)),
    ("MELODEE__PODCAST__WORKER__MAX_ATTEMPTS", Some("5"), Number(1, 100)),
    ("MELODEE__PODCAST__WORKER__POLL_INTERVAL_SECONDS", Some("30"), Number(1, 86400)),
    ("MELODEE__POLICY__BATCH_SIZE", Some("500"), Number(1, 10000)),
    ("MELODEE__POLICY__CONCURRENCY", Some("4"), Number(1, 64)),
    ("MELODEE__POLICY__DESTRUCTIVE_CONFIDENCE_BPS", Some("8000"), Number(1, 10000)),
    ("MELODEE__POLICY__NOVEL_GREATEST_HITS_DISPOSITION", Some("review"), Choice(&["review", "accept"])),
    ("MELODEE__POLICY__REDUNDANT_GREATEST_HITS_DISPOSITION", Some("ignore"), Choice(&["ignore", "delete"])),
    ("MELODEE__POLICY__RULE_SET_VERSION", Some("2"), Number(2, 2147483647)),
    ("MELODEE__POLICY__UNCERTAIN_GREATEST_HITS_DISPOSITION", Some("review"), Choice(&["review", "quarantine"])),
    ("MELODEE__POLICY__WORKER__ENABLED", Some("true"), Flag),
    ("MELODEE__POLICY__WORKER__POLL_INTERVAL_SECONDS", Some("60"), Number(1, u32::MAX as i64)),
    ("MELODEE__PUBLICATION__CONCURRENCY", Some("2"), Number(1, u32::MAX as i64)),
    ("MELODEE__PUBLICATION__NFS_MIB_PER_SECOND", Some("8"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__BATCH_SIZE", Some("256"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__DEFAULT_LIMIT", Some("200"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__ENABLED", Some("false"), Flag),
    ("MELODEE__SEARCH__LEASE_SECONDS", Some("60"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__MAX_LIMIT", Some("1000"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS", Some("5"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__POLL_INTERVAL_MS", Some("1000"), Number(1, u32::MAX as i64)),
    ("MELODEE__SEARCH__ROOT", None, Host),
    ("MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE", Some("1000"), Number(1, u32::MAX as i64)),
    ("MELODEE__SERVER__BIND", Some("127.0.0.1:8080"), Host),
    ("MELODEE__SERVER__TRUSTED_PROXIES", None, Host),
    ("MELODEE__SHUTDOWN__GRACE_SECONDS", Some("30"), Number(1, 600)),
    ("MELODEE__STORAGE__ROOT", None, Host),
    ("MELODEE__VALIDATION__CONCURRENCY", Some("2"), Number(1, u32::MAX as i64)),
    ("MELODEE__WORK__CLAIM_BATCH_SIZE", Some("4"), Number(1, u32::MAX as i64)),
    ("MELODEE__WORK__LEASE_SECONDS", Some("300"), Number(1, u32::MAX as i64)),
    ("MELODEE__WORK__MAX_ATTEMPTS", Some("5"), Number(1, u32::MAX as i64)),
    ("MELODEE__WORK__RETRY_BASE_SECONDS", Some("60"), Number(1, u32::MAX as i64)),
    ("MELODEE__WORK__RETRY_MAX_SECONDS", Some("3600"), Number(1, u32::MAX as i64)),
];

/// This existing registry name and the native startup key control the same feature.
pub const PODCAST_ENABLED: &str = "MELODEE__PODCAST__ENABLED";
pub const PODCAST_REGISTRY_KEY: &str = "podcast.enabled";

pub fn definition(key: &str) -> Option<&'static RuntimeDefinition> {
    DEFINITIONS.iter().find(|entry| entry.0 == key)
}

impl Control {
    pub fn accepts(self, value: &str) -> bool {
        match self {
            Host => false,
            Flag => matches!(value, "true" | "false"),
            Number(min, max) => value.parse::<i64>().is_ok_and(|n| (min..=max).contains(&n)),
            Choice(choices) => choices.contains(&value),
            Marker => {
                !value.is_empty()
                    && value.len() <= 255
                    && value != "."
                    && value != ".."
                    && !value.contains(['/', '\\'])
                    && !value.chars().any(char::is_control)
            }
        }
    }
}

pub type StartupValues = BTreeMap<String, String>;
static ACTIVE: OnceLock<StartupValues> = OnceLock::new();
thread_local! {
    static PREVIEW: RefCell<Option<StartupValues>> = const { RefCell::new(None) };
}

pub fn active() -> Option<&'static StartupValues> {
    ACTIVE.get()
}

/// Standard config parsers use the same reader when validating a candidate and on startup.
pub fn var(key: &str) -> Result<String, std::env::VarError> {
    PREVIEW.with_borrow(|candidate| {
        candidate
            .as_ref()
            .or(ACTIVE.get())
            .and_then(|m| m.get(key))
            .cloned()
            .map_or_else(|| std::env::var(key), Ok)
    })
}

/// Scope an override snapshot to a synchronous validation call. It cannot reach other requests.
pub fn preview<T>(values: StartupValues, check: impl FnOnce() -> T) -> T {
    struct Restore(Option<StartupValues>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PREVIEW.replace(self.0.take());
        }
    }
    let _restore = Restore(PREVIEW.replace(Some(values)));
    check()
}

pub async fn load<'e>(db: impl sqlx::PgExecutor<'e>) -> Result<StartupValues, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM server_startup_overrides ORDER BY key",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect())
}

pub async fn initialize(pool: &PgPool) -> Result<(), crate::SettingsError> {
    let values = load(pool).await?;
    for (key, v) in &values {
        if !definition(key).is_some_and(|(_, _, kind)| kind.accepts(v)) {
            return Err(crate::SettingsError::InvalidValue {
                key: key.clone(),
                reason: "invalid stored startup override".into(),
            });
        }
    }
    ACTIVE
        .set(values)
        .map_err(|_| crate::SettingsError::InvalidDocument("startup settings initialized twice"))
}

pub fn host_registry_key(key: &str) -> bool {
    matches!(
        key,
        "jellyfin.token.pepper"
            | "security.secretKey"
            | "encryption.privateKey"
            | "mpv.path"
            | "mpv.socketPath"
            | "theme.libraryPath"
            | "searchEngine.musicbrainz.storagePath"
            | "system.baseUrl"
    ) || key.starts_with("transcoding.command.")
}

pub fn text_value(value: &Value) -> Option<String> {
    match value {
        Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A custom database key cannot shadow a canonical variable or a registered setting.
pub fn reserved_alias(key: &str) -> bool {
    if crate::known_setting(key).is_some() {
        return false;
    }
    let name = crate::env_var_name(key);
    key.to_ascii_uppercase().starts_with("MELODEE__")
        || definition(&name).is_some()
        || crate::KNOWN_SETTINGS
            .iter()
            .any(|k| crate::env_var_name(k.key) == name)
        || name == "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_inventory_covers_documented_and_example_server_configuration() {
        use std::collections::BTreeSet;
        let mut names: BTreeSet<_> = crate::KNOWN_SETTINGS
            .iter()
            .map(|setting| crate::env_var_name(setting.key))
            .collect();
        assert!(
            names.remove(PODCAST_ENABLED),
            "podcast startup alias must be registered"
        );
        for &(key, default, control) in DEFINITIONS {
            assert!(
                names.insert(key.to_owned()),
                "duplicate startup setting {key}"
            );
            if let Some(value) = default
                && !matches!(control, Control::Host)
            {
                assert!(control.accepts(value), "invalid default for {key}");
            }
        }
        let docs = include_str!("../../../docs/src/content/docs/installation/configuration.md");
        let example = include_str!("../../../.env.example");
        for key in docs
            .lines()
            .filter_map(|line| {
                line.strip_prefix("| `")?
                    .split_once("` |")
                    .map(|(key, _)| key)
            })
            .chain(example.lines().filter_map(|line| {
                line.trim_start_matches('#')
                    .trim()
                    .split_once('=')
                    .map(|(key, _)| key)
            }))
            .filter(|key| key.starts_with("MELODEE__"))
        {
            assert!(
                names.contains(key),
                "missing Admin UI classification for {key}"
            );
        }
    }
}
