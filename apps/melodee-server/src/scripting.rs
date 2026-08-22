//! sandboxed scripting hooks (PRD-SCRIPT-001..005, ADR-0134 §6,
//! ADR-0135).
//!
//! Operator-installed JavaScript hooks evaluated in a **capability-less** Boa
//! sandbox: a fresh `boa_engine::Context` per execution with zero host
//! capabilities registered — no filesystem, network, process, or module
//! objects exist in the global scope. The host injects only plain data: the
//! event payload `ctx` and `scriptConfig` (`eventName`, `timeoutMs`,
//! `onDeny`), both registered as globals and passed as the `check(ctx,
//! scriptConfig)` call arguments.
//!
//! Hook contract (legacy-compatible; evidence: //! design/evidence/phase-9/script-events-legacy-inventory.md §1a): //!
//! - A script body without a `function check` declaration is auto-wrapped as
//! `function check(ctx, scriptConfig) { return (<body>); }` (the legacy
//! substring rule is preserved verbatim, including its quirks).
//! - Accepted returns: `true` / `false` / `{ result: bool, message?: string }`.
//! Anything else (including a thrown error) is an `error` outcome.
//! - All code executes under a `"use strict";` prologue (legacy Jint ran with
//! strict mode on).
//!
//! Resource limits (ADR-0135 §3): every execution runs on a dedicated OS
//! thread with a wall-clock timeout (default 250 ms, per-script override up
//! to the 10 s registry ceiling); a timed-out thread is detached and never
//! joined, bounded by a leaked-thread counter; payloads are capped at
//! [`MAX_CTX_BYTES`] and return values at [`MAX_RETURN_BYTES`]. Boa has no
//! instruction fuel (`scriptConfig.maxStatements` from legacy configs has no
//! equivalent) and no heap limit (the registry `memory_bytes` is advisory);
//! CPU runaway is bounded by the wall clock plus the circuit breaker.
//!
//! Failure semantics (ADR-0135 §4): error, timeout, and circuit-breaker trips
//! are **fail-degraded** — the triggering operation proceeds as if the hook
//! returned its documented default (`allow` for every wired hook) — and every
//! evaluation is recorded in the append-only `script_runs` audit. A script
//! whose [`CIRCUIT_BREAKER_WINDOW`] most recent runs all failed is disabled
//! transactionally (a `tripped` audit row plus the registry disable) and
//! requires an explicit operator re-enable.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration as StdDuration, Instant},
};

use melodee_ingestion::{
    CandidateSnapshot, DirectoryGateFuture, DirectoryGateOutcome, DirectoryProcessingGate,
};
use melodee_persistence::{
    MAX_TIMEOUT_MS, NewScriptRun, RegisteredScript, ScriptRegistryStore, ScriptRun,
    ScriptRunOutcome, ScriptRunStore, ScriptStoreError,
};
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use thiserror::Error;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::warn;

/// The one server-side enforcement hook the legacy server actually enforced
/// (inventory §1a); fired once per stable candidate directory before media
/// inspection.
pub const EVENT_DIRECTORY_PROCESSING_START: &str = "directoryProcessingStart";

/// Events with a live server-side evaluation point in this build. The
/// registry accepts any event name (operator forward-compatibility), but
/// only wired events ever fire; the admin API reports this list so operators
/// can tell live hooks from inert registrations.
pub const WIRED_EVENTS: &[&str] = &[EVENT_DIRECTORY_PROCESSING_START];

/// ADR-0135 §3 default wall-clock budget; mirrors the registry column
/// default.
pub const DEFAULT_TIMEOUT_MS: i32 = 250;

/// Serialized-size cap for the injected `ctx` payload. The wired hook's ctx
/// (one directory's counters and a bounded track-number list) is a few
/// hundred bytes; 256 KiB leaves generous headroom while keeping a hostile
/// or buggy payload from becoming a memory amplifier.
pub const MAX_CTX_BYTES: usize = 256 * 1024;

/// Serialized-size cap for the script's return value. Conforming returns
/// are a boolean or a small object with a short message; 16 KiB is far above
/// any legitimate verdict and bounds what a script can hand back.
pub const MAX_RETURN_BYTES: usize = 16 * 1024;

/// Circuit-breaker window: a script whose N most recent runs all ended in
/// `error`/`timeout` is disabled (ADR-0135 §3).
pub const CIRCUIT_BREAKER_WINDOW: u32 = 3;

/// Bound on detached (timed-out, never-joined) worker threads process-wide.
/// A timeout while the leaked count is at this bound trips the offending
/// script immediately instead of waiting for the per-script window to fill.
pub const MAX_LEAKED_THREADS: usize = 64;

/// Bounded retention for the execution audit; enforced on recorded runs so
/// the append-only table stays bounded without a new worker.
const RUN_RETENTION: Duration = Duration::days(90);

/// Bound on per-file tag probes while building a hook ctx. The legacy
/// provider was unbounded; this cap keeps a pathological directory (tens of
/// thousands of files) from delaying the inspection pipeline — count/size
/// fields still cover every file, duration/track fields cover the first
/// `MAX_CTX_TAG_PROBES` media files.
const MAX_CTX_TAG_PROBES: usize = 4096;

/// The effective deny action for the wired hook. `quarantine` physical moves
/// are deferred (they require the durable decision/audit contract)
/// and the legacy `delete` action stays rewritten to `skip` (inventory §1a,
/// §5); the registry therefore carries no `onDeny` column yet and the
/// injected `scriptConfig.onDeny` is this constant.
pub const EFFECTIVE_ON_DENY: &str = "skip";

/// One sandboxed evaluation's outcome. `outcome` uses the audit vocabulary
/// directly so recording needs no translation; `message` is the
/// script-provided deny message.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub outcome: ScriptRunOutcome,
    pub message: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
}

impl Evaluation {
    fn finished(outcome: ScriptRunOutcome, started: Instant) -> Self {
        Self {
            outcome,
            message: None,
            error: None,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        }
    }

    fn errored(started: Instant, error: String) -> Self {
        Self {
            outcome: ScriptRunOutcome::Error,
            message: None,
            error: Some(error),
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        }
    }

    /// The host decision this evaluation resolves to, applying fail-degraded
    /// semantics: only a real `deny` verdict denies; every failure mode is
    /// the documented default (`allow`).
    #[must_use]
    pub fn allows(&self) -> bool {
        self.outcome != ScriptRunOutcome::Deny
    }
}

/// Legacy wrap rule (ScriptEvaluationService.cs `NormalizeToCheckFunction`):
/// a body containing the substring `function check` (ordinal) is used as-is;
/// anything else becomes `function check(ctx, scriptConfig) { return (<body>);
/// }`. The substring quirk (a comment or string literal mentioning `function
/// check` suppresses wrapping) is preserved deliberately — it is the
/// documented legacy contract.
#[must_use]
pub fn normalize_to_check_function(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.contains("function check") {
        trimmed.to_string()
    } else {
        format!("function check(ctx, scriptConfig) {{ return ({trimmed}); }}")
    }
}

/// Runs one script on a dedicated OS thread with a wall-clock deadline. Pure:
/// no database, no filesystem, no async runtime assumptions; async callers
/// wrap it in `tokio::task::spawn_blocking`. A timed-out thread is detached
/// (never joined) and counted in `leaked_threads`.
#[must_use]
pub fn evaluate_sandboxed(
    source: &str,
    event: &str,
    timeout_ms: i32,
    on_deny: &str,
    ctx: &JsonValue,
    leaked_threads: &AtomicUsize,
) -> Evaluation {
    let started = Instant::now();
    let timeout_ms = timeout_ms.clamp(1, MAX_TIMEOUT_MS);

    let ctx_json = match serde_json::to_string(ctx) {
        Ok(serialized) => serialized,
        Err(error) => {
            return Evaluation::errored(started, format!("ctx not serializable: {error}"));
        }
    };
    if ctx_json.len() > MAX_CTX_BYTES {
        return Evaluation::errored(
            started,
            format!(
                "ctx payload exceeds {} bytes ({} bytes)",
                MAX_CTX_BYTES,
                ctx_json.len()
            ),
        );
    }
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Evaluation::errored(started, "script body is empty".to_string());
    }

    let program = format!("\"use strict\";\n{}", normalize_to_check_function(trimmed));
    let script_config = serde_json::json!({
        "eventName": event,
        "timeoutMs": timeout_ms,
        "onDeny": on_deny,
    });
    let ctx = ctx.clone();

    let (sender, receiver) = mpsc::channel::<Evaluation>();
    let thread = std::thread::Builder::new().name(format!("melodee-script-{event}"));
    let spawn = thread.spawn(move || {
        let evaluation = run_in_sandbox(&program, &ctx, &script_config).into_evaluation();
        // The receiver may be gone (timeout): the send result is
        // deliberately ignored — the detached thread's only job left is to
        // finish or be abandoned.
        let _ = sender.send(evaluation);
    });
    if let Err(error) = spawn {
        return Evaluation::errored(started, format!("worker thread spawn failed: {error}"));
    }

    match receiver.recv_timeout(StdDuration::from_millis(u64::from(timeout_ms as u32))) {
        Ok(evaluation) => evaluation,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let leaked = leaked_threads.fetch_add(1, Ordering::Relaxed) + 1;
            warn!(
                event,
                leaked, "script execution timed out; worker thread detached"
            );
            Evaluation {
                outcome: ScriptRunOutcome::Timeout,
                message: None,
                error: Some(format!(
                    "execution exceeded {timeout_ms} ms wall-clock budget"
                )),
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Evaluation::errored(
            started,
            "worker thread terminated without a result".to_string(),
        ),
    }
}

/// Evaluates the wrapped program in a fresh capability-less context and
/// interprets the `check` return against the documented contract.
fn run_in_sandbox(program: &str, ctx: &JsonValue, script_config: &JsonValue) -> EvaluationBuilder {
    use boa_engine::{Context, JsValue, Source, js_string, property::Attribute};

    let started = Instant::now();
    let finish = |build: Result<(bool, Option<String>), String>| -> EvaluationBuilder {
        EvaluationBuilder { started, build }
    };

    let mut context = Context::default();
    let inject = |context: &mut Context| -> Result<(JsValue, JsValue), String> {
        let ctx_value = JsValue::from_json(ctx, context)
            .map_err(|error| format!("ctx injection failed: {error}"))?;
        let config_value = JsValue::from_json(script_config, context)
            .map_err(|error| format!("scriptConfig injection failed: {error}"))?;
        // Readonly, permanent, non-enumerable: scripts receive the payload as
        // data and cannot replace the binding for subsequent reads.
        let attributes = Attribute::READONLY | Attribute::NON_ENUMERABLE | Attribute::PERMANENT;
        context
            .register_global_property(js_string!("ctx"), ctx_value.clone(), attributes)
            .map_err(|error| format!("ctx registration failed: {error}"))?;
        context
            .register_global_property(js_string!("scriptConfig"), config_value.clone(), attributes)
            .map_err(|error| format!("scriptConfig registration failed: {error}"))?;
        Ok((ctx_value, config_value))
    };

    let (ctx_value, config_value) = match inject(&mut context) {
        Ok(values) => values,
        Err(error) => return finish(Err(error)),
    };
    if let Err(error) = context.eval(Source::from_bytes(program)) {
        return finish(Err(format!("script load failed: {error}")));
    }
    let check = match context
        .global_object()
        .get(js_string!("check"), &mut context)
    {
        Ok(value) => value,
        Err(error) => return finish(Err(format!("check lookup failed: {error}"))),
    };
    let Some(check) = check.as_callable() else {
        return finish(Err(
            "script did not define a callable `check` function".to_string()
        ));
    };
    let result = match check.call(
        &JsValue::undefined(),
        &[ctx_value, config_value],
        &mut context,
    ) {
        Ok(value) => value,
        Err(error) => return finish(Err(format!("script threw: {error}"))),
    };

    // Return-size cap before interpretation: the serialized verdict is the
    // only thing a script can hand back, so bound it.
    let serialized = match result.to_json(&mut context) {
        Ok(Some(value)) => match serde_json::to_string(&value) {
            Ok(text) => text,
            Err(error) => return finish(Err(format!("return not serializable: {error}"))),
        },
        // undefined is never a conforming return.
        Ok(None) => {
            return finish(Err(
                "script returned undefined; expected true/false or { result, message? }"
                    .to_string(),
            ));
        }
        Err(error) => return finish(Err(format!("return conversion failed: {error}"))),
    };
    if serialized.len() > MAX_RETURN_BYTES {
        return finish(Err(format!(
            "return value exceeds {} bytes ({} bytes)",
            MAX_RETURN_BYTES,
            serialized.len()
        )));
    }

    finish(interpret_verdict(&serialized))
}

/// Interim carrier so every exit path of [`run_in_sandbox`] attaches the
/// same wall-clock measurement.
struct EvaluationBuilder {
    started: Instant,
    build: Result<(bool, Option<String>), String>,
}

impl EvaluationBuilder {
    fn into_evaluation(self) -> Evaluation {
        match self.build {
            Ok((true, _)) => Evaluation::finished(ScriptRunOutcome::Allow, self.started),
            Ok((false, message)) => Evaluation {
                outcome: ScriptRunOutcome::Deny,
                message,
                error: None,
                duration_ms: self
                    .started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(i64::MAX),
            },
            Err(error) => Evaluation::errored(self.started, error),
        }
    }
}

/// Interprets the serialized verdict per the documented contract: `true` /
/// `false` / `{ result: bool, message?: string }`; anything else is an error
/// outcome (fail-degraded to allow by the caller).
fn interpret_verdict(serialized: &str) -> Result<(bool, Option<String>), String> {
    let value: JsonValue = serde_json::from_str(serialized)
        .map_err(|error| format!("return deserialization failed: {error}"))?;
    match &value {
        JsonValue::Bool(decision) => Ok((*decision, None)),
        JsonValue::Object(map) => match map.get("result") {
            Some(JsonValue::Bool(decision)) => {
                let message = match map.get("message") {
                    Some(JsonValue::String(message)) => Some(message.clone()),
                    Some(other) if !other.is_null() => Some(other.to_string()),
                    _ => None,
                };
                Ok((*decision, message))
            }
            _ => Err(
                "script returned an object without a boolean `result`; expected true/false or { result, message? }"
                    .to_string(),
            ),
        },
        _ => Err(format!(
            "script returned a non-conforming value; expected true/false or {{ result, message? }}, got {serialized}"
        )),
    }
}

/// The host's combined verdict for one event after evaluating its enabled
/// scripts.
#[derive(Clone, Debug, PartialEq)]
pub struct HookVerdict {
    pub allow: bool,
    pub message: Option<String>,
}

impl HookVerdict {
    #[must_use]
    pub const fn allow() -> Self {
        Self {
            allow: true,
            message: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ScriptingError {
    #[error(transparent)]
    Store(#[from] ScriptStoreError),
    #[error("script evaluation worker failed to join")]
    Join,
    #[error("hook context build failed")]
    ContextBuild,
}

/// Registry + executor + audit + circuit breaker. Cloned handles share the
/// leaked-thread counter so the process-wide bound holds.
pub struct ScriptingService {
    pool: PgPool,
    leaked_threads: Arc<AtomicUsize>,
}

impl std::fmt::Debug for ScriptingService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScriptingService")
            .finish_non_exhaustive()
    }
}

impl ScriptingService {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            leaked_threads: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Process-wide count of detached (timed-out, never-joined) worker
    /// threads. Surfaced for diagnostics/tests.
    #[must_use]
    pub fn leaked_thread_count(&self) -> usize {
        self.leaked_threads.load(Ordering::Relaxed)
    }

    /// Runs one sandboxed evaluation off the async runtime's worker threads.
    async fn evaluate_offline(
        &self,
        script_source: String,
        event: String,
        timeout_ms: i32,
        ctx: JsonValue,
    ) -> Result<Evaluation, ScriptingError> {
        let leaked = Arc::clone(&self.leaked_threads);
        tokio::task::spawn_blocking(move || {
            evaluate_sandboxed(
                &script_source,
                &event,
                timeout_ms,
                EFFECTIVE_ON_DENY,
                &ctx,
                &leaked,
            )
        })
        .await
        .map_err(|_| ScriptingError::Join)
    }

    /// Evaluates every enabled script registered for `event` against `ctx`
    /// (deterministic name order), stopping at the first real deny. Every
    /// evaluation is audited; failures are fail-degraded (allow) and feed the
    /// circuit breaker. No enabled scripts resolves to allow **without** an
    /// audit row — the legacy missing-setting default (inventory §1a).
    pub async fn evaluate_event(
        &self,
        event: &str,
        ctx: &JsonValue,
    ) -> Result<HookVerdict, ScriptingError> {
        let scripts = ScriptRegistryStore::new()
            .list_enabled_for_event(&self.pool, event)
            .await?;
        let mut verdict = HookVerdict::allow();
        for script in scripts {
            let evaluation = self
                .evaluate_offline(
                    script.source.clone(),
                    event.to_string(),
                    script.timeout_ms,
                    ctx.clone(),
                )
                .await?;
            self.record_and_break(&script, &evaluation).await?;
            if evaluation.outcome == ScriptRunOutcome::Deny {
                verdict = HookVerdict {
                    allow: false,
                    message: evaluation.message.clone(),
                };
                break;
            }
        }
        Ok(verdict)
    }

    /// Operator test-run (replaces the legacy editor "test" button; the
    /// spec'd legacy REST `/api/scripts/validate` was never implemented and
    /// is not recreated): evaluates `source` — a stored script's current body
    /// or an ad-hoc one (`script_id` NULL) — against an admin-supplied ctx
    /// mock and records the run. A test-run never itself trips the circuit
    /// breaker, but a stored script's test-run rows are part of the failure
    /// window a later production evaluation consults — the audit stays honest
    /// about what the current body does.
    pub async fn test_run(
        &self,
        event: &str,
        script_id: Option<uuid::Uuid>,
        source: &str,
        timeout_ms: i32,
        ctx: &JsonValue,
    ) -> Result<(Evaluation, ScriptRun), ScriptingError> {
        let evaluation = self
            .evaluate_offline(
                source.to_string(),
                event.to_string(),
                timeout_ms,
                ctx.clone(),
            )
            .await?;
        let run = ScriptRunStore::new()
            .record_run(
                &self.pool,
                &NewScriptRun {
                    script_id,
                    event,
                    outcome: evaluation.outcome,
                    duration_ms: evaluation.duration_ms,
                    error: evaluation.error.as_deref(),
                },
                OffsetDateTime::now_utc(),
            )
            .await?;
        self.purge_expired_runs().await;
        Ok((evaluation, run))
    }

    /// Records one evaluation and applies the circuit breaker: when the
    /// script's [`CIRCUIT_BREAKER_WINDOW`] most recent runs all failed, or a
    /// timeout lands while the process-wide leaked-thread bound is reached,
    /// the script is disabled transactionally with a `tripped` audit row.
    async fn record_and_break(
        &self,
        script: &RegisteredScript,
        evaluation: &Evaluation,
    ) -> Result<(), ScriptingError> {
        let now = OffsetDateTime::now_utc();
        let runs = ScriptRunStore::new();
        runs.record_run(
            &self.pool,
            &NewScriptRun {
                script_id: Some(script.id),
                event: &script.event,
                outcome: evaluation.outcome,
                duration_ms: evaluation.duration_ms,
                error: evaluation.error.as_deref(),
            },
            now,
        )
        .await?;
        self.purge_expired_runs().await;

        if !evaluation.outcome.is_failure() {
            return Ok(());
        }
        let window_full = runs
            .count_recent_failures(&self.pool, script.id, CIRCUIT_BREAKER_WINDOW)
            .await?
            >= CIRCUIT_BREAKER_WINDOW;
        let leaked_bound_hit = evaluation.outcome == ScriptRunOutcome::Timeout
            && self.leaked_thread_count() >= MAX_LEAKED_THREADS;
        if window_full || leaked_bound_hit {
            let reason = if window_full {
                format!(
                    "circuit breaker: {} consecutive failed runs; script disabled",
                    CIRCUIT_BREAKER_WINDOW
                )
            } else {
                format!(
                    "circuit breaker: leaked-thread bound ({MAX_LEAKED_THREADS}) reached; script disabled"
                )
            };
            warn!(script_id = %script.id, event = %script.event, %reason);
            runs.record_trip_and_disable(
                &self.pool,
                &NewScriptRun {
                    script_id: Some(script.id),
                    event: &script.event,
                    outcome: ScriptRunOutcome::Tripped,
                    duration_ms: 0,
                    error: Some(&reason),
                },
                now,
            )
            .await?;
        }
        Ok(())
    }

    /// Bounded retention on recorded runs; a purge failure never fails the
    /// evaluation being recorded.
    async fn purge_expired_runs(&self) {
        if let Err(error) = ScriptRunStore::new()
            .purge_older_than(&self.pool, OffsetDateTime::now_utc() - RUN_RETENTION)
            .await
        {
            warn!(%error, "script_runs retention purge failed");
        }
    }

    /// Builds the `directoryProcessingStart` ctx from the verified candidate
    /// snapshot (blocking tag I/O off the runtime) and evaluates the hook.
    pub async fn evaluate_directory_processing_start(
        &self,
        candidate_root: &Path,
        snapshot: &CandidateSnapshot,
    ) -> Result<HookVerdict, ScriptingError> {
        // The enabled-script lookup is the cheap pre-gate: with no scripts
        // installed (the default) the pipeline pays one indexed query per
        // claimed candidate and no ctx is ever built.
        let any_enabled = !ScriptRegistryStore::new()
            .list_enabled_for_event(&self.pool, EVENT_DIRECTORY_PROCESSING_START)
            .await?
            .is_empty();
        if !any_enabled {
            return Ok(HookVerdict::allow());
        }
        let root = candidate_root.to_path_buf();
        let snapshot = snapshot.clone();
        let ctx = tokio::task::spawn_blocking(move || {
            build_directory_processing_context(&root, &snapshot)
        })
        .await
        .map_err(|_| ScriptingError::Join)??;
        self.evaluate_event(EVENT_DIRECTORY_PROCESSING_START, &ctx)
            .await
    }
}

/// The server's [`DirectoryProcessingGate`] implementation: adapts the
/// scripting service to the ingestion pipeline's injection point. The gate
/// is fail-degraded even across store/service failures — an error here can
/// never block ingestion (ADR-0135 §4).
pub struct ScriptingDirectoryGate {
    service: Arc<ScriptingService>,
}

impl ScriptingDirectoryGate {
    #[must_use]
    pub fn new(service: Arc<ScriptingService>) -> Self {
        Self { service }
    }
}

impl DirectoryProcessingGate for ScriptingDirectoryGate {
    fn evaluate<'a>(
        &'a self,
        candidate_root: &'a Path,
        snapshot: &'a CandidateSnapshot,
    ) -> DirectoryGateFuture<'a> {
        Box::pin(async move {
            match self
                .service
                .evaluate_directory_processing_start(candidate_root, snapshot)
                .await
            {
                Ok(verdict) if verdict.allow => DirectoryGateOutcome::Allow,
                Ok(verdict) => DirectoryGateOutcome::Skip {
                    reason: verdict
                        .message
                        .unwrap_or_else(|| "directoryProcessingStart denied".to_string()),
                },
                Err(error) => {
                    warn!(%error, "directoryProcessingStart gate failed; fail-degraded to allow");
                    DirectoryGateOutcome::Allow
                }
            }
        })
    }
}

/// Builds the legacy `DirectoryProcessingContext` payload (camelCase field
/// names are the frozen hook contract; inventory §1a) from the verified
/// candidate snapshot plus per-file tag probes — the same data sources the
/// legacy `DirectoryContextProvider` used (top-level files only; tag reads
/// through the media layer; unreadable files logged and skipped).
///
/// Field semantics, mirroring legacy exactly:
/// - `totalFilesCount` / `totalSizeMegabytes` / `mostRecentModified` cover
/// **top-level regular files only** (legacy used `SearchOption.
/// TopDirectoryOnly`); sizes are MiB rounded to 2 decimals; the timestamp
/// is ISO-8601 UTC (legacy emitted the "O" format — both parse via the
/// documented `new Date(ctx.*)` idiom).
/// - `mediaFilesCount`, `totalDurationMinutes`, `trackNumbers` (sorted
/// ascending, duplicates kept) come from successful tag probes of
/// top-level audio files (the inspection classifier's extension set).
/// - `hasTrackNumberGaps` uses the **shipped** legacy definition: false when
/// fewer than one media file or two track numbers, otherwise true when any
/// adjacent sorted pair is non-consecutive — the sequence may start at any
/// number (inventory §4.7 records the stricter spec-doc definition as a
/// do-not-preserve ambiguity; decision recorded in the migration guide).
fn build_directory_processing_context(
    root: &Path,
    snapshot: &CandidateSnapshot,
) -> Result<JsonValue, ScriptingError> {
    let top_level: Vec<_> = snapshot
        .files
        .iter()
        .filter(|file| file.file_type == "file" && !file.relative_path.contains('/'))
        .collect();

    let total_bytes: i64 = top_level.iter().map(|file| file.file_size).sum();
    let total_size_megabytes = (total_bytes as f64 / (1024.0 * 1024.0) * 100.0).round() / 100.0;
    let most_recent_modified = top_level
        .iter()
        .map(|file| file.modified_at)
        .max()
        .unwrap_or_else(OffsetDateTime::now_utc)
        .format(&Rfc3339)
        .map_err(|_| ScriptingError::ContextBuild)?;

    let mut media_files_count = 0_u64;
    let mut total_duration_ms = 0_u64;
    let mut track_numbers: Vec<i64> = Vec::new();
    for file in top_level
        .iter()
        .filter(|file| melodee_media::is_audio_file_name(&file.relative_path))
        .take(MAX_CTX_TAG_PROBES)
    {
        let Some(probe) = melodee_media::probe_media_file(&root.join(&file.relative_path)) else {
            continue;
        };
        media_files_count += 1;
        if let Some(duration_ms) = probe.duration_ms {
            total_duration_ms = total_duration_ms.saturating_add(duration_ms);
        }
        if let Some(track_number) = probe.track_number
            && track_number > 0
        {
            track_numbers.push(i64::from(track_number));
        }
    }
    track_numbers.sort_unstable();
    let total_duration_minutes = (total_duration_ms as f64 / 60_000.0 * 100.0).round() / 100.0;
    // Shipped legacy definition (DirectoryContextProvider.
    // CalculateHasTrackNumberGaps): any-start sequences count as contiguous.
    let has_track_number_gaps = media_files_count >= 1
        && track_numbers.len() >= 2
        && track_numbers.windows(2).any(|pair| pair[1] != pair[0] + 1);

    Ok(serde_json::json!({
        "path": root.to_string_lossy(),
        "directoryName": root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "totalFilesCount": top_level.len(),
        "totalSizeMegabytes": total_size_megabytes,
        "mostRecentModified": most_recent_modified,
        "mediaFilesCount": media_files_count,
        "totalDurationMinutes": total_duration_minutes,
        "trackNumbers": track_numbers,
        "hasTrackNumberGaps": has_track_number_gaps,
    }))
}

/// DTO shared by the admin API and the CLI for a recorded run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationDto {
    pub outcome: String,
    pub message: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
}

impl From<&Evaluation> for EvaluationDto {
    fn from(evaluation: &Evaluation) -> Self {
        Self {
            outcome: evaluation.outcome.as_str().to_string(),
            message: evaluation.message.clone(),
            error: evaluation.error.clone(),
            duration_ms: evaluation.duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn eval(source: &str, ctx: &JsonValue) -> Evaluation {
        evaluate_sandboxed(
            source,
            EVENT_DIRECTORY_PROCESSING_START,
            DEFAULT_TIMEOUT_MS,
            EFFECTIVE_ON_DENY,
            ctx,
            &AtomicUsize::new(0),
        )
    }

    fn eval_with_timeout(source: &str, timeout_ms: i32, leaked: &AtomicUsize) -> Evaluation {
        evaluate_sandboxed(
            source,
            EVENT_DIRECTORY_PROCESSING_START,
            timeout_ms,
            EFFECTIVE_ON_DENY,
            &serde_json::json!({}),
            leaked,
        )
    }

    #[test]
    fn sandbox_has_no_host_capability_globals() {
        // ADR-0135 §2 proof: no fs/net/process/module objects exist. The
        // expression returns true only when every capability global is
        // absent and the injected data globals are present.
        let evaluation = eval(
            "typeof process === 'undefined' \
             && typeof require === 'undefined' \
             && typeof module === 'undefined' \
             && typeof fetch === 'undefined' \
             && typeof XMLHttpRequest === 'undefined' \
             && typeof Deno === 'undefined' \
             && typeof Bun === 'undefined' \
             && typeof globalThis.ctx === 'object' \
             && typeof globalThis.scriptConfig === 'object'",
            &serde_json::json!({}),
        );
        assert_eq!(
            evaluation.outcome,
            ScriptRunOutcome::Allow,
            "{evaluation:?}"
        );
    }

    #[test]
    fn sandbox_receives_ctx_and_script_config_as_plain_data() {
        let evaluation = eval(
            "ctx.mediaFilesCount === 7 \
             && scriptConfig.eventName === 'directoryProcessingStart' \
             && scriptConfig.timeoutMs === 250 \
             && scriptConfig.onDeny === 'skip'",
            &serde_json::json!({ "mediaFilesCount": 7 }),
        );
        assert_eq!(
            evaluation.outcome,
            ScriptRunOutcome::Allow,
            "{evaluation:?}"
        );
    }

    #[test]
    fn infinite_loop_times_out_without_blocking_the_caller() {
        let leaked = AtomicUsize::new(0);
        let started = Instant::now();
        let evaluation = eval_with_timeout(
            "function check(ctx, scriptConfig) { while (true) {} }",
            100,
            &leaked,
        );
        assert_eq!(evaluation.outcome, ScriptRunOutcome::Timeout);
        assert!(
            started.elapsed() < StdDuration::from_secs(10),
            "caller must return promptly after the deadline"
        );
        assert_eq!(leaked.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn throwing_script_is_an_error_outcome() {
        // Statement-form throw via the function form; a bare `throw` body is
        // a load-time syntax error under the auto-wrap (as in legacy Jint).
        let evaluation = eval(
            "function check(ctx, scriptConfig) { throw new Error('boom'); }",
            &serde_json::json!({}),
        );
        assert_eq!(evaluation.outcome, ScriptRunOutcome::Error);
        assert!(
            evaluation
                .error
                .as_deref()
                .is_some_and(|error| error.contains("boom")),
            "{evaluation:?}"
        );
        assert!(evaluation.allows(), "error outcomes fail-degraded to allow");
    }

    #[test]
    fn nonconforming_returns_are_error_outcomes() {
        for source in ["42", "'yes'", "undefined", "{ message: 'x' }"] {
            let evaluation = eval(source, &serde_json::json!({}));
            assert_eq!(
                evaluation.outcome,
                ScriptRunOutcome::Error,
                "source {source:?}"
            );
            assert!(evaluation.allows());
        }
    }

    #[test]
    fn oversize_return_is_rejected() {
        let big = "x".repeat(MAX_RETURN_BYTES);
        let source = format!("({{ result: true, message: '{big}' }})");
        let evaluation = eval(&source, &serde_json::json!({}));
        assert_eq!(evaluation.outcome, ScriptRunOutcome::Error);
        assert!(
            evaluation
                .error
                .as_deref()
                .is_some_and(|error| error.contains("return value exceeds")),
            "{evaluation:?}"
        );
    }

    #[test]
    fn oversize_ctx_is_rejected_without_evaluating() {
        let ctx = serde_json::json!({ "blob": "x".repeat(MAX_CTX_BYTES) });
        // A script that would deny must never run: the payload cap trips
        // first and fail-degrades to allow.
        let evaluation = eval("false", &ctx);
        assert_eq!(evaluation.outcome, ScriptRunOutcome::Error);
        assert!(evaluation.allows());
    }

    #[test]
    fn strict_mode_is_enforced() {
        // Assignment to an undeclared variable throws in strict mode.
        let evaluation = eval("undeclaredGlobal = 1", &serde_json::json!({}));
        assert_eq!(
            evaluation.outcome,
            ScriptRunOutcome::Error,
            "{evaluation:?}"
        );
    }

    #[test]
    fn legacy_directory_processing_start_example_evaluates_identically() {
        // The documented canonical legacy script (migration guide §3):  // identical source text must produce the documented verdicts.
        let source = "function check(ctx, scriptConfig) {\n  if (ctx.mediaFilesCount < 3) {\n    return { result: false, message: \"Release has fewer than three media files.\" };\n  }\n  return true;\n}";

        let denied = eval(source, &serde_json::json!({ "mediaFilesCount": 2 }));
        assert_eq!(denied.outcome, ScriptRunOutcome::Deny);
        assert_eq!(
            denied.message.as_deref(),
            Some("Release has fewer than three media files.")
        );

        let allowed = eval(source, &serde_json::json!({ "mediaFilesCount": 3 }));
        assert_eq!(allowed.outcome, ScriptRunOutcome::Allow);

        // Bare-expression form through the auto-wrap (migration guide §3).
        let bare_allowed = eval(
            "ctx.mediaFilesCount >= 3 && !ctx.hasTrackNumberGaps",
            &serde_json::json!({ "mediaFilesCount": 4, "hasTrackNumberGaps": false }),
        );
        assert_eq!(bare_allowed.outcome, ScriptRunOutcome::Allow);
        let bare_denied = eval(
            "ctx.mediaFilesCount >= 3 && !ctx.hasTrackNumberGaps",
            &serde_json::json!({ "mediaFilesCount": 4, "hasTrackNumberGaps": true }),
        );
        assert_eq!(bare_denied.outcome, ScriptRunOutcome::Deny);
        assert_eq!(bare_denied.message, None);
    }

    #[test]
    fn wrap_rule_matches_legacy_substring_behavior() {
        assert_eq!(
            normalize_to_check_function("  ctx.a > 1  "),
            "function check(ctx, scriptConfig) { return (ctx.a > 1); }"
        );
        let declared = "  function check(ctx, scriptConfig) { return true; }  ";
        assert_eq!(normalize_to_check_function(declared), declared.trim());
    }

    #[test]
    fn has_track_number_gaps_uses_shipped_legacy_definition() {
        // The shipped definition allows sequences starting at any number and
        // treats duplicates as gaps (adjacent pair must differ by exactly 1).
        fn gaps(track_numbers: &[i64], media_count: u64) -> bool {
            let mut sorted = track_numbers.to_vec();
            sorted.sort_unstable();
            media_count >= 1
                && sorted.len() >= 2
                && sorted.windows(2).any(|pair| pair[1] != pair[0] + 1)
        }
        assert!(!gaps(&[], 0));
        assert!(!gaps(&[5], 1), "fewer than two track numbers: no gap");
        assert!(!gaps(&[4, 5, 6], 3), "any-start sequence is contiguous");
        assert!(gaps(&[1, 2, 4], 3));
        assert!(gaps(&[1, 1, 2], 3), "duplicate track number is a gap");
    }
}
