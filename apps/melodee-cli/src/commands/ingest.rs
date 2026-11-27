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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RetainedObligationsPage {
    items: Vec<melodee_persistence::RetainedObligationView>,
    next_after: Option<Uuid>,
}

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
    /// List up to 100 ambiguous pre-upgrade inbound paths for operator review.
    LegacyPaths {
        /// Continue after the last work-item id from the prior page.
        #[arg(long)]
        after: Option<Uuid>,
    },
    /// List up to 100 interrupted pre-catalog purge guards for maintenance review.
    PurgeGuards {
        /// Continue after the last work-item id from the prior page.
        #[arg(long)]
        after: Option<Uuid>,
    },
    /// List active retained-source obligations without changing their state.
    RetainedObligations {
        /// Continue after the last work-item ID from a previous page.
        #[arg(long)]
        after: Option<Uuid>,
        /// Maximum number of rows to return (1–128).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=128))]
        limit: u16,
    },
    /// Release one reviewed purge guard under an exclusive maintenance fence.
    RecoverPurgeGuard {
        /// Exact work item ID from purge-guards.
        #[arg(long)]
        work_item_id: Uuid,
        /// Exact fencing token from purge-guards.
        #[arg(long)]
        fence_token: Uuid,
        /// Exact guard owner from purge-guards.
        #[arg(long, value_parser = parse_nonblank_exact)]
        guard_owner: String,
        /// Operator identity recorded in the audit event.
        #[arg(long, value_parser = super::common::parse_nonblank)]
        actor: String,
        /// Inspection findings and reason for releasing this guard.
        #[arg(long, value_parser = super::common::parse_nonblank)]
        reason: String,
        /// All Melodee servers and workers have been stopped and fenced.
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        stopped_all_writers: bool,
        /// Source, staging, work, and canonical media were inspected.
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        inspected_media: bool,
        /// Database and affected media backups were verified.
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        backup_verified: bool,
        /// Confirm the reviewed decision to release only this guard.
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        confirm: bool,
    },
    /// Re-read an owned staging tree and verify its durable receipt.
    VerifyStaged { id: Uuid },
    /// Bind an ambiguous historical item to explicitly reviewed staged evidence.
    ResolveLegacyPath {
        #[arg(long)]
        legacy: Uuid,
        #[arg(long)]
        reviewed: Uuid,
        /// Independently reviewed BLAKE3 tree hash from the staged receipt.
        #[arg(long)]
        receipt_tree_hash: String,
        /// Operator identity recorded in the audit event.
        #[arg(long)]
        actor: String,
        /// Evidence and reason for the reviewed binding.
        #[arg(long)]
        reason: String,
    },
}

fn parse_nonblank_exact(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        Err("value must not be blank".to_owned())
    } else {
        Ok(raw.to_owned())
    }
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
        IngestCommands::RetainedObligations { after, limit } => {
            match store
                .list_retained_obligations(&pool, after, usize::from(limit))
                .await
            {
                Ok(rows) => {
                    let next_after = if rows.len() == usize::from(limit) {
                        rows.last().map(|row| row.work_item_id)
                    } else {
                        None
                    };
                    match serde_json::to_string_pretty(&RetainedObligationsPage {
                        items: rows,
                        next_after,
                    }) {
                        Ok(output) => println!("{output}"),
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::PurgeGuards { after } => {
            match store.list_pre_catalog_purge_guards(&pool, after).await {
                Ok(rows) => match serde_json::to_string_pretty(&rows) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::RecoverPurgeGuard {
            work_item_id,
            fence_token,
            guard_owner,
            actor,
            reason,
            stopped_all_writers,
            inspected_media,
            backup_verified,
            confirm,
        } => {
            // Keep this check in addition to clap's required flags so a future
            // caller constructing the enum directly cannot bypass attestations.
            if !stopped_all_writers || !inspected_media || !backup_verified || !confirm {
                eprintln!("error: maintenance and inspection attestations are required");
                std::process::exit(2);
            }
            match store
                .recover_pre_catalog_purge_guard(
                    &pool,
                    work_item_id,
                    fence_token,
                    &guard_owner,
                    &actor,
                    &reason,
                    melodee_persistence::PurgeGuardRecoveryAttestation {
                        writers_stopped: stopped_all_writers,
                        storage_inspected: inspected_media,
                        backup_verified,
                    },
                )
                .await
            {
                Ok(()) => println!("purge guard released; no media or work item was deleted"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::LegacyPaths { after } => {
            match store.list_legacy_inbound_paths(&pool, after).await {
                Ok(rows) => match serde_json::to_string_pretty(&rows) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::VerifyStaged { id } => {
            let result = async {
                let item = store.get(&pool, id).await?;
                let owned =
                    melodee_ingestion::owned_source_root(&config, &store, &pool, &item).await?;
                Ok::<_, Box<dyn std::error::Error>>(serde_json::json!({"workItemId": item.id,
                    "stagingPath": owned.root, "receiptTreeHash": owned.tree_hash}))
            }
            .await;
            match result {
                Ok(value) => println!("{value:#}"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        IngestCommands::ResolveLegacyPath {
            legacy,
            reviewed,
            receipt_tree_hash,
            actor,
            reason,
        } => {
            let review = melodee_ingestion::LegacyPathReview {
                legacy_id: legacy,
                reviewed_id: reviewed,
                receipt_tree_hash: &receipt_tree_hash,
                actor: &actor,
                reason: &reason,
                now: OffsetDateTime::now_utc(),
            };
            match melodee_ingestion::resolve_legacy_inbound_path(&config, &store, &pool, &review)
                .await
            {
                Ok(true) => println!("legacy path binding recorded"),
                Ok(false) => println!("legacy path binding already recorded"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
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
