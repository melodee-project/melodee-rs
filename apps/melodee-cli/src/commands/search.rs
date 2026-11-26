//! MQL, Tantivy projection rebuild, and search diagnostics commands.

use clap::Subcommand;
use melodee_ingestion::IngestionConfig;
use melodee_search::{
    SearchConfig, autocomplete_active, diagnostic, query_active, request_rebuild, run_rebuild_once,
};

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum SearchCommands {
    /// Show durable backlog, active generation, and on-disk health.
    Status,
    /// Build and atomically activate a fresh generation from PostgreSQL.
    Rebuild {
        /// Stable operator identity recorded with the durable request.
        #[arg(long, value_parser = super::common::parse_nonblank)]
        actor: String,
    },
    /// Execute a bounded MQL query against the active generation.
    Query {
        /// MQL expression.
        expression: String,
        /// Result limit. Defaults to MELODEE__SEARCH__DEFAULT_LIMIT.
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Return bounded deterministic prefix suggestions.
    Autocomplete {
        /// artist, release, or track.
        kind: String,
        prefix: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Parse MQL and print the pure AST as JSON.
    Parse { expression: String },
    /// Parse, validate, and print the framework-free query plan.
    Explain { expression: String },
    /// Preview or apply bounded processed-outbox retention (ADR-0214).
    ///
    /// A bare invocation shows the dry-run preview. `--apply` uses exactly the
    /// same selection, fencing and audit logic as the scheduled maintenance
    /// job; `--dry-run` is accepted for explicitness.
    PruneOutbox {
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        #[arg(long)]
        apply: bool,
    },
}

pub(crate) async fn run(command: SearchCommands) {
    let mut config = SearchConfig::from_env().unwrap_or_else(|error| {
        eprintln!("error: invalid search configuration: {error}");
        std::process::exit(2);
    });
    if !matches!(
        &command,
        SearchCommands::Parse { .. } | SearchCommands::Explain { .. }
    ) {
        let ingestion = IngestionConfig::from_env().unwrap_or_else(|error| {
            eprintln!("error: invalid ingestion configuration: {error}");
            std::process::exit(2);
        });
        let mut other_roots = ingestion
            .sources
            .iter()
            .map(|source| source.root.clone())
            .collect::<Vec<_>>();
        if let Some(roots) = ingestion.publication_roots().unwrap_or_else(|error| {
            eprintln!("error: invalid publication configuration: {error}");
            std::process::exit(2);
        }) {
            other_roots.push(roots.storage_root);
            other_roots.push(roots.work_area_base);
        }
        config.search_root = config
            .resolve_root_against(&other_roots)
            .unwrap_or_else(|error| {
                eprintln!("error: {}", error.operator_message());
                std::process::exit(2);
            });
    }
    match command {
        SearchCommands::Parse { expression } => {
            let ast = melodee_mql::parse_with_bounds(&expression, &config.mql_bounds())
                .unwrap_or_else(|error| {
                    eprintln!("error: {error}");
                    std::process::exit(2);
                });
            println!(
                "{}",
                serde_json::to_string_pretty(&ast).unwrap_or_else(|error| {
                    eprintln!("error: cannot serialize MQL AST: {error}");
                    std::process::exit(1);
                })
            );
        }
        SearchCommands::Explain { expression } => {
            let plan = melodee_mql::parse_and_plan_with_bounds(&expression, &config.mql_bounds())
                .unwrap_or_else(|error| {
                    eprintln!("error: {error}");
                    std::process::exit(2);
                });
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).unwrap_or_else(|error| {
                    eprintln!("error: cannot serialize MQL plan: {error}");
                    std::process::exit(1);
                })
            );
        }
        SearchCommands::PruneOutbox { dry_run, apply } => {
            let pool = pool().await;
            if dry_run && apply {
                eprintln!("error: --dry-run and --apply are mutually exclusive");
                std::process::exit(2);
            }
            let retention_days = match melodee_persistence::SettingsResolver::from_env(&pool) {
                Ok(resolver) => resolver
                    .resolve("search.processedOutboxRetentionDays")
                    .await
                    .ok()
                    .and_then(|setting| setting.value)
                    .and_then(|value| value.as_i64())
                    .unwrap_or(config.processed_outbox_retention_days),
                Err(_) => config.processed_outbox_retention_days,
            };
            let retention_days = retention_days.clamp(
                melodee_persistence::MIN_PROCESSED_OUTBOX_RETENTION_DAYS,
                melodee_persistence::MAX_PROCESSED_OUTBOX_RETENTION_DAYS,
            );
            let root = config.search_root.to_string_lossy().to_string();
            let now = time::OffsetDateTime::now_utc();
            let result = melodee_persistence::prune_processed_outbox(
                &pool,
                melodee_persistence::OutboxRetentionConfig { retention_days },
                apply,
                // One CLI pass stays bounded; rerun to continue.
                32,
                Some(root.as_str()),
                now,
            )
            .await;
            match result {
                Ok(result) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).unwrap_or_else(|error| {
                            eprintln!("error: cannot serialize retention result: {error}");
                            std::process::exit(1);
                        })
                    );
                }
                Err(error) => {
                    eprintln!("error: outbox retention failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        SearchCommands::Status => {
            let pool = pool().await;
            let status = diagnostic(&config, &pool).await.unwrap_or_else(|error| {
                eprintln!("error: {}", error.operator_message());
                std::process::exit(1);
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&status).unwrap_or_else(|error| {
                    eprintln!("error: cannot serialize search status: {error}");
                    std::process::exit(1);
                })
            );
        }
        SearchCommands::Rebuild { actor } => {
            let pool = pool().await;
            let job = request_rebuild(&config, &pool, &actor)
                .await
                .unwrap_or_else(|error| {
                    eprintln!("error: {}", error.operator_message());
                    std::process::exit(1);
                });
            println!("rebuild requested: {}", job.job_id);
            let completed = run_rebuild_once(&config, &pool)
                .await
                .unwrap_or_else(|error| {
                    eprintln!(
                        "error: rebuild {} failed: {}",
                        job.job_id,
                        error.operator_message()
                    );
                    std::process::exit(1);
                });
            println!("rebuild activated: {}", completed.unwrap_or(job.job_id));
        }
        SearchCommands::Query {
            expression,
            limit,
            offset,
        } => {
            let pool = pool().await;
            let hits = query_active(
                &config,
                &pool,
                &expression,
                limit.unwrap_or(config.default_limit),
                offset,
            )
            .await
            .unwrap_or_else(|error| {
                eprintln!("error: {}", error.operator_message());
                std::process::exit(1);
            });
            for hit in hits {
                println!(
                    "{}\t{}\t{}\t{}",
                    hit.kind, hit.internal_id, hit.public_id, hit.display
                );
            }
        }
        SearchCommands::Autocomplete {
            kind,
            prefix,
            limit,
        } => {
            let pool = pool().await;
            let values = autocomplete_active(&config, &pool, &kind, &prefix, limit)
                .await
                .unwrap_or_else(|error| {
                    eprintln!("error: {}", error.operator_message());
                    std::process::exit(1);
                });
            for value in values {
                println!("{value}");
            }
        }
    }
}
