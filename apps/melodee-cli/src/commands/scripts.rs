//! Scripting hook administration: list registered scripts and evaluate one
//! against a ctx mock in the real sandbox (mirrors the admin API; every test
//! run is recorded in the script_runs audit).

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::Value;

use super::common::{parse_nonblank, pool, read_bounded_file};

#[derive(Debug, Subcommand)]
pub(crate) enum ScriptCommands {
    /// List registered scripts (bounded), event then name.
    List {
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Evaluate a script against a ctx mock (JSON file) in the real sandbox
    /// and print the honestly recorded outcome. Without --source, every
    /// enabled script for the event runs in registry order with production
    /// short-circuit; test-runs never trip the circuit breaker.
    Test {
        /// Hook event name (e.g. directoryProcessingStart).
        #[arg(long, value_parser = parse_nonblank)]
        event: String,
        /// Path to the ctx mock JSON file.
        #[arg(long)]
        ctx: PathBuf,
        /// Ad-hoc script source file; omit to evaluate the stored scripts.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Wall-clock budget override in milliseconds (bounded by the
        /// registry ceiling of 10000; ad-hoc default 250).
        #[arg(long)]
        timeout_ms: Option<i32>,
    },
}

/// scripting hook operations (PRD-SCRIPT-001/004, ADR-0135): DB-direct
/// over the same stores and sandboxed executor as the admin API. Outcomes —
/// including error/timeout fail-degraded results — are data, never command
/// failures (exit 1 is reserved for store failures, exit 2 for usage).
pub(crate) async fn run(command: ScriptCommands) {
    let pool = pool().await;
    match command {
        ScriptCommands::List { limit } => {
            match melodee_persistence::ScriptRegistryStore::new()
                .list(&pool, limit, 0)
                .await
            {
                Ok(scripts) => {
                    for script in &scripts {
                        let wired = if melodee_server::scripting::WIRED_EVENTS
                            .contains(&script.event.as_str())
                        {
                            "wired"
                        } else {
                            "inert"
                        };
                        println!(
                            "{:<28} {:<28} v{} enabled={} timeout={}ms {}",
                            script.event,
                            script.name,
                            script.version,
                            script.enabled,
                            script.timeout_ms,
                            wired,
                        );
                    }
                    println!("-- {} script(s)", scripts.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        ScriptCommands::Test {
            event,
            ctx,
            source,
            timeout_ms,
        } => {
            let ctx = read_json_file(&ctx);
            let service = melodee_server::scripting::ScriptingService::new(pool.clone());
            match source {
                Some(source_path) => {
                    let source = read_source_file(&source_path);
                    let timeout =
                        timeout_ms.unwrap_or(melodee_server::scripting::DEFAULT_TIMEOUT_MS);
                    match service.test_run(&event, None, &source, timeout, &ctx).await {
                        Ok((evaluation, run)) => {
                            println!(
                                "outcome:     {} (ad-hoc source)",
                                evaluation.outcome.as_str()
                            );
                            println!(
                                "message:     {}",
                                evaluation.message.as_deref().unwrap_or("-")
                            );
                            println!(
                                "error:       {}",
                                evaluation.error.as_deref().unwrap_or("-")
                            );
                            println!("duration_ms: {}", evaluation.duration_ms);
                            println!("run_id:      {}", run.id);
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    let scripts = melodee_persistence::ScriptRegistryStore::new()
                        .list_enabled_for_event(&pool, &event)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        });
                    if scripts.is_empty() {
                        println!("no enabled scripts for event {event}; verdict: allow (default)");
                        return;
                    }
                    let mut verdict = "allow";
                    for script in &scripts {
                        let timeout = timeout_ms.unwrap_or(script.timeout_ms);
                        let result = service
                            .test_run(&event, Some(script.id), &script.source, timeout, &ctx)
                            .await;
                        match result {
                            Ok((evaluation, run)) => {
                                println!(
                                    "script:      {} v{} ({})",
                                    script.name, script.version, script.id
                                );
                                println!(
                                    "outcome:     {} message={} error={} duration_ms={}",
                                    evaluation.outcome.as_str(),
                                    evaluation.message.as_deref().unwrap_or("-"),
                                    evaluation.error.as_deref().unwrap_or("-"),
                                    evaluation.duration_ms,
                                );
                                println!("run_id:      {}", run.id);
                                if evaluation.outcome == melodee_persistence::ScriptRunOutcome::Deny
                                {
                                    verdict = "deny";
                                    break;
                                }
                            }
                            Err(error) => {
                                eprintln!("error: {error}");
                                std::process::exit(1);
                            }
                        }
                    }
                    println!(
                        "verdict: {verdict} (effective onDeny: {})",
                        melodee_server::scripting::EFFECTIVE_ON_DENY
                    );
                }
            }
        }
    }
}

fn read_json_file(path: &Path) -> Value {
    let bytes =
        read_bounded_file(path, melodee_server::scripting::MAX_CTX_BYTES).unwrap_or_else(|error| {
            eprintln!("error: cannot read {}: {error}", path.display());
            std::process::exit(2);
        });
    if bytes.len() > melodee_server::scripting::MAX_CTX_BYTES {
        eprintln!(
            "error: ctx file exceeds {} bytes",
            melodee_server::scripting::MAX_CTX_BYTES
        );
        std::process::exit(2);
    }
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        eprintln!("error: {} is not valid JSON: {error}", path.display());
        std::process::exit(2);
    })
}

fn read_source_file(path: &Path) -> String {
    let bytes = read_bounded_file(path, melodee_persistence::MAX_SCRIPT_SOURCE_BYTES)
        .unwrap_or_else(|error| {
            eprintln!("error: cannot read {}: {error}", path.display());
            std::process::exit(2);
        });
    let source = String::from_utf8(bytes).unwrap_or_else(|_| {
        eprintln!("error: {} is not valid UTF-8", path.display());
        std::process::exit(2);
    });
    if source.len() > melodee_persistence::MAX_SCRIPT_SOURCE_BYTES {
        eprintln!(
            "error: source file exceeds {} bytes",
            melodee_persistence::MAX_SCRIPT_SOURCE_BYTES
        );
        std::process::exit(2);
    }
    source
}
