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
//! Hook contract (legacy-compatible; evidence:
//! legacy C# behavior audit):
//!
//! - A script body without a `function check` declaration is auto-wrapped as
//!   `function check(ctx, scriptConfig) { return (<body>); }` (the legacy
//!   substring rule is preserved verbatim, including its quirks).
//! - Accepted returns: `true` / `false` / `{ result: bool, message?: string }`.
//!   Anything else (including a thrown error) is an `error` outcome.
//! - All code executes under a `"use strict";` prologue (legacy Jint ran with
//!   strict mode on).
//!
//! Resource limits (ADR-0193): executions run in a dedicated child process,
//! with VM loop/recursion/stack limits, an OS address-space ceiling, and a
//! parent-enforced deadline. Every child is reaped; no timed-out script keeps
//! executing in the server. Input/output and concurrent executions are bounded.
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
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration as StdDuration, Instant},
};

use melodee_ingestion::{
    CandidateSnapshot, DirectoryGateFuture, DirectoryGateOutcome, DirectoryProcessingGate,
};
use melodee_persistence::{
    MAX_TIMEOUT_MS, NewScriptRun, RegisteredScript, ScriptRegistryStore, ScriptRun,
    ScriptRunOutcome, ScriptRunStore, ScriptStoreError, SettingsResolver,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use thiserror::Error;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::warn;

/// The one server-side enforcement hook the legacy server actually enforced
/// (inventory §1a); fired once per stable candidate directory before media
/// inspection.
pub const EVENT_DIRECTORY_PROCESSING_START: &str = "directoryProcessingStart";

/// Legacy master switch for hook execution (`scripting.enabled`, registered
/// default `false`). The shell/JS hook path was previously always live in the
/// Rust server regardless of this setting, so an operator who disabled
/// scripting still saw hooks run. It is read through the layered settings on
/// each candidate so a config change takes effect without a restart, and it
/// gates [`EVENT_DIRECTORY_PROCESSING_START`] evaluation.
///
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

/// Maximum simultaneously executing script child processes.
pub const MAX_SCRIPT_WORKERS: usize = 4;
/// Fixed address-space headroom for the loader, VM, and serialization, in
/// addition to the registered script memory budget (ADR-0193).
pub const WORKER_MEMORY_OVERHEAD: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MEMORY_BYTES: i64 = 16 * 1024 * 1024;
const MAX_WORKER_OUTPUT_BYTES: u64 = 32 * 1024;
const MAX_WORKER_INPUT_BYTES: u64 = 2 * 1024 * 1024;
static WORKER_ADMISSION: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

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

/// Runs one script in a single-use child process with a wall-clock deadline.
/// Async callers use the service's bounded blocking admission. The final
/// counter argument remains for source compatibility; detached threads are no
/// longer created, and the counter is never incremented.
#[must_use]
pub fn evaluate_sandboxed(
    source: &str,
    event: &str,
    timeout_ms: i32,
    on_deny: &str,
    ctx: &JsonValue,
    _leaked_threads: &AtomicUsize,
) -> Evaluation {
    evaluate_process(
        source,
        event,
        timeout_ms,
        on_deny,
        ctx,
        DEFAULT_MEMORY_BYTES,
        &AtomicBool::new(false),
    )
}

#[derive(Serialize, Deserialize)]
struct WorkerRequest {
    source: String,
    event: String,
    timeout_ms: i32,
    on_deny: String,
    ctx: JsonValue,
}

#[derive(Serialize, Deserialize)]
struct WorkerResponse {
    outcome: String,
    message: Option<String>,
    error: Option<String>,
    duration_ms: i64,
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Also covers early IO failures: no child may outlive its owner.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker_executable() -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut parent = executable.parent().ok_or("executable has no parent")?;
    // Cargo integration/unit binaries live in deps; production binaries and
    // the helper are installed beside each other. Never search PATH.
    if parent.file_name().is_some_and(|name| name == "deps") {
        parent = parent
            .parent()
            .ok_or("test executable has no profile directory")?;
    }
    Ok(parent.join(format!(
        "melodee-script-worker{}",
        std::env::consts::EXE_SUFFIX
    )))
}

fn evaluate_process(
    source: &str,
    event: &str,
    timeout_ms: i32,
    on_deny: &str,
    ctx: &JsonValue,
    memory_bytes: i64,
    cancelled: &AtomicBool,
) -> Evaluation {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let started = Instant::now();
    let timeout_ms = timeout_ms.clamp(1, MAX_TIMEOUT_MS);
    if source.len() > melodee_persistence::MAX_SCRIPT_SOURCE_BYTES || source.trim().is_empty() {
        return Evaluation::errored(
            started,
            "script body is empty or exceeds the source limit".to_owned(),
        );
    }
    if serde_json::to_vec(ctx).map_or(true, |value| value.len() > MAX_CTX_BYTES) {
        return Evaluation::errored(started, "ctx payload exceeds the byte limit".to_owned());
    }
    let request = WorkerRequest {
        source: source.to_owned(),
        event: event.to_owned(),
        timeout_ms,
        on_deny: on_deny.to_owned(),
        ctx: ctx.clone(),
    };
    let payload = match serde_json::to_vec(&request) {
        Ok(payload) if payload.len() <= MAX_WORKER_INPUT_BYTES as usize => payload,
        _ => {
            return Evaluation::errored(
                started,
                "script request exceeds the byte limit".to_owned(),
            );
        }
    };
    let result = (|| -> Result<Evaluation, String> {
        let executable = worker_executable()?;
        let memory = memory_bytes.clamp(
            melodee_persistence::MIN_MEMORY_BYTES,
            melodee_persistence::MAX_MEMORY_BYTES,
        ) as u64
            + WORKER_MEMORY_OVERHEAD;
        let mut child = ChildGuard(
            Command::new(executable)
                .args(["--memory-limit-bytes", &memory.to_string()])
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("cannot start isolated script worker: {error}"))?,
        );
        let mut input = child.0.stdin.take().ok_or("worker input pipe is missing")?;
        let output = child
            .0
            .stdout
            .take()
            .ok_or("worker output pipe is missing")?;
        // Drain both pipes concurrently so input size and pipe capacity cannot
        // defeat the parent deadline. Kill+wait closes both before joining.
        let writer = std::thread::spawn(move || input.write_all(&payload));
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            output
                .take(MAX_WORKER_OUTPUT_BYTES + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let deadline = StdDuration::from_millis(timeout_ms as u64);
        let timed_out = loop {
            match child.0.try_wait() {
                Ok(Some(_)) => break false,
                Ok(None) if started.elapsed() < deadline && !cancelled.load(Ordering::Relaxed) => {
                    std::thread::sleep(StdDuration::from_millis(2))
                }
                Ok(None) => break true,
                Err(_) => break true,
            }
        };
        if timed_out {
            let _ = child.0.kill();
        }
        let status = child.0.wait().map_err(|error| error.to_string());
        let write_result = writer
            .join()
            .map_err(|_| "worker input writer failed".to_owned());
        let output = reader
            .join()
            .map_err(|_| "worker output reader failed".to_owned())?;
        if timed_out {
            return Ok(Evaluation::finished(ScriptRunOutcome::Timeout, started));
        }
        if !status?.success() {
            return Err("isolated script worker exited before returning a verdict (resource limit or worker failure)".to_owned());
        }
        write_result?.map_err(|error| error.to_string())?;
        let output = output.map_err(|error| error.to_string())?;
        if output.len() > MAX_WORKER_OUTPUT_BYTES as usize {
            return Err("worker output exceeded the byte limit".to_owned());
        }
        let response: WorkerResponse =
            serde_json::from_slice(&output).map_err(|_| "worker returned an invalid verdict")?;
        let outcome = match response.outcome.as_str() {
            "allow" => ScriptRunOutcome::Allow,
            "deny" => ScriptRunOutcome::Deny,
            "error" => ScriptRunOutcome::Error,
            _ => return Err("worker returned an invalid outcome".to_owned()),
        };
        Ok(Evaluation {
            outcome,
            message: response.message,
            error: response.error,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        })
    })();
    result.unwrap_or_else(|error| Evaluation::errored(started, error))
}

/// Entry point for the installed, single-use helper. Limits are applied in the
/// child before reading script data, never to the server process.
pub fn run_worker(memory_limit: u64) -> Result<(), String> {
    use std::io::{Read, Write};
    if !(WORKER_MEMORY_OVERHEAD
        ..=WORKER_MEMORY_OVERHEAD + melodee_persistence::MAX_MEMORY_BYTES as u64)
        .contains(&memory_limit)
    {
        return Err("invalid worker memory limit".to_owned());
    }
    apply_memory_limit(memory_limit)?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_WORKER_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_WORKER_INPUT_BYTES as usize {
        return Err("worker input exceeded limit".to_owned());
    }
    let request: WorkerRequest =
        serde_json::from_slice(&bytes).map_err(|_| "invalid worker request")?;
    if request.source.len() > melodee_persistence::MAX_SCRIPT_SOURCE_BYTES {
        return Err("script exceeds source limit".to_owned());
    }
    let config = serde_json::json!({"eventName": request.event, "timeoutMs": request.timeout_ms, "onDeny": request.on_deny});
    let program = format!(
        "\"use strict\";\n{}",
        normalize_to_check_function(&request.source)
    );
    let evaluation = run_in_sandbox(&program, &request.ctx, &config).into_evaluation();
    let response = WorkerResponse {
        outcome: evaluation.outcome.as_str().to_owned(),
        message: evaluation.message,
        error: evaluation
            .error
            .map(|error| error.chars().take(2000).collect()),
        duration_ms: evaluation.duration_ms,
    };
    let output = serde_json::to_vec(&response).map_err(|_| "cannot serialize verdict")?;
    if output.len() > MAX_WORKER_OUTPUT_BYTES as usize {
        return Err("worker output exceeded limit".to_owned());
    }
    std::io::stdout()
        .write_all(&output)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn apply_memory_limit(bytes: u64) -> Result<(), String> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )
    .map_err(|error| format!("cannot disable worker core dumps: {error}"))?;
    let inherited = getrlimit(Resource::As);
    let limit = inherited
        .maximum
        .map_or(bytes, |maximum| maximum.min(bytes));
    setrlimit(
        Resource::As,
        Rlimit {
            current: Some(limit),
            maximum: Some(limit),
        },
    )
    .map_err(|error| format!("cannot enforce worker memory limit: {error}"))
}

#[cfg(not(target_os = "linux"))]
fn apply_memory_limit(_bytes: u64) -> Result<(), String> {
    Err("isolated scripting requires the supported Linux resource-limit boundary".to_owned())
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
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(100_000);
    context.runtime_limits_mut().set_recursion_limit(128);
    context.runtime_limits_mut().set_stack_size_limit(4096);
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

/// Registry, isolated executor, audit, and consecutive-failure circuit breaker.
pub struct ScriptingService {
    pool: PgPool,
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
        Self { pool }
    }

    /// Compatibility diagnostic retained for existing callers. Process isolation
    /// never detaches script threads, so this value is always zero.
    #[must_use]
    pub fn leaked_thread_count(&self) -> usize {
        0
    }

    /// Runs one sandboxed evaluation off the async runtime's worker threads.
    async fn evaluate_offline(
        &self,
        script_source: String,
        event: String,
        timeout_ms: i32,
        ctx: JsonValue,
        memory_bytes: i64,
    ) -> Result<Evaluation, ScriptingError> {
        let admission = WORKER_ADMISSION
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_SCRIPT_WORKERS)))
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ScriptingError::Join)?;
        struct Cancellation(Arc<AtomicBool>);
        impl Drop for Cancellation {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancellation = Cancellation(cancelled.clone());
        tokio::task::spawn_blocking(move || {
            let _admission = admission;
            evaluate_process(
                &script_source,
                &event,
                timeout_ms,
                EFFECTIVE_ON_DENY,
                &ctx,
                memory_bytes,
                &cancelled,
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
                    script.memory_bytes,
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
        let memory_bytes = match script_id {
            Some(id) => {
                ScriptRegistryStore::new()
                    .get(&self.pool, id)
                    .await?
                    .memory_bytes
            }
            None => DEFAULT_MEMORY_BYTES,
        };
        let evaluation = self
            .evaluate_offline(
                source.to_string(),
                event.to_string(),
                timeout_ms,
                ctx.clone(),
                memory_bytes,
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
    /// script's [`CIRCUIT_BREAKER_WINDOW`] most recent runs all failed, the
    /// script is disabled transactionally with a `tripped` audit row.
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
        if window_full {
            let reason = format!(
                "circuit breaker: {} consecutive failed runs; script disabled",
                CIRCUIT_BREAKER_WINDOW
            );
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

    /// Legacy `scripting.enabled` master switch. A resolution failure keeps
    /// the documented legacy default (disabled) and is logged: an unreadable
    /// settings layer must not silently enable script execution.
    async fn global_enabled(&self) -> bool {
        match SettingsResolver::new(&self.pool)
            .resolve("scripting.enabled")
            .await
        {
            Ok(resolved) => resolved
                .value
                .as_ref()
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            Err(error) => {
                warn!(%error, "unable to resolve scripting.enabled; treating scripting as disabled");
                false
            }
        }
    }

    /// Builds the `directoryProcessingStart` ctx from the verified candidate
    /// snapshot (blocking tag I/O off the runtime) and evaluates the hook.
    pub async fn evaluate_directory_processing_start(
        &self,
        candidate_root: &Path,
        snapshot: &CandidateSnapshot,
    ) -> Result<HookVerdict, ScriptingError> {
        if !self.global_enabled().await {
            return Ok(HookVerdict::allow());
        }
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
///
/// - `totalFilesCount` / `totalSizeMegabytes` / `mostRecentModified` cover
///   **top-level regular files only** (legacy used `SearchOption.
///   TopDirectoryOnly`); sizes are MiB rounded to 2 decimals; the timestamp
///   is ISO-8601 UTC (legacy emitted the "O" format — both parse via the
///   documented `new Date(ctx.*)` idiom).
/// - `mediaFilesCount`, `totalDurationMinutes`, `trackNumbers` (sorted
///   ascending, duplicates kept) come from successful tag probes of
///   top-level audio files (the inspection classifier's extension set).
/// - `hasTrackNumberGaps` uses the **shipped** legacy definition: false when
///   fewer than one media file or two track numbers, otherwise true when any
///   adjacent sorted pair is non-consecutive — the sequence may start at any
///   number (inventory §4.7 records the stricter spec-doc definition as a
///   do-not-preserve ambiguity; decision recorded in the migration guide).
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
        assert!(evaluation.outcome.is_failure(), "{evaluation:?}");
        assert!(
            started.elapsed() < StdDuration::from_secs(10),
            "caller must return promptly after the deadline"
        );
        assert_eq!(leaked.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn cancelling_an_async_evaluation_reclaims_admission_after_reaping() {
        let admission = WORKER_ADMISSION
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_SCRIPT_WORKERS)))
            .clone();
        let held = admission
            .clone()
            .acquire_many_owned((MAX_SCRIPT_WORKERS - 1) as u32)
            .await
            .expect("reserve other slots");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@localhost/unused")
            .expect("lazy pool");
        let service = ScriptingService::new(pool);
        let task = tokio::spawn(async move {
            service
                .evaluate_offline(
                    "function check(){ return /^(a+)+$/.test('a'.repeat(40)+'!'); }".into(),
                    "cancel-test".into(),
                    MAX_TIMEOUT_MS,
                    serde_json::json!({}),
                    DEFAULT_MEMORY_BYTES,
                )
                .await
        });
        tokio::time::timeout(StdDuration::from_secs(1), async {
            while admission.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("evaluation admitted");
        task.abort();
        let _ = task.await;
        // The blocking closure owns this permit until evaluate_process has
        // killed/waited for the child and joined both pipe workers.
        let recovered =
            tokio::time::timeout(StdDuration::from_millis(500), admission.acquire_owned())
                .await
                .expect("cancel reaps before full script deadline")
                .expect("recovered slot");
        drop(recovered);
        drop(held);
    }

    #[test]
    fn allocation_and_recursion_fail_in_isolation_and_next_execution_succeeds() {
        for source in [
            "function check(){ let s='x'; while(true) s += s; }",
            "function check(){ return check(); }",
        ] {
            let evaluation = eval(source, &serde_json::json!({}));
            assert!(evaluation.outcome.is_failure(), "{evaluation:?}");
            assert!(evaluation.allows());
        }
        assert_eq!(
            eval("true", &serde_json::json!({})).outcome,
            ScriptRunOutcome::Allow
        );
    }

    #[test]
    fn tiny_deadlines_terminate_children_and_never_increment_detached_threads() {
        let leaked = AtomicUsize::new(0);
        for _ in 0..4 {
            let evaluation = eval_with_timeout("function check(){ while(true){} }", 1, &leaked);
            assert!(evaluation.outcome.is_failure());
        }
        assert_eq!(leaked.load(Ordering::Relaxed), 0);
        assert_eq!(
            eval("true", &serde_json::json!({})).outcome,
            ScriptRunOutcome::Allow
        );
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
        // The documented canonical legacy script (migration guide §3):
        // identical source text must produce the documented verdicts.
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
