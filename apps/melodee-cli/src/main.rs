use std::{collections::BTreeMap, env, process::Command};

use clap::{Parser, Subcommand, ValueEnum};
use melodee_catalog::{CatalogConfig, import_once};
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_ingestion::{IngestionConfig, publish_once, reconcile_all_sources, validate_once};
use melodee_persistence::{
    CatalogStore, PolicyAdminOverrideRequest, PolicyStore, WorkEngineConfig, WorkItemStore,
    WorkerId, migrate,
};
use melodee_policy::{
    PolicyConfig, PolicyEngine, PolicyEvaluation, default_rule_set, evaluate_catalog_context,
};
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
    /// Test policy rules against a sample of existing catalog Releases.
    /// Reports matched-rule counts, existing dispositions, and destructive impact.
    Test {
        /// Maximum number of Releases to test.
        #[arg(
            long,
            default_value_t = 100,
            value_parser = clap::value_parser!(u32).range(1..=10_000)
        )]
        limit: u32,
        /// Stable administrator identity recorded with the test summary.
        #[arg(long, value_parser = parse_nonblank)]
        actor: String,
    },
    /// Evaluate a single Release: classify, assess novelty, and produce a
    /// disposition without persisting or executing.
    Evaluate {
        /// Release UUID (v7).
        id: Uuid,
    },
    /// Inspect the durable current policy decision and audit history.
    Inspect {
        /// Release UUID (v7).
        id: Uuid,
    },
    /// Record an explicit Administrator override as a pending decision.
    Decide {
        /// Release UUID (v7).
        id: Uuid,
        /// Administrator-selected disposition.
        disposition: PolicyDispositionArg,
        /// Stable administrator identity recorded in the audit trail.
        #[arg(long)]
        actor: String,
        /// Operator-safe reason recorded with the override.
        #[arg(long)]
        reason: String,
    },
    /// Show default policy rules.
    Rules,
    /// Show current policy configuration.
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PolicyDispositionArg {
    Accept,
    Review,
    Ignore,
    Quarantine,
    Delete,
}

impl From<PolicyDispositionArg> for melodee_domain::Disposition {
    fn from(value: PolicyDispositionArg) -> Self {
        match value {
            PolicyDispositionArg::Accept => Self::Accept,
            PolicyDispositionArg::Review => Self::Review,
            PolicyDispositionArg::Ignore => Self::Ignore,
            PolicyDispositionArg::Quarantine => Self::Quarantine,
            PolicyDispositionArg::Delete => Self::Delete,
        }
    }
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
            println!("Phase 5: complete — see design/evidence/phase-5/PHASE-5-REPORT.md");
            println!("Phase 6: active — see IMPLEMENTATION.md");
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
            let store = melodee_persistence::PolicyStore;
            let now = OffsetDateTime::now_utc();
            let engine = match load_policy_engine(&store, &pool, &policy_config, now).await {
                Ok(engine) => engine,
                Err(error) => {
                    eprintln!("error: unable to load immutable policy rule set: {error}");
                    std::process::exit(1);
                }
            };
            match command {
                PolicyCommands::Test { limit, actor } => {
                    match run_policy_test(&engine, &store, &pool, limit, actor, now).await {
                        Ok(report) => {
                            println!(
                                "test: evaluated={} errors={} destructive_impact={} samples={}",
                                report.summary.total_releases_evaluated,
                                report.errors,
                                report.summary.destructive_impact_count,
                                report.summary.sample_release_ids.len()
                            );
                            println!(
                                "  proposed_dispositions: {:?}",
                                report.proposed_dispositions
                            );
                            println!("  matched_rules: {:?}", report.matched_rules);
                            println!(
                                "  existing_dispositions: {:?}",
                                report.existing_dispositions
                            );
                            println!("  existing_states: {:?}", report.existing_states);
                            println!(
                                "  sample_release_ids: {:?}",
                                report.summary.sample_release_ids
                            );
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                PolicyCommands::Evaluate { id } => {
                    let release_id = melodee_domain::ReleaseId::from_uuid(id);
                    match run_policy_evaluate(&engine, &store, &pool, release_id, now).await {
                        Ok(output) => {
                            println!(
                                "evaluate: release_id={} disposition={} potential_destructive={} confidence={} existing_disposition={} rules={:?} evidence={}",
                                id,
                                output.decision.disposition.as_str(),
                                output.decision.disposition.is_destructive(),
                                output.classification.confidence.basis_points(),
                                output
                                    .existing_disposition
                                    .map_or("none", melodee_domain::Disposition::as_str),
                                output.decision.matched_rule_ids,
                                output.decision.evidence_summary,
                            );
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                PolicyCommands::Inspect { id } => {
                    let release_id = melodee_domain::ReleaseId::from_uuid(id);
                    match run_policy_inspect(&store, &pool, release_id).await {
                        Ok(()) => {}
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                PolicyCommands::Decide {
                    id,
                    disposition,
                    actor,
                    reason,
                } => {
                    let release_id = melodee_domain::ReleaseId::from_uuid(id);
                    match run_policy_decide(
                        &store,
                        &pool,
                        release_id,
                        disposition.into(),
                        actor,
                        reason,
                        now,
                    )
                    .await
                    {
                        Ok(decision) => {
                            println!(
                                "decide: release_id={} disposition={} completion_state={} actor={:?} reason={}",
                                id,
                                decision.disposition.as_str(),
                                decision.completion_state.as_str(),
                                decision.actor,
                                decision.override_reason.as_deref().unwrap_or("none"),
                            );
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                PolicyCommands::Rules => {
                    println!("rule_set_id: {}", engine.rule_set_id());
                    println!("rule_set_version: {}", engine.rule_set_version());
                    println!("rule_set_name: {}", engine.rule_set().name);
                    for rule in &engine.rule_set().rules {
                        println!(
                            "  {} type={} disposition={} confidence_threshold={:?}",
                            rule.id,
                            rule.rule_type.as_str(),
                            rule.disposition.as_str(),
                            rule.confidence_threshold.map(|value| value.basis_points()),
                        );
                    }
                }
                PolicyCommands::Config => {
                    println!("policy config:");
                    println!("  batch_size: {}", policy_config.batch_size);
                    println!("  concurrency: {}", policy_config.concurrency);
                    println!("  rule_set_version: {}", policy_config.rule_set_version);
                    println!(
                        "  destructive_confidence_bps: {}",
                        policy_config.destructive_confidence_bps
                    );
                    println!(
                        "  redundant_greatest_hits_disposition: {}",
                        policy_config.redundant_greatest_hits_disposition.as_str()
                    );
                    println!(
                        "  novel_greatest_hits_disposition: {}",
                        policy_config.novel_greatest_hits_disposition.as_str()
                    );
                    println!(
                        "  uncertain_greatest_hits_disposition: {}",
                        policy_config.uncertain_greatest_hits_disposition.as_str()
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

#[derive(Debug)]
struct PolicyTestReport {
    summary: melodee_domain::PolicyTestSummary,
    proposed_dispositions: BTreeMap<String, u32>,
    matched_rules: BTreeMap<String, u32>,
    existing_dispositions: BTreeMap<String, u32>,
    existing_states: BTreeMap<String, u32>,
    errors: u32,
}

#[derive(Debug)]
struct PolicyTestEvaluationError {
    release_id: melodee_domain::ReleaseId,
    source: melodee_policy::PolicyEngineError,
}

impl std::fmt::Display for PolicyTestEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "policy test evaluation failed for Release {}: {}",
            self.release_id, self.source
        )
    }
}

impl std::error::Error for PolicyTestEvaluationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

async fn load_policy_engine(
    store: &PolicyStore,
    pool: &PgPool,
    config: &PolicyConfig,
    now: OffsetDateTime,
) -> Result<PolicyEngine, Box<dyn std::error::Error>> {
    let proposed = default_rule_set(config)?;
    let stored = store.ensure_rule_set(pool, &proposed, now).await?;
    Ok(PolicyEngine::with_rule_set(config.clone(), stored)?)
}

async fn run_policy_test(
    engine: &PolicyEngine,
    store: &PolicyStore,
    pool: &PgPool,
    limit: u32,
    actor: String,
    now: OffsetDateTime,
) -> Result<PolicyTestReport, Box<dyn std::error::Error>> {
    let contexts = store.load_test_contexts(pool, limit).await?;
    let mut proposed_dispositions = BTreeMap::new();
    let mut matched_rules = BTreeMap::new();
    let mut existing_dispositions = BTreeMap::new();
    let mut existing_states = BTreeMap::new();
    let mut samples = Vec::new();
    let mut destructive_impact = 0u32;
    let mut evaluated = 0u32;

    for context in &contexts {
        let evaluation = require_policy_test_evaluation(
            context.claim.release_id,
            evaluate_catalog_context(engine, context, now),
        )?;
        evaluated = evaluated.saturating_add(1);
        increment(
            &mut proposed_dispositions,
            evaluation.decision.disposition.as_str(),
        );
        if evaluation.decision.disposition.is_destructive() {
            destructive_impact = destructive_impact.saturating_add(1);
        }
        for matched in &evaluation.matched_rules {
            increment(&mut matched_rules, &matched.rule_id);
        }
        increment(
            &mut existing_dispositions,
            context
                .existing_disposition
                .map_or("none", melodee_domain::Disposition::as_str),
        );
        increment(&mut existing_states, context.claim.item.state.as_str());
        if samples.len() < melodee_persistence::MAX_POLICY_TEST_SAMPLES {
            samples.push(context.claim.release_id);
        }
    }

    let disposition_counts = melodee_domain::Disposition::ALL
        .into_iter()
        .filter_map(|disposition| {
            proposed_dispositions
                .get(disposition.as_str())
                .copied()
                .filter(|count| *count > 0)
                .map(|count| (disposition, count))
        })
        .collect();
    let matched_rule_counts = matched_rules
        .iter()
        .map(|(rule, count)| (rule.clone(), *count))
        .collect();
    let mut existing_disposition_counts = existing_dispositions
        .get("none")
        .copied()
        .filter(|count| *count > 0)
        .map(|count| vec![(None, count)])
        .unwrap_or_default();
    existing_disposition_counts.extend(melodee_domain::Disposition::ALL.into_iter().filter_map(
        |disposition| {
            existing_dispositions
                .get(disposition.as_str())
                .copied()
                .filter(|count| *count > 0)
                .map(|count| (Some(disposition), count))
        },
    ));
    let summary = melodee_domain::PolicyTestSummary::new(melodee_domain::PolicyTestSummary {
        test_run_id: melodee_domain::PolicyTestRunId::new(),
        rule_set_id: engine.rule_set_id(),
        rule_set_version: engine.rule_set_version(),
        total_releases_evaluated: evaluated,
        disposition_counts,
        matched_rule_counts,
        existing_disposition_counts,
        destructive_impact_count: destructive_impact,
        sample_release_ids: samples,
        ran_by: melodee_domain::PolicyActor::Administrator { actor },
        ran_at: now,
    })?;
    store.store_test_run(pool, &summary).await?;

    Ok(PolicyTestReport {
        summary,
        proposed_dispositions,
        matched_rules,
        existing_dispositions,
        existing_states,
        errors: 0,
    })
}

fn require_policy_test_evaluation(
    release_id: melodee_domain::ReleaseId,
    result: Result<PolicyEvaluation, melodee_policy::PolicyEngineError>,
) -> Result<PolicyEvaluation, PolicyTestEvaluationError> {
    result.map_err(|source| PolicyTestEvaluationError { release_id, source })
}

async fn run_policy_evaluate(
    engine: &PolicyEngine,
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
    now: OffsetDateTime,
) -> Result<PolicyEvaluation, Box<dyn std::error::Error>> {
    let context = store
        .load_snapshot(pool, release_id)
        .await?
        .ok_or_else(|| operator_error(format!("Release {release_id} is not cataloged")))?;
    Ok(evaluate_catalog_context(engine, &context, now)?)
}

async fn run_policy_inspect(
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
) -> Result<(), Box<dyn std::error::Error>> {
    let (decision, history) = store.inspect_decision(pool, release_id).await?;
    let decision = decision.ok_or_else(|| {
        operator_error(format!(
            "Release {release_id} has no durable policy decision"
        ))
    })?;
    println!("release_id:          {}", decision.release_id);
    println!("work_item_id:        {}", decision.work_item_id);
    println!("decision_id:         {}", decision.id);
    println!(
        "rule_set:            {}@{}",
        decision.rule_set_id, decision.rule_set_version
    );
    println!("disposition:         {}", decision.disposition.as_str());
    println!(
        "completion_state:    {}",
        decision.completion_state.as_str()
    );
    println!("matched_rules:       {:?}", decision.matched_rule_ids);
    println!("source_fingerprint:  {}", decision.source_fingerprint);
    println!("actor:                {:?}", decision.actor);
    println!(
        "override_reason:      {}",
        decision.override_reason.as_deref().unwrap_or("none")
    );
    println!("evidence:             {}", decision.evidence_summary);
    println!("decided_at:           {}", decision.decided_at);
    println!("completed_at:         {:?}", decision.completed_at);
    println!("audit:");
    for entry in history {
        println!(
            "  {} event={} disposition={} completion={} actor={:?} reason={}",
            entry.created_at,
            entry.event,
            entry.decision.disposition.as_str(),
            entry.decision.completion_state.as_str(),
            entry.decision.actor,
            entry.decision.override_reason.as_deref().unwrap_or("none"),
        );
    }
    Ok(())
}

async fn run_policy_decide(
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
    disposition: melodee_domain::Disposition,
    actor: String,
    reason: String,
    now: OffsetDateTime,
) -> Result<melodee_persistence::StoredPolicyDecision, Box<dyn std::error::Error>> {
    let context = store
        .load_snapshot(pool, release_id)
        .await?
        .ok_or_else(|| operator_error(format!("Release {release_id} is not cataloged")))?;
    let decision = context.current_decision.ok_or_else(|| {
        operator_error(format!("Release {release_id} has no current decision to override; allow the policy worker to evaluate it first"))
    })?;
    let result = store
        .apply_admin_override(
            pool,
            &PolicyAdminOverrideRequest {
                work_item_id: context.claim.item.id,
                decision_id: decision.id,
                chosen_disposition: disposition,
                actor,
                reason,
                source_fingerprint: context.claim.source_fingerprint,
                now,
            },
        )
        .await?;
    Ok(result.decision)
}

fn increment(counts: &mut BTreeMap<String, u32>, key: &str) {
    let count = counts.entry(key.to_owned()).or_insert(0);
    *count = count.saturating_add(1);
}

fn parse_nonblank(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        Err("value must not be blank".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

fn operator_error(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
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

    #[test]
    fn policy_read_only_commands_match_the_work_package() {
        let test = Cli::try_parse_from([
            "melodee",
            "policy",
            "test",
            "--limit",
            "23",
            "--actor",
            "policy-test-admin",
        ])
        .expect("valid policy test command");
        assert!(matches!(
            test.command,
            Commands::Policy {
                command: PolicyCommands::Test { limit: 23, actor }
            } if actor == "policy-test-admin"
        ));
        assert!(
            Cli::try_parse_from([
                "melodee", "policy", "test", "--limit", "0", "--actor", "admin",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "melodee", "policy", "test", "--limit", "10001", "--actor", "admin",
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["melodee", "policy", "test", "--limit", "1"]).is_err());
        assert!(
            Cli::try_parse_from([
                "melodee", "policy", "test", "--limit", "1", "--actor", "   ",
            ])
            .is_err()
        );

        let id = Uuid::now_v7();
        for command in ["evaluate", "inspect"] {
            let parsed =
                Cli::try_parse_from(["melodee", "policy", command, id.to_string().as_str()])
                    .expect("valid policy release command");
            assert!(match parsed.command {
                Commands::Policy {
                    command: PolicyCommands::Evaluate { id: parsed },
                }
                | Commands::Policy {
                    command: PolicyCommands::Inspect { id: parsed },
                } => parsed == id,
                _ => false,
            });
        }
    }

    #[test]
    fn policy_test_evaluation_error_is_typed_and_not_silently_counted() {
        let release_id = melodee_domain::ReleaseId::new();
        let error = require_policy_test_evaluation(
            release_id,
            Err(melodee_policy::PolicyEngineError::ReleaseMismatch),
        )
        .expect_err("invalid Release facts fail the entire policy test");
        assert_eq!(error.release_id, release_id);
        assert!(error.to_string().contains(&release_id.to_string()));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn policy_decide_requires_disposition_actor_and_reason() {
        let id = Uuid::now_v7();
        let parsed = Cli::try_parse_from([
            "melodee",
            "policy",
            "decide",
            id.to_string().as_str(),
            "review",
            "--actor",
            "local-admin",
            "--reason",
            "verified uncertain audio",
        ])
        .expect("valid policy decision command");
        assert!(matches!(
            parsed.command,
            Commands::Policy {
                command: PolicyCommands::Decide {
                    id: parsed,
                    disposition: PolicyDispositionArg::Review,
                    actor,
                    reason,
                }
            } if parsed == id && actor == "local-admin" && reason == "verified uncertain audio"
        ));

        assert!(
            Cli::try_parse_from([
                "melodee",
                "policy",
                "decide",
                id.to_string().as_str(),
                "delete",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "melodee",
                "policy",
                "decide",
                id.to_string().as_str(),
                "delete",
                "--actor",
                "local-admin",
                "--reason",
                "verified duplicate",
                "--complete",
            ])
            .is_err()
        );
    }
}
