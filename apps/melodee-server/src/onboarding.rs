//! Phase 9 WP-08 first-run onboarding preflight (PRD-SETUP-001/002,
//! ADR-0134 §3).
//!
//! [`evaluate_onboarding`] runs six named checks, each reported with
//! pass/fail and a remediation hint:
//!
//! 1. `database` (**required**) — PostgreSQL is reachable and compiled
//!    migrations are fully applied with no recorded failure.
//! 2. `storage_paths` — configured inbound source roots exist and are
//!    readable, and the canonical storage root (when set) is writable. An
//!    unconfigured server fails this check with configuration guidance;
//!    advisory, because media paths can also be configured after first run.
//! 3. `media_tools` — the configured `ffprobe`/`ffmpeg` executables answer
//!    `-version`. Advisory (conversion/normalization degrade, browsing does
//!    not).
//! 4. `search_index` — when the search worker is enabled, its root is
//!    configured and ready (existing directory writable / creatable under a
//!    writable parent) without creating or claiming anything. Disabled
//!    search passes vacuously. Advisory.
//! 5. `admin_user` (**required**) — at least one enabled, unlocked
//!    administrator exists (`melodee-cli users bootstrap-admin` creates the
//!    first one).
//! 6. `email` — layered email settings validate (enabled ⇒ from/host set).
//!    Advisory (delivery disabled is a valid steady state).
//!
//! Completion ([`complete_onboarding`]) fails closed while any required
//! check fails and writes the singleton `onboarding_state` row with the
//! check evidence (never secret material).
//!
//! # Migration bypass (PRD-SETUP-002)
//!
//! On every status evaluation the report carries `bypassed: true` when the
//! database shows evidence of an existing (migrated) installation, defined
//! exactly as:
//!
//! - `onboarding_state.completed_at` is already set, OR
//! - the catalog is non-empty (`EXISTS (SELECT 1 FROM releases)` or
//!   `EXISTS (SELECT 1 FROM artists)`), OR
//! - `server_settings` holds at least one known registry key (a legacy
//!   `Settings` import lands as such rows in Phase 11).
//!
//! A bypassed server never requires onboarding completion; completion
//! remains allowed (idempotent marker rewrite).

use melodee_ingestion::IngestionConfig;
use melodee_persistence::{
    KNOWN_SETTINGS, OnboardingState, OnboardingStore, SettingsError, SettingsResolver,
    migration_status,
};
use melodee_search::SearchConfig;
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;

/// One named preflight check result.
#[derive(Clone, Debug, PartialEq)]
pub struct PreflightCheck {
    pub name: &'static str,
    /// Required checks gate onboarding completion; advisory checks report
    /// honestly without blocking it.
    pub required: bool,
    pub passed: bool,
    pub detail: String,
    pub remediation: Option<String>,
}

impl PreflightCheck {
    fn pass(name: &'static str, required: bool, detail: impl Into<String>) -> Self {
        Self {
            name,
            required,
            passed: true,
            detail: detail.into(),
            remediation: None,
        }
    }

    fn fail(
        name: &'static str,
        required: bool,
        detail: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            name,
            required,
            passed: false,
            detail: detail.into(),
            remediation: Some(remediation.into()),
        }
    }
}

/// The full onboarding evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct OnboardingReport {
    pub checks: Vec<PreflightCheck>,
    pub completed: bool,
    /// Migration-bypass evidence (see module docs for the exact heuristic).
    pub bypassed: bool,
    /// All required checks pass.
    pub can_complete: bool,
}

#[derive(Debug, Error)]
pub enum OnboardingError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("settings store error")]
    Settings(#[from] SettingsError),
    #[error("onboarding cannot complete while required preflight checks fail")]
    ChecksFailed(Box<OnboardingReport>),
}

/// Probes whether `directory` is writable by creating and removing a
/// uniquely named probe file. Never touches existing content.
/// Readiness verdict for a runtime root the server creates on demand: pass
/// when the root exists and is writable, or when it is creatable under a
/// writable existing ancestor. Nothing is created or removed beyond the
/// write probe itself.
fn writable_or_creatable(root: &std::path::Path) -> Result<String, String> {
    if root.is_dir() {
        return if directory_is_writable(root) {
            Ok(format!("{} exists and is writable", root.display()))
        } else {
            Err(format!("{} is not writable", root.display()))
        };
    }
    let mut ancestor = root.parent();
    while let Some(candidate) = ancestor {
        if candidate.is_dir() {
            return if directory_is_writable(candidate) {
                Ok(format!(
                    "{} is creatable under {}",
                    root.display(),
                    candidate.display()
                ))
            } else {
                Err(format!(
                    "{} is not creatable: {} is not writable",
                    root.display(),
                    candidate.display()
                ))
            };
        }
        ancestor = candidate.parent();
    }
    Err(format!("{} has no existing ancestor", root.display()))
}

fn directory_is_writable(directory: &std::path::Path) -> bool {
    let probe = directory.join(format!(
        ".melodee-write-probe-{}",
        uuid::Uuid::now_v7().simple()
    ));
    let writable = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if writable {
        let _ = std::fs::remove_file(&probe);
    }
    writable
}

pub(crate) async fn check_database(pool: &PgPool) -> PreflightCheck {
    match migration_status(pool).await {
        Ok(status) if status.is_current() => PreflightCheck::pass(
            "database",
            true,
            format!(
                "postgresql reachable; migrations current (applied={}, latest={})",
                status.applied, status.latest_available
            ),
        ),
        Ok(status) => PreflightCheck::fail(
            "database",
            true,
            format!(
                "migrations not current (applied={}, latest={}, all_successful={})",
                status.applied, status.latest_available, status.all_successful
            ),
            "run `melodee migrate` with the current server binary",
        ),
        Err(error) => PreflightCheck::fail(
            "database",
            true,
            format!("postgresql query failed: {error}"),
            "verify MELODEE__DATABASE__URL and that PostgreSQL is running",
        ),
    }
}

pub(crate) fn check_storage_paths() -> PreflightCheck {
    let config = match IngestionConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            return PreflightCheck::fail(
                "storage_paths",
                false,
                format!("invalid ingestion configuration: {error}"),
                "fix the MELODEE__INGEST__* / MELODEE__STORAGE__ROOT environment values",
            );
        }
    };
    let mut problems = Vec::new();
    for source in &config.sources {
        if std::fs::read_dir(&source.root).is_err() {
            problems.push(format!(
                "inbound source {} does not exist or is not readable",
                source.root.display()
            ));
        }
    }
    if config.enabled() {
        match &config.staging_root {
            Some(root) if !root.is_dir() => problems.push(format!(
                "staging root {} is not a directory",
                root.display()
            )),
            Some(root) if !directory_is_writable(root) => {
                problems.push(format!("staging root {} is not writable", root.display()))
            }
            None => problems.push(
                "MELODEE__INGEST__STAGING_ROOT is required while inbound ingestion is enabled"
                    .to_owned(),
            ),
            _ => {}
        }
    }
    // The normalization work area is disposable and created on demand; like
    // the search root, it passes when it exists and is writable or when it is
    // creatable under a writable ancestor.
    if let Err(problem) = writable_or_creatable(&config.work_area_base) {
        problems.push(format!("normalization work area {problem}"));
    }
    match &config.storage_root {
        Some(root) if !root.is_dir() => {
            problems.push(format!(
                "storage root {} is not a directory",
                root.display()
            ));
        }
        Some(root) if !directory_is_writable(root) => {
            problems.push(format!("storage root {} is not writable", root.display()));
        }
        _ => {}
    }
    if !problems.is_empty() {
        return PreflightCheck::fail(
            "storage_paths",
            false,
            problems.join("; "),
            "create the directories with read/write/rename permission for inbound and write permission for staging, work, and storage",
        );
    }
    if config.sources.is_empty() && config.storage_root.is_none() {
        return PreflightCheck::fail(
            "storage_paths",
            false,
            "no inbound source or storage root configured",
            "set MELODEE__INGEST__SOURCES_JSON (or MELODEE__INGEST__SOURCE_ROOT), MELODEE__INGEST__STAGING_ROOT, and MELODEE__STORAGE__ROOT",
        );
    }
    // ADR-0145 diagnostics: report each storage role separately so operators
    // see the transient inbox, durable staging, disposable work area, and
    // canonical library as distinct roots.
    PreflightCheck::pass(
        "storage_paths",
        false,
        format!(
            "inbound: {} source(s) [{}]; staging: {}; work: {}; canonical: {}",
            config.sources.len(),
            config
                .sources
                .iter()
                .map(|source| source.root.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            config
                .staging_root
                .as_ref()
                .map_or("not set".to_string(), |root| root.display().to_string()),
            config.work_area_base.display(),
            config
                .storage_root
                .as_ref()
                .map_or("not set".to_string(), |root| root.display().to_string())
        ),
    )
}

/// Probes one executable with `-version`, bounded so a hung tool cannot
/// stall onboarding. Reuses the paths the ingestion pipeline itself runs.
/// Spawning is blocking std `Command` (the workspace tokio build has no
/// `process` feature), so a timed-out probe leaks its blocking thread until
/// the child exits — acceptable for a `-version` probe.
async fn probe_tool(path: &std::path::Path) -> bool {
    let path = path.to_path_buf();
    let probe = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&path)
            .arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    });
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), probe).await,
        Ok(Ok(Ok(status))) if status.success()
    )
}

pub(crate) async fn check_media_tools() -> PreflightCheck {
    let config = match IngestionConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            return PreflightCheck::fail(
                "media_tools",
                false,
                format!("invalid ingestion configuration: {error}"),
                "fix the MELODEE__INGEST__* environment values",
            );
        }
    };
    let ffprobe = probe_tool(&config.ffprobe_path).await;
    let ffmpeg = probe_tool(&config.ffmpeg_path).await;
    if ffprobe && ffmpeg {
        PreflightCheck::pass(
            "media_tools",
            false,
            format!(
                "ffprobe ({}) and ffmpeg ({}) available",
                config.ffprobe_path.display(),
                config.ffmpeg_path.display()
            ),
        )
    } else {
        PreflightCheck::fail(
            "media_tools",
            false,
            format!(
                "ffprobe: {}, ffmpeg: {}",
                if ffprobe { "ok" } else { "missing" },
                if ffmpeg { "ok" } else { "missing" }
            ),
            "install ffmpeg/ffprobe or set MELODEE__INSPECTION__FFPROBE_PATH / MELODEE__NORMALIZATION__FFMPEG_PATH",
        )
    }
}

pub(crate) fn check_search_index() -> PreflightCheck {
    let config = match SearchConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            return PreflightCheck::fail(
                "search_index",
                false,
                format!("invalid search configuration: {error}"),
                "fix the MELODEE__SEARCH__* environment values",
            );
        }
    };
    if !config.enabled {
        return PreflightCheck::pass("search_index", false, "search worker disabled");
    }
    let root = config.search_root;
    if root.is_dir() {
        return if directory_is_writable(&root) {
            PreflightCheck::pass(
                "search_index",
                false,
                format!("search root {} exists and is writable", root.display()),
            )
        } else {
            PreflightCheck::fail(
                "search_index",
                false,
                format!("search root {} is not writable", root.display()),
                "grant the server user write permission on MELODEE__SEARCH__ROOT",
            )
        };
    }
    // Readiness without creating anything: the root must be creatable under
    // an existing writable ancestor (SearchRoot::open owns the creation).
    let mut ancestor = root.parent();
    while let Some(candidate) = ancestor {
        if candidate.is_dir() {
            return if directory_is_writable(candidate) {
                PreflightCheck::pass(
                    "search_index",
                    false,
                    format!(
                        "search root {} is creatable under {}",
                        root.display(),
                        candidate.display()
                    ),
                )
            } else {
                PreflightCheck::fail(
                    "search_index",
                    false,
                    format!(
                        "search root {} is not creatable: {} is not writable",
                        root.display(),
                        candidate.display()
                    ),
                    "create MELODEE__SEARCH__ROOT writable by the server user",
                )
            };
        }
        ancestor = candidate.parent();
    }
    PreflightCheck::fail(
        "search_index",
        false,
        format!("search root {} has no existing ancestor", root.display()),
        "create MELODEE__SEARCH__ROOT or point it at an existing filesystem location",
    )
}

async fn check_admin_user(pool: &PgPool) -> PreflightCheck {
    let exists: bool = match sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM native_users \
         WHERE is_admin AND enabled AND NOT locked)",
    )
    .fetch_one(pool)
    .await
    {
        Ok(exists) => exists,
        Err(error) => {
            return PreflightCheck::fail(
                "admin_user",
                true,
                format!("query failed: {error}"),
                "verify database connectivity",
            );
        }
    };
    if exists {
        PreflightCheck::pass("admin_user", true, "at least one administrator exists")
    } else {
        PreflightCheck::fail(
            "admin_user",
            true,
            "no enabled administrator account exists",
            "run `melodee-cli users bootstrap-admin --username <name> --email <email>` with MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD set",
        )
    }
}

async fn check_email(pool: &PgPool) -> PreflightCheck {
    let resolver = match SettingsResolver::from_env(pool) {
        Ok(resolver) => resolver,
        Err(error) => {
            return PreflightCheck::fail(
                "email",
                false,
                format!("settings resolution failed: {error}"),
                "fix MELODEE__CONFIG_FILE or the layered settings",
            );
        }
    };
    match crate::notifications::EmailSettings::from_resolved(&resolver).await {
        Ok(settings) if settings.enabled => PreflightCheck::pass(
            "email",
            false,
            format!(
                "smtp delivery enabled via {}:{}",
                settings.smtp_host, settings.smtp_port
            ),
        ),
        Ok(_) => PreflightCheck::pass(
            "email",
            false,
            "email delivery disabled (valid steady state)",
        ),
        Err(error) => PreflightCheck::fail(
            "email",
            false,
            format!("invalid email settings: {error}"),
            "set email.fromEmail/email.smtpHost (MELODEE__EMAIL__* or server_settings) or disable email.enabled",
        ),
    }
}

/// Migration-bypass evidence (module docs define the exact heuristic).
/// Errors are deliberately mapped to `false` per leg: the onboarding gate
/// must never report bypass on uncertain evidence (fail closed).
async fn has_migration_evidence(pool: &PgPool) -> bool {
    let catalog_non_empty: bool = match sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM releases) OR EXISTS (SELECT 1 FROM artists)",
    )
    .fetch_one(pool)
    .await
    {
        Ok(exists) => exists,
        Err(_) => return false,
    };
    if catalog_non_empty {
        return true;
    }
    let known_keys: Vec<&str> = KNOWN_SETTINGS.iter().map(|known| known.key).collect();
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM server_settings WHERE key = ANY($1))")
        .bind(&known_keys)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

/// Runs the full preflight and evaluates completion/bypass state.
pub async fn evaluate_onboarding(pool: &PgPool) -> OnboardingReport {
    let checks = vec![
        check_database(pool).await,
        check_storage_paths(),
        check_media_tools().await,
        check_search_index(),
        check_admin_user(pool).await,
        check_email(pool).await,
    ];
    let completed = OnboardingStore::new()
        .is_completed(pool)
        .await
        .unwrap_or(false);
    let bypassed = completed || has_migration_evidence(pool).await;
    let can_complete = checks.iter().all(|check| check.passed || !check.required);
    OnboardingReport {
        checks,
        completed,
        bypassed,
        can_complete,
    }
}

/// Completes onboarding: re-evaluates (never trust a stale status), fails
/// closed while any required check fails, then writes the singleton marker
/// with the check evidence. Secret values never enter the evidence.
pub async fn complete_onboarding(
    pool: &PgPool,
    completed_by: &str,
) -> Result<OnboardingState, OnboardingError> {
    let report = evaluate_onboarding(pool).await;
    if !report.can_complete {
        return Err(OnboardingError::ChecksFailed(Box::new(report)));
    }
    let evidence = serde_json::json!({
        "evaluatedAt": OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "bypassed": report.bypassed,
        "checks": report
            .checks
            .iter()
            .map(|check| serde_json::json!({
                "name": check.name,
                "required": check.required,
                "passed": check.passed,
                "detail": check.detail,
            }))
            .collect::<Vec<_>>(),
    });
    let state = OnboardingStore::new()
        .complete(pool, completed_by, evidence, OffsetDateTime::now_utc())
        .await?;
    Ok(state)
}
