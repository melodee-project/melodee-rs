//! Phase 9 WP-11 scripting hook persistence (ADR-0134 §6, ADR-0135,
//! PRD-SCRIPT-001..004) over `script_registry` and `script_runs`
//! (migration 0027).
//!
//! The registry holds operator-installed JavaScript hook bodies (one row per
//! (event, name)); nothing is seeded — scripts are operator trust-boundary
//! content (ADR-0135). `version` bumps only when the stored source changes,
//! so operators can distinguish real script churn from metadata re-saves.
//! Upserts never touch `enabled`: a circuit-breaker-disabled or deliberately
//! disabled script stays off until an explicit enable.
//!
//! `script_runs` is an **append-only execution audit**: one row per
//! evaluation, never updated, purged only by bounded retention
//! ([`ScriptRunStore::purge_older_than`]). The circuit breaker reads the most
//! recent outcomes per script ([`ScriptRunStore::count_recent_failures`]);
//! tripping a script (audit row + registry disable) is a single transaction
//! ([`ScriptRunStore::record_trip_and_disable`]).

use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Bounded event name length, mirroring the migration CHECK.
pub const MAX_SCRIPT_EVENT_CHARS: usize = 100;
/// Bounded script name length, mirroring the migration CHECK.
pub const MAX_SCRIPT_NAME_CHARS: usize = 255;
/// Serialized-size bound for script bodies, mirroring the migration CHECK.
pub const MAX_SCRIPT_SOURCE_BYTES: usize = 256 * 1024;
/// Bounded run-error length, mirroring the migration CHECK. Longer engine
/// errors are truncated on a char boundary before insert.
pub const MAX_RUN_ERROR_CHARS: usize = 2000;
/// ADR-0135 §3 hard ceiling on the per-execution wall-clock budget.
pub const MAX_TIMEOUT_MS: i32 = 10_000;
/// Sanity bounds for the advisory memory budget (not enforceable with Boa;
/// ADR-0135 §3), mirroring the migration CHECK.
pub const MIN_MEMORY_BYTES: i64 = 4096;
pub const MAX_MEMORY_BYTES: i64 = 64 * 1024 * 1024;
/// Upper bound for registry list responses; the API layer clamps to this.
pub const MAX_SCRIPT_LIST: u32 = 500;
/// Upper bound for run list responses; the API layer clamps to this.
pub const MAX_RUN_LIST: u32 = 500;

type RegistryRow = (
    Uuid,
    String,
    String,
    String,
    bool,
    i32,
    i64,
    i32,
    OffsetDateTime,
    OffsetDateTime,
);

type RunRow = (
    Uuid,
    Option<Uuid>,
    String,
    String,
    i64,
    Option<String>,
    OffsetDateTime,
);

/// One stored script row.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredScript {
    pub id: Uuid,
    pub event: String,
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub timeout_ms: i32,
    pub memory_bytes: i64,
    pub version: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl From<RegistryRow> for RegisteredScript {
    fn from(row: RegistryRow) -> Self {
        let (
            id,
            event,
            name,
            source,
            enabled,
            timeout_ms,
            memory_bytes,
            version,
            created_at,
            updated_at,
        ) = row;
        Self {
            id,
            event,
            name,
            source,
            enabled,
            timeout_ms,
            memory_bytes,
            version,
            created_at,
            updated_at,
        }
    }
}

/// Upsert payload. On conflict the stored row's source and limits are
/// replaced and `version` bumps only when the source changed; `enabled` is
/// deliberately untouched (see module docs).
pub struct ScriptUpsert<'a> {
    pub event: &'a str,
    pub name: &'a str,
    pub source: &'a str,
    pub timeout_ms: i32,
    pub memory_bytes: i64,
}

/// Execution outcome recorded in the audit. `Allow`/`Deny` are real script
/// verdicts; `Error`/`Timeout` are fail-degraded defaults (the host operation
/// proceeded as allow); `Tripped` records the circuit breaker disabling a
/// script. The `script_runs` CHECK is the fail-closed backstop for this
/// vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptRunOutcome {
    Allow,
    Deny,
    Error,
    Timeout,
    Tripped,
}

impl ScriptRunOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Tripped => "tripped",
        }
    }

    /// Whether the outcome counts toward the circuit breaker's
    /// consecutive-failure window.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Error | Self::Timeout)
    }
}

/// One append-only execution audit row.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptRun {
    pub id: Uuid,
    pub script_id: Option<Uuid>,
    pub event: String,
    pub outcome: String,
    pub duration_ms: i64,
    pub error: Option<String>,
    pub created_at: OffsetDateTime,
}

impl From<RunRow> for ScriptRun {
    fn from(row: RunRow) -> Self {
        let (id, script_id, event, outcome, duration_ms, error, created_at) = row;
        Self {
            id,
            script_id,
            event,
            outcome,
            duration_ms,
            error,
            created_at,
        }
    }
}

/// Audit insert payload. `error` is truncated to the migration bound on a
/// char boundary so engine messages always fit.
pub struct NewScriptRun<'a> {
    pub script_id: Option<Uuid>,
    pub event: &'a str,
    pub outcome: ScriptRunOutcome,
    pub duration_ms: i64,
    pub error: Option<&'a str>,
}

#[derive(Debug, Error)]
pub enum ScriptStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("script not found")]
    ScriptNotFound,
    #[error("invalid script: {0}")]
    InvalidScript(&'static str),
    #[error("invalid script run: {0}")]
    InvalidRun(&'static str),
}

fn validate_event(event: &str) -> Result<(), ScriptStoreError> {
    if event.trim().is_empty() {
        return Err(ScriptStoreError::InvalidScript("event must be non-blank"));
    }
    if event.chars().count() > MAX_SCRIPT_EVENT_CHARS {
        return Err(ScriptStoreError::InvalidScript(
            "event exceeds 100 characters",
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ScriptStoreError> {
    if name.trim().is_empty() {
        return Err(ScriptStoreError::InvalidScript("name must be non-blank"));
    }
    if name.chars().count() > MAX_SCRIPT_NAME_CHARS {
        return Err(ScriptStoreError::InvalidScript(
            "name exceeds 255 characters",
        ));
    }
    // Path-hostile characters are excluded so admin routes keyed by
    // (event, name) stay unambiguous (mirrors the migration CHECK).
    if name.chars().any(|c| c == '/' || c.is_control()) {
        return Err(ScriptStoreError::InvalidScript(
            "name must not contain '/' or control characters",
        ));
    }
    Ok(())
}

fn validate_source(source: &str) -> Result<(), ScriptStoreError> {
    if source.trim().is_empty() {
        return Err(ScriptStoreError::InvalidScript("source must be non-blank"));
    }
    if source.len() > MAX_SCRIPT_SOURCE_BYTES {
        return Err(ScriptStoreError::InvalidScript(
            "source exceeds 262144 bytes",
        ));
    }
    Ok(())
}

fn validate_limits(timeout_ms: i32, memory_bytes: i64) -> Result<(), ScriptStoreError> {
    if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(ScriptStoreError::InvalidScript(
            "timeout_ms must be within 1..=10000 (ADR-0135 ceiling)",
        ));
    }
    if !(MIN_MEMORY_BYTES..=MAX_MEMORY_BYTES).contains(&memory_bytes) {
        return Err(ScriptStoreError::InvalidScript(
            "memory_bytes must be within 4096..=67108864",
        ));
    }
    Ok(())
}

/// Truncates on a char boundary so recorded engine errors always fit the
/// audit column.
fn truncate_error(error: &str) -> String {
    if error.chars().count() <= MAX_RUN_ERROR_CHARS {
        return error.to_string();
    }
    // Reserve one character for the truncation marker so the stored value
    // always fits the CHECK bound.
    let mut truncated: String = error.chars().take(MAX_RUN_ERROR_CHARS - 1).collect();
    truncated.push('…');
    truncated
}

/// Stateless entry point for script registry administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptRegistryStore;

impl ScriptRegistryStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Bounded, deterministic listing: event, then name.
    pub async fn list(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RegisteredScript>, ScriptStoreError> {
        let rows = sqlx::query_as::<_, RegistryRow>(
            "SELECT id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at \
             FROM script_registry ORDER BY event, name LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit.min(MAX_SCRIPT_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(RegisteredScript::from).collect())
    }

    pub async fn get(&self, pool: &PgPool, id: Uuid) -> Result<RegisteredScript, ScriptStoreError> {
        let row = sqlx::query_as::<_, RegistryRow>(
            "SELECT id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at \
             FROM script_registry WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        row.map(RegisteredScript::from)
            .ok_or(ScriptStoreError::ScriptNotFound)
    }

    pub async fn get_by_name(
        &self,
        pool: &PgPool,
        event: &str,
        name: &str,
    ) -> Result<RegisteredScript, ScriptStoreError> {
        let row = sqlx::query_as::<_, RegistryRow>(
            "SELECT id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at \
             FROM script_registry WHERE event=$1 AND name=$2",
        )
        .bind(event)
        .bind(name)
        .fetch_optional(pool)
        .await?;
        row.map(RegisteredScript::from)
            .ok_or(ScriptStoreError::ScriptNotFound)
    }

    /// The enabled scripts for one event, in deterministic evaluation order
    /// (name). This is the hook gate's per-candidate lookup.
    pub async fn list_enabled_for_event(
        &self,
        pool: &PgPool,
        event: &str,
    ) -> Result<Vec<RegisteredScript>, ScriptStoreError> {
        let rows = sqlx::query_as::<_, RegistryRow>(
            "SELECT id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at \
             FROM script_registry WHERE event=$1 AND enabled ORDER BY name",
        )
        .bind(event)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(RegisteredScript::from).collect())
    }

    /// Creates or replaces one (event, name) script. `version` bumps only
    /// when the source changed; `enabled` is untouched on conflict so a
    /// circuit-breaker-disabled script is never silently re-enabled by a
    /// re-save.
    pub async fn upsert(
        &self,
        pool: &PgPool,
        upsert: &ScriptUpsert<'_>,
        now: OffsetDateTime,
    ) -> Result<RegisteredScript, ScriptStoreError> {
        validate_event(upsert.event)?;
        validate_name(upsert.name)?;
        validate_source(upsert.source)?;
        validate_limits(upsert.timeout_ms, upsert.memory_bytes)?;
        let row = sqlx::query_as::<_, RegistryRow>(
            "INSERT INTO script_registry \
               (id,event,name,source,timeout_ms,memory_bytes,version,created_at,updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,1,$7,$7) \
             ON CONFLICT (event, name) DO UPDATE SET \
               source = EXCLUDED.source, timeout_ms = EXCLUDED.timeout_ms, \
               memory_bytes = EXCLUDED.memory_bytes, \
               version = script_registry.version \
                 + CASE WHEN script_registry.source <> EXCLUDED.source THEN 1 ELSE 0 END, \
               updated_at = EXCLUDED.updated_at \
             RETURNING id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at",
        )
        .bind(Uuid::now_v7())
        .bind(upsert.event)
        .bind(upsert.name)
        .bind(upsert.source)
        .bind(upsert.timeout_ms)
        .bind(upsert.memory_bytes)
        .bind(now)
        .fetch_one(pool)
        .await?;
        Ok(RegisteredScript::from(row))
    }

    /// Explicit enable/disable. Disabled scripts are skipped by the hook
    /// gate; a tripped script requires this explicit operator action to run
    /// again.
    pub async fn set_enabled(
        &self,
        pool: &PgPool,
        id: Uuid,
        enabled: bool,
        now: OffsetDateTime,
    ) -> Result<RegisteredScript, ScriptStoreError> {
        let row = sqlx::query_as::<_, RegistryRow>(
            "UPDATE script_registry SET enabled=$2, updated_at=$3 WHERE id=$1 \
             RETURNING id,event,name,source,enabled,timeout_ms,memory_bytes,version,created_at,updated_at",
        )
        .bind(id)
        .bind(enabled)
        .bind(now)
        .fetch_optional(pool)
        .await?;
        row.map(RegisteredScript::from)
            .ok_or(ScriptStoreError::ScriptNotFound)
    }

    /// Deletes one script. Past audit rows survive with `script_id` NULL
    /// (ON DELETE SET NULL).
    pub async fn delete(&self, pool: &PgPool, id: Uuid) -> Result<(), ScriptStoreError> {
        let result = sqlx::query("DELETE FROM script_registry WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ScriptStoreError::ScriptNotFound);
        }
        Ok(())
    }
}

/// Stateless entry point for the append-only execution audit.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptRunStore;

impl ScriptRunStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Appends one execution audit row. A hook evaluation never rolls back
    /// the operation that triggered it (ADR-0135 §4), so callers record after
    /// the fact.
    pub async fn record_run(
        &self,
        pool: &PgPool,
        run: &NewScriptRun<'_>,
        now: OffsetDateTime,
    ) -> Result<ScriptRun, ScriptStoreError> {
        validate_event(run.event).map_err(|error| match error {
            ScriptStoreError::InvalidScript(message) => ScriptStoreError::InvalidRun(message),
            other => other,
        })?;
        if run.duration_ms < 0 {
            return Err(ScriptStoreError::InvalidRun("duration_ms must be >= 0"));
        }
        let error = run.error.map(truncate_error);
        let row = sqlx::query_as::<_, RunRow>(
            "INSERT INTO script_runs \
               (id,script_id,event,outcome,duration_ms,error,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             RETURNING id,script_id,event,outcome,duration_ms,error,created_at",
        )
        .bind(Uuid::now_v7())
        .bind(run.script_id)
        .bind(run.event)
        .bind(run.outcome.as_str())
        .bind(run.duration_ms)
        .bind(error)
        .bind(now)
        .fetch_one(pool)
        .await?;
        Ok(ScriptRun::from(row))
    }

    /// Bounded listing, newest first; optionally scoped to one script.
    pub async fn list_recent(
        &self,
        pool: &PgPool,
        script_id: Option<Uuid>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ScriptRun>, ScriptStoreError> {
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id,script_id,event,outcome,duration_ms,error,created_at \
             FROM script_runs WHERE ($1::uuid IS NULL OR script_id=$1) \
             ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
        )
        .bind(script_id)
        .bind(i64::from(limit.min(MAX_RUN_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(ScriptRun::from).collect())
    }

    /// Counts failure outcomes (`error`, `timeout`) among the script's
    /// `window` most recent runs. The circuit breaker trips when this equals
    /// the window, i.e. every recent run failed.
    pub async fn count_recent_failures(
        &self,
        pool: &PgPool,
        script_id: Uuid,
        window: u32,
    ) -> Result<u32, ScriptStoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ( \
               SELECT outcome FROM script_runs WHERE script_id=$1 \
               ORDER BY created_at DESC, id DESC LIMIT $2 \
             ) recent WHERE outcome IN ('error', 'timeout')",
        )
        .bind(script_id)
        .bind(i64::from(window))
        .fetch_one(pool)
        .await?;
        u32::try_from(count).map_err(|_| {
            ScriptStoreError::Database(sqlx::Error::Decode(
                "recent failure count exceeds u32".into(),
            ))
        })
    }

    /// Circuit-breaker trip (ADR-0135 §3): records the `tripped` audit row
    /// and disables the script in one transaction, so the audit never shows
    /// a trip without the disable (or vice versa).
    pub async fn record_trip_and_disable(
        &self,
        pool: &PgPool,
        run: &NewScriptRun<'_>,
        now: OffsetDateTime,
    ) -> Result<ScriptRun, ScriptStoreError> {
        let Some(script_id) = run.script_id else {
            return Err(ScriptStoreError::InvalidRun(
                "a tripped run must reference a registry script",
            ));
        };
        if run.outcome != ScriptRunOutcome::Tripped {
            return Err(ScriptStoreError::InvalidRun(
                "record_trip_and_disable requires the tripped outcome",
            ));
        }
        validate_event(run.event).map_err(|error| match error {
            ScriptStoreError::InvalidScript(message) => ScriptStoreError::InvalidRun(message),
            other => other,
        })?;
        let mut tx = pool.begin().await?;
        let error = run.error.map(truncate_error);
        let row = sqlx::query_as::<_, RunRow>(
            "INSERT INTO script_runs \
               (id,script_id,event,outcome,duration_ms,error,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             RETURNING id,script_id,event,outcome,duration_ms,error,created_at",
        )
        .bind(Uuid::now_v7())
        .bind(script_id)
        .bind(run.event)
        .bind(run.outcome.as_str())
        .bind(run.duration_ms)
        .bind(error)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let disabled =
            sqlx::query("UPDATE script_registry SET enabled=false, updated_at=$2 WHERE id=$1")
                .bind(script_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        if disabled.rows_affected() == 0 {
            return Err(ScriptStoreError::ScriptNotFound);
        }
        tx.commit().await?;
        Ok(ScriptRun::from(row))
    }

    /// Bounded retention: removes audit rows older than `cutoff`, returning
    /// the number removed. The table is append-only and otherwise never
    /// mutated; this is the single sanctioned deletion path (ADR-0134 §6).
    pub async fn purge_older_than(
        &self,
        pool: &PgPool,
        cutoff: OffsetDateTime,
    ) -> Result<u64, ScriptStoreError> {
        let result = sqlx::query("DELETE FROM script_runs WHERE created_at < $1")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}
