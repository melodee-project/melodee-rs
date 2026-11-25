//! Inbound ingestion operator commands: reconcile/inspect/normalize/validate/
//! publish bounded passes plus manifest and publication diagnostics.

use clap::Subcommand;
use melodee_domain::IngestionState;
use melodee_ingestion::{
    ENV_STORAGE_ROOT, IngestionConfig, publish_once, reconcile_all_sources, validate_once,
};
use melodee_persistence::{WorkEngineConfig, WorkItemStore};
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum IngestCommands {
    /// Register a JSON array of inbound directory names before transfer. Use --manifest - for stdin.
    RegisterBulk {
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// Configured inbound library UUID when several sources are configured.
        #[arg(long)]
        source_id: Option<Uuid>,
    },
    /// Report arrival-to-playable times for the bulk submission and concurrent ordinary arrivals.
    BulkReport { id: Uuid },
    /// Run one reconciliation pass for configured inbound sources.
    Reconcile,
    /// Claim and inspect one bounded batch of Ready Releases.
    Inspect,
    /// Claim and normalize one bounded batch of Normalizing Releases.
    Normalize,
    /// Claim and structurally validate one bounded batch before canonical I/O.
    Validate,
    /// Publish one bounded batch to the configured canonical storage root.
    Publish,
    /// Print the durable manifest for a work item.
    Manifest {
        /// Work item id or API key.
        id: Uuid,
    },
    /// Print durable validation, authorization, plan, receipt, and journal evidence.
    Publication {
        /// Work item id or API key.
        id: Uuid,
    },
    /// Print a concise ingest queue and publication-configuration summary.
    Status,
}

pub(crate) async fn run(command: IngestCommands) {
    let config = IngestionConfig::from_env().unwrap_or_else(|e| {
        eprintln!("error: invalid ingestion configuration: {e}");
        std::process::exit(2);
    });
    if matches!(&command, IngestCommands::Validate | IngestCommands::Publish) {
        require_publication_config(&config);
    }
    let pool = pool().await;
    let store = WorkItemStore;

    match command {
        IngestCommands::RegisterBulk {
            manifest,
            source_id,
        } => {
            let source = match config.sources.as_slice() {
                [] => source_id,
                [single] if source_id.is_none() => single.id,
                sources if source_id.is_some() && sources.iter().any(|s| s.id == source_id) => {
                    source_id
                }
                _ => {
                    eprintln!("error: specify the inbound source using --source-id");
                    std::process::exit(2);
                }
            };
            let result = async {
                let names: Vec<String> = if manifest == std::path::Path::new("-") {
                    serde_json::from_reader(std::io::stdin().lock())?
                } else {
                    serde_json::from_reader(std::fs::File::open(manifest)?)?
                };
                let id =
                    melodee_persistence::processing::register_bulk(&pool, source, &names).await?;
                println!(
                    "Registered {} inbound Release directories. Submission {id}",
                    names.len()
                );
                Ok::<(), Box<dyn std::error::Error>>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        }
        IngestCommands::BulkReport { id } => {
            match melodee_persistence::processing::bulk_report(&pool, id).await {
                Ok(report) => println!("{report:#}"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::Reconcile => {
            if config.sources.is_empty() {
                println!("no inbound sources configured");
                return;
            }
            let now = OffsetDateTime::now_utc();
            let results = match reconcile_all_sources(&config, &store, &pool, now).await {
                Ok(results) => results,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            };
            for result in results {
                print_reconcile_result(&result);
            }
            println!("reconciliation finished");
        }
        IngestCommands::Status => {
            let counts = match store.counts_by_state(&pool).await {
                Ok(counts) => counts,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };

            let total: i64 = counts.iter().map(|count| count.count).sum();
            println!("ingest work queue: {total} total");
            if counts.is_empty() {
                println!("no work items");
            } else {
                for state in IngestionState::ALL {
                    if let Some(count) = counts.iter().find(|count| count.state == state) {
                        println!("  {:<12} {}", state.as_str(), count.count);
                    }
                }
            }
            match config.publication_roots() {
                Ok(Some(roots)) => {
                    let scheduling = if config.publication_enabled() {
                        "enabled"
                    } else {
                        "inactive (ingest disabled or no inbound source configured)"
                    };
                    println!(
                        "canonical publication: configured ({}); worker scheduling: {scheduling}",
                        roots.storage_root.display()
                    );
                }
                Ok(None) => println!(
                    "canonical publication: disabled ({} is not set)",
                    ENV_STORAGE_ROOT
                ),
                Err(error) => {
                    eprintln!("error: invalid publication configuration: {error}");
                    std::process::exit(2);
                }
            }
        }
        IngestCommands::Inspect => {
            let work_config = WorkEngineConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid work configuration: {error}");
                std::process::exit(2);
            });
            match melodee_ingestion::inspect_ready_once(
                &config,
                &store,
                &pool,
                &work_config,
                OffsetDateTime::now_utc(),
                None,
            )
            .await
            {
                Ok(stats) => println!(
                    "inspection: claimed={} completed={} invalid={} source_changed={} failed={} errors={} duration_ms={}",
                    stats.claimed,
                    stats.completed,
                    stats.structurally_invalid,
                    stats.source_changed,
                    stats.failed,
                    stats.errors,
                    stats.duration_ms,
                ),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::Normalize => {
            let work_config = WorkEngineConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid work configuration: {error}");
                std::process::exit(2);
            });
            match melodee_ingestion::normalize_once(
                &config,
                &store,
                &pool,
                &work_config,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                Ok(stats) => println!(
                    "normalization: claimed={} completed={} failed={} errors={} duration_ms={}",
                    stats.claimed, stats.completed, stats.failed, stats.errors, stats.duration_ms,
                ),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::Validate => {
            let work_config = WorkEngineConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid work configuration: {error}");
                std::process::exit(2);
            });
            match validate_once(
                &config,
                &store,
                &pool,
                &work_config,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                Ok(stats) => println!(
                    "validation: claimed={} completed={} needs_review={} failed={} errors={} duration_ms={}",
                    stats.claimed,
                    stats.completed,
                    stats.needs_review,
                    stats.failed,
                    stats.errors,
                    stats.duration_ms,
                ),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::Publish => {
            let work_config = WorkEngineConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid work configuration: {error}");
                std::process::exit(2);
            });
            match publish_once(
                &config,
                &store,
                &pool,
                &work_config,
                OffsetDateTime::now_utc(),
            )
            .await
            {
                Ok(stats) => println!(
                    "publication: claimed={} completed={} adopted={} failed={} errors={} duration_ms={}",
                    stats.claimed,
                    stats.completed,
                    stats.adopted,
                    stats.failed,
                    stats.errors,
                    stats.duration_ms,
                ),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::Manifest { id } => match store.get(&pool, id).await {
            Ok(item) => match store.get_manifest(&pool, item.id).await {
                Ok(Some(manifest)) => {
                    println!("manifest_hash: {}", manifest.manifest_hash);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&manifest.payload)
                            .unwrap_or_else(|_| "{}".to_owned())
                    );
                }
                Ok(None) => {
                    eprintln!("no manifest published for {id}");
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            },
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        },
        IngestCommands::Publication { id } => match store.get(&pool, id).await {
            Ok(item) => match store.get_publication(&pool, item.id).await {
                Ok(Some(publication)) => {
                    println!("work_item_id:        {}", publication.work_item_id);
                    println!("release_id:          {}", publication.release_id);
                    println!(
                        "authorization:       {} v{} ({})",
                        publication.authorization.name,
                        publication.authorization.version,
                        publication.authorization.id
                    );
                    println!(
                        "validation:          {} ({} finding(s))",
                        match publication.validation.outcome {
                            melodee_domain::ValidationOutcome::Passed => "passed",
                            melodee_domain::ValidationOutcome::NeedsReview => "needs_review",
                        },
                        publication.validation.findings.len()
                    );
                    println!("plan_hash:           {}", publication.plan_hash);
                    println!("layout_version:      {}", publication.plan.layout_version);
                    println!(
                        "target_path:         {}",
                        publication.plan.target_relative_path
                    );
                    println!(
                        "manifest_path:       {}",
                        publication.plan.manifest_relative_path
                    );
                    println!("canonical_assets:    {}", publication.plan.assets.len());
                    println!("source_disposition: retained");
                    println!("authorized_at:       {}", publication.authorized_at);
                    println!("published_at:        {:?}", publication.published_at);
                    println!(
                        "sealed_manifest_hash:{:?}",
                        publication.sealed_manifest_hash
                    );
                    match store.list_publication_journal(&pool, item.id).await {
                        Ok(entries) => {
                            println!("journal:");
                            for entry in entries {
                                println!(
                                    "  [{}] {} asset={:?} worker={}",
                                    entry.created_at,
                                    entry.step.as_str(),
                                    entry.asset_path,
                                    entry.worker_id
                                );
                            }
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                Ok(None) => {
                    eprintln!("no publication authorization or plan for {id}");
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            },
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        },
    }
}

fn require_publication_config(config: &IngestionConfig) {
    match config.publication_roots() {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!(
                "error: {} must be explicitly configured for validation/publication",
                ENV_STORAGE_ROOT
            );
            std::process::exit(2);
        }
        Err(error) => {
            eprintln!("error: invalid publication configuration: {error}");
            std::process::exit(2);
        }
    }
}

fn print_reconcile_result(result: &melodee_ingestion::ReconcileResult) {
    println!("source: {}", result.source);
    println!("  candidates_seen: {}", result.stats.candidates_seen);
    println!("  new_work_items: {}", result.stats.new_work_items);
    println!(
        "  existing_work_items: {}",
        result.stats.existing_work_items
    );
    println!("  settling_updated: {}", result.stats.settling_updated);
    println!("  ready_promoted: {}", result.stats.ready_promoted);
    println!("  ready_reverted: {}", result.stats.ready_reverted);
    println!("  snapshot_changed: {}", result.stats.snapshot_changed);
    println!("  missing_candidates: {}", result.stats.missing_candidates);
    println!("  errors: {}", result.stats.errors);
    println!("  duration_ms: {}", result.stats.duration_ms);
}
