use std::{env, process::Command};

use clap::{Parser, Subcommand};
use melodee_catalog::{CatalogConfig, import_once};
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_ingestion::{IngestionConfig, publish_once, reconcile_all_sources, validate_once};
use melodee_persistence::{CatalogStore, WorkEngineConfig, WorkItemStore, WorkerId, migrate};
use melodee_policy::{PolicyConfig, PolicyDryRunner, PolicyEngine};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "melodee", version, about = "Melodee Rust administrative CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print repository phase and specification locations.
    Plan,
    /// Check locally available prerequisites without changing the system.
    Doctor,
    /// Apply database migrations (explicit SQL, from empty or current).
    Migrate,
    /// Inspect durable Release work items.
    Work {
        #[command(subcommand)]
        command: WorkCommands,
    },
    /// Phase 3 ingestion worker operations and diagnostics.
    Ingest {
        #[command(subcommand)]
        command: IngestCommands,
    },
    /// Phase 4 PostgreSQL catalog import and diagnostics.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommands,
    },
    /// Phase 5 policy classification, novelty, and dry-run diagnostics.
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
}

#[derive(Debug, Subcommand)]
enum WorkCommands {
    /// List work items (optionally filtered by state), newest first.
    List {
        /// Only show items in this state (e.g. ready, failed, needs_review).
        #[arg(long)]
        state: Option<String>,
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show one work item by id or api key, including its event history.
    Show {
        /// Work item id or api key (UUID).
        id: Uuid,
    },
    /// Development only: seed N synthetic ready work items. Requires
    /// --i-know-what-im-doing because it writes test data.
    SeedTest {
        /// Number of synthetic work items to create.
        #[arg(long, default_value_t = 3)]
        count: u32,
        /// Required acknowledgement for writing synthetic data.
        #[arg(long)]
        i_know_what_im_doing: bool,
    },
    /// Development only: claim up to N items with a throwaway worker and
    /// print what was claimed (leases are then released).
    ClaimTest {
        /// Maximum items to claim (bounded by configuration).
        #[arg(long, default_value_t = 4)]
        count: u32,
        /// Required acknowledgement for mutating lease state.
        #[arg(long)]
        i_know_what_im_doing: bool,
    },
}

#[derive(Debug, Subcommand)]
enum IngestCommands {
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

#[derive(Debug, Subcommand)]
enum CatalogCommands {
    /// Claim and import one bounded batch of published manifest-v3 Releases.
    Import,
    /// Print catalog entity counts and worker configuration.
    Status,
    /// Show the immutable catalog-import receipt for one work item.
    Inspect {
        /// Work item id or API key.
        id: Uuid,
    },
}

#[derive(Debug, Subcommand)]
enum PolicyCommands {
    /// Dry-run policy evaluation against the catalog (no destructive action).
    DryRun {
        /// Maximum number of Releases to evaluate.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Show default policy rules.
    Rules,
    /// Show current policy configuration.
    Config,
}

fn command_available(command: &str, version_argument: &str) -> bool {
    Command::new(command)
        .arg(version_argument)
        .output()
        .is_ok_and(|result| result.status.success())
}

/// The database URL comes from exactly one explicit key
/// (`MELODEE__DATABASE__URL`). It is never printed because it may contain a
/// password.
fn database_url() -> String {
    match env::var(melodee_persistence::ENV_DATABASE_URL) {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!(
                "error: {} is not set (example: postgres://user:pass@host:5432/melodee)",
                melodee_persistence::ENV_DATABASE_URL
            );
            std::process::exit(2);
        }
    }
}

async fn pool() -> PgPool {
    PgPool::connect(&database_url()).await.unwrap_or_else(|e| {
        eprintln!("error: cannot connect to database: {e}");
        std::process::exit(2);
    })
}

fn print_item(item: &ReleaseWorkItem) {
    println!(
        "{}  {:<12} attempts={} lease={} next={:?} updated={}",
        item.api_key,
        item.state.as_str(),
        item.attempt_count,
        item.lease_owner.as_deref().unwrap_or("-"),
        item.next_attempt_at,
        item.updated_at,
    );
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Plan => {
            println!("Official roadmap: IMPLEMENTATION.md");
            println!("Phase 0: complete — see design/evidence/phase-0/PHASE-0-REPORT.md");
            println!("Phase 1: complete — see design/evidence/phase-1/PHASE-1-REPORT.md");
            println!("Phase 2: complete — see design/evidence/phase-2/PHASE-2-REPORT.md");
            println!("Phase 3: complete — see design/evidence/phase-3/PHASE-3-REPORT.md");
            println!("Phase 4: complete — see design/evidence/phase-4/PHASE-4-REPORT.md");
            println!("Phase 5: active — see IMPLEMENTATION.md");
            println!("Read: PRD.md, SPEC.md, IMPLEMENTATION.md, AGENTS.md");
        }
        Commands::Doctor => {
            let checks = [
                ("ffmpeg", command_available("ffmpeg", "-version")),
                ("ffprobe", command_available("ffprobe", "-version")),
                ("fpcalc", command_available("fpcalc", "-version")),
                (
                    melodee_persistence::ENV_DATABASE_URL,
                    env::var_os(melodee_persistence::ENV_DATABASE_URL).is_some(),
                ),
            ];

            for (name, available) in checks {
                let status = if available { "ok" } else { "missing" };
                println!("{name}: {status}");
            }
            match IngestionConfig::from_env() {
                Ok(config) => match config.publication_roots() {
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
                        melodee_ingestion::ENV_STORAGE_ROOT
                    ),
                    Err(error) => println!("canonical publication: invalid ({error})"),
                },
                Err(error) => println!("canonical publication: invalid ({error})"),
            }
        }
        Commands::Migrate => {
            let pool = pool().await;
            if let Err(e) = migrate(&pool).await {
                eprintln!("error: migration failed: {e}");
                std::process::exit(1);
            }
            println!("migrations applied");
        }
        Commands::Work { command } => {
            let pool = pool().await;
            let store = WorkItemStore;
            let config = WorkEngineConfig::from_env().unwrap_or_else(|e| {
                eprintln!("error: invalid work configuration: {e}");
                std::process::exit(2);
            });
            match command {
                WorkCommands::List { state, limit } => {
                    let parsed = state.map(|s| {
                        IngestionState::parse(&s).unwrap_or_else(|| {
                            eprintln!("error: unknown state '{s}'");
                            std::process::exit(2);
                        })
                    });
                    match store.list(&pool, parsed, limit.min(500), 0).await {
                        Ok(items) => {
                            for item in &items {
                                print_item(item);
                            }
                            println!("-- {} item(s)", items.len());
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                WorkCommands::Show { id } => match store.get(&pool, id).await {
                    Ok(item) => {
                        print_item(&item);
                        println!("  id:            {}", item.id);
                        println!("  source_path:   {}", item.source_path);
                        println!("  current_path:  {}", item.current_path);
                        println!("  state_reason:  {:?}", item.state_reason);
                        println!(
                            "  last_error:    {:?} {:?}",
                            item.last_error_code, item.last_error_message
                        );
                        println!("  correlation:   {}", item.correlation_id);
                        println!("  discovered_at: {}", item.discovered_at);
                        println!("  completed_at:  {:?}", item.completed_at);
                        match store.list_events(&pool, item.id).await {
                            Ok(events) => {
                                println!("  events:");
                                for event in events {
                                    println!(
                                        "    [{}] {} {:?} -> {:?} {:?} worker={:?}",
                                        event.created_at,
                                        event.event_type.as_str(),
                                        event.from_state.map(|s| s.as_str()),
                                        event.to_state.map(|s| s.as_str()),
                                        event.reason_code.unwrap_or_default(),
                                        event.worker_id,
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("error: {e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                },
                WorkCommands::SeedTest {
                    count,
                    i_know_what_im_doing,
                } => {
                    if !i_know_what_im_doing {
                        eprintln!(
                            "error: seed-test writes synthetic data; pass --i-know-what-im-doing"
                        );
                        std::process::exit(2);
                    }
                    let now = OffsetDateTime::now_utc();
                    for index in 0..count {
                        let item = ReleaseWorkItem::discovered(
                            format!("/tmp/melodee-seed-test/release-{index}"),
                            None,
                            Uuid::now_v7(),
                            now,
                        );
                        let id = item.id;
                        if let Err(e) = store.create(&pool, &item).await {
                            eprintln!("error: {e}");
                            std::process::exit(1);
                        }
                        for to in [IngestionState::Settling, IngestionState::Ready] {
                            if let Err(e) = store
                                .system_transition(&pool, id, to, Some("seed-test"), now)
                                .await
                            {
                                eprintln!("error: {e}");
                                std::process::exit(1);
                            }
                        }
                        println!("seeded {id}");
                    }
                }
                WorkCommands::ClaimTest {
                    count,
                    i_know_what_im_doing,
                } => {
                    if !i_know_what_im_doing {
                        eprintln!(
                            "error: claim-test mutates lease state; pass --i-know-what-im-doing"
                        );
                        std::process::exit(2);
                    }
                    let worker = WorkerId::new();
                    let now = OffsetDateTime::now_utc();
                    let config = WorkEngineConfig {
                        claim_batch_size: count.max(1),
                        ..config
                    };
                    match store.claim(&pool, &worker, &config, now).await {
                        Ok(items) => {
                            for item in &items {
                                print_item(item);
                                if let Err(e) =
                                    store.release_lease(&pool, item.id, &worker, now).await
                                {
                                    eprintln!("error releasing {}: {e}", item.id);
                                    std::process::exit(1);
                                }
                            }
                            println!(
                                "-- claimed {} item(s) as {worker} (leases released)",
                                items.len()
                            );
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Commands::Ingest { command } => {
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
                            melodee_ingestion::ENV_STORAGE_ROOT
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
                            stats.claimed,
                            stats.completed,
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
                                    melodee_domain::ValidationOutcome::NeedsReview => {
                                        "needs_review"
                                    }
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
        Commands::Policy { command } => {
            let policy_config = match PolicyConfig::from_env() {
                Ok(c) => c,
                Err(error) => {
                    eprintln!("error: invalid policy configuration: {error}");
                    std::process::exit(2);
                }
            };
            let pool = pool().await;
            let engine = PolicyEngine::new(policy_config.clone());
            let store = melodee_persistence::PolicyStore;
            match command {
                PolicyCommands::DryRun { limit } => {
                    let now = OffsetDateTime::now_utc();
                    match run_policy_dry_run(&engine, &store, &pool, limit, now).await {
                        Ok(stats) => {
                            println!(
                                "dry-run: evaluated={} decisions={} errors={} duration_ms={}",
                                stats.evaluated, stats.decisions, stats.errors, stats.duration_ms
                            );
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                PolicyCommands::Rules => {
                    println!("rule_set_version: {}", engine.rule_set_version());
                    println!("phase5-default-rules:");
                    for rule in &melodee_policy::default_rules().rules {
                        println!("  {}", rule);
                    }
                }
                PolicyCommands::Config => {
                    println!("policy config:");
                    println!("  batch_size: {}", policy_config.batch_size);
                    println!("  concurrency: {}", policy_config.concurrency);
                    println!(
                        "  destructive_confidence_bps: {}",
                        policy_config.destructive_confidence_bps
                    );
                }
            }
        }
        Commands::Catalog { command } => {
            let catalog_config = CatalogConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid catalog configuration: {error}");
                std::process::exit(2);
            });
            let pool = pool().await;
            let store = CatalogStore;

            match command {
                CatalogCommands::Import => {
                    let work_config = WorkEngineConfig::from_env().unwrap_or_else(|error| {
                        eprintln!("error: invalid work configuration: {error}");
                        std::process::exit(2);
                    });
                    match import_once(
                        &catalog_config,
                        &store,
                        &pool,
                        &work_config,
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    {
                        Ok(stats) => println!(
                            "catalog import: claimed={} completed={} already_completed={} released={} failed={} errors={} duration_ms={}",
                            stats.claimed,
                            stats.completed,
                            stats.already_completed,
                            stats.released,
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
                CatalogCommands::Status => match store.counts(&pool).await {
                    Ok(counts) => {
                        println!(
                            "catalog worker: {} concurrency={} batch_size={} claim_limit={} poll_interval_seconds={}",
                            if catalog_config.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            },
                            catalog_config.concurrency,
                            catalog_config.batch_size,
                            catalog_config.claim_limit(),
                            catalog_config.poll_interval.whole_seconds(),
                        );
                        println!("artists:        {}", counts.artists);
                        println!("releases:       {}", counts.releases);
                        println!("tracks:         {}", counts.tracks);
                        println!("media_assets:   {}", counts.media_assets);
                        println!("imports:        {}", counts.imports);
                        println!("pending_outbox: {}", counts.pending_outbox);
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    }
                },
                CatalogCommands::Inspect { id } => {
                    let work_store = WorkItemStore;
                    let item = match work_store.get(&pool, id).await {
                        Ok(item) => item,
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    };
                    println!("work_item_id:    {}", item.id);
                    println!("work_item_key:   {}", item.api_key);
                    println!("state:           {}", item.state.as_str());
                    println!("attempts:        {}", item.attempt_count);
                    println!("next_attempt_at: {:?}", item.next_attempt_at);
                    println!("last_error_code: {:?}", item.last_error_code);
                    println!("last_error:      {:?}", item.last_error_message);
                    match store.get_import(&pool, item.id).await {
                        Ok(Some(import)) => {
                            println!("catalog_import:  {}", import.id);
                            println!("release_id:      {}", import.release_id);
                            println!("manifest_row_id: {}", import.manifest_row_id);
                            println!("manifest_id:     {}", import.manifest_id);
                            println!("manifest_schema: {}", import.manifest_schema_version);
                            println!("manifest_hash:   {}", import.semantic_manifest_hash);
                            println!("plan_hash:       {}", import.publication_plan_hash);
                            println!("sealed_hash:     {}", import.sealed_manifest_hash);
                            println!("resolver:        {}", import.resolver_version);
                            println!("imported_at:     {}", import.imported_at);
                        }
                        Ok(None) => println!("catalog_import:  none"),
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
    }
}

fn require_publication_config(config: &IngestionConfig) {
    match config.publication_roots() {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!(
                "error: {} must be explicitly configured for validation/publication",
                melodee_ingestion::ENV_STORAGE_ROOT
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

async fn run_policy_dry_run(
    engine: &PolicyEngine,
    _store: &melodee_persistence::PolicyStore,
    pool: &PgPool,
    limit: u32,
    now: OffsetDateTime,
) -> Result<melodee_policy::PolicyPassStats, Box<dyn std::error::Error>> {
    let rows: Vec<(uuid::Uuid, String)> =
        sqlx::query_as(r#"SELECT id, title FROM releases ORDER BY id LIMIT $1"#)
            .bind(i64::from(limit.min(500)))
            .fetch_all(pool)
            .await?;

    let inputs: Vec<melodee_policy::PolicyInput> = rows
        .iter()
        .map(|(id, title)| melodee_policy::PolicyInput {
            release_id: melodee_domain::ReleaseId::from_uuid(*id),
            title: Some(title.clone()),
            directory_name: None,
            parent_directory_name: None,
            structured_tag_traits: vec![],
            embedded_external_id_traits: vec![],
            catalog_known_release_traits: vec![],
            existing_classification_traits: std::collections::BTreeSet::new(),
            actors: Vec::new(),
            novelty_category: melodee_policy::NoveltyCategory::Unknown,
            now,
        })
        .collect();

    let runner = PolicyDryRunner::new(engine.clone());
    let results = runner.run_dry_run(inputs, now);
    Ok(results.stats)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn publication_diagnostics_command_accepts_work_item_identifier() {
        let id = Uuid::now_v7();
        let cli = Cli::try_parse_from(["melodee", "ingest", "publication", &id.to_string()])
            .expect("valid publication diagnostics command");
        assert!(matches!(
            &cli.command,
            Commands::Ingest {
                command: IngestCommands::Publication { id: parsed }
            } if *parsed == id
        ));
    }

    #[test]
    fn validation_and_publication_batch_commands_are_exposed() {
        for (name, expected_validation) in [("validate", true), ("publish", false)] {
            let cli = Cli::try_parse_from(["melodee", "ingest", name])
                .expect("valid bounded stage command");
            assert!(
                matches!(
                    &cli.command,
                    Commands::Ingest {
                        command: IngestCommands::Validate
                    }
                ) == expected_validation
            );
            assert!(
                matches!(
                    &cli.command,
                    Commands::Ingest {
                        command: IngestCommands::Publish
                    }
                ) != expected_validation
            );
        }
    }

    #[test]
    fn catalog_operator_commands_are_exposed() {
        let import = Cli::try_parse_from(["melodee", "catalog", "import"])
            .expect("valid catalog import command");
        assert!(matches!(
            import.command,
            Commands::Catalog {
                command: CatalogCommands::Import
            }
        ));

        let status = Cli::try_parse_from(["melodee", "catalog", "status"])
            .expect("valid catalog status command");
        assert!(matches!(
            status.command,
            Commands::Catalog {
                command: CatalogCommands::Status
            }
        ));

        let id = Uuid::now_v7();
        let inspect =
            Cli::try_parse_from(["melodee", "catalog", "inspect", id.to_string().as_str()])
                .expect("valid catalog inspect command");
        assert!(matches!(
            inspect.command,
            Commands::Catalog {
                command: CatalogCommands::Inspect { id: parsed }
            } if parsed == id
        ));
    }
}
