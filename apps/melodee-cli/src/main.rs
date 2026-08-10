use std::{env, process::Command};

use std::collections::HashMap;

use clap::{Parser, Subcommand};
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_ingestion::{IngestionConfig, reconcile_all_sources};
use melodee_persistence::{WorkEngineConfig, WorkItemStore, WorkerId, migrate};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "melodee",
    version,
    about = "Melodee Rust administrative CLI bootstrap"
)]
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
    /// Development-only inbound discovery and settling diagnostics.
    Ingest {
        #[command(subcommand)]
        command: IngestCommands,
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
    /// Development-only: run one reconciliation pass for configured inbound sources.
    Reconcile,
    /// Development-only: claim and inspect one bounded batch of Ready Releases.
    Inspect,
    /// Development-only: print the durable manifest for a work item.
    Manifest {
        /// Work item id or API key.
        id: Uuid,
    },
    /// Development-only: print a concise ingest queue summary.
    Status,
}

fn command_available(command: &str, version_argument: &str) -> bool {
    Command::new(command)
        .arg(version_argument)
        .output()
        .is_ok_and(|result| result.status.success())
}

/// The database URL comes from exactly one explicit key
/// (MELODEE__DATABASE__URL) or --database-url style flag equivalent. It is
/// never printed (may contain a password).
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
            println!("Current phase: Phase 1B — durable inbound discovery and settling");
            println!("Read: PRD.md, SPEC.md, IMPLEMENTATION.md, AGENTS.md");
            println!("Phase 0 baseline: git tag phase-0-baseline");
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
            let pool = pool().await;
            let store = WorkItemStore;
            let config = IngestionConfig::from_env().unwrap_or_else(|e| {
                eprintln!("error: invalid ingestion configuration: {e}");
                std::process::exit(2);
            });

            match command {
                IngestCommands::Reconcile => {
                    if config.sources.is_empty() {
                        println!("no inbound sources configured");
                        return;
                    }
                    let now = OffsetDateTime::now_utc();
                    let results = match reconcile_all_sources(&config, &store, &pool, now).await {
                        Ok(results) => results,
                        Err(error) => match error {
                            melodee_ingestion::IngestionError::Persistence(error) => {
                                eprintln!("store error: {error}");
                                std::process::exit(1);
                            }
                            melodee_ingestion::IngestionError::Config(message) => {
                                eprintln!("config error: {message}");
                                std::process::exit(2);
                            }
                            melodee_ingestion::IngestionError::Source(message) => {
                                eprintln!("source error: {message}");
                                std::process::exit(2);
                            }
                            melodee_ingestion::IngestionError::Snapshot(message) => {
                                eprintln!("snapshot error: {message}");
                                std::process::exit(2);
                            }
                            melodee_ingestion::IngestionError::Inspection(message) => {
                                eprintln!("inspection error: {message}");
                                std::process::exit(1);
                            }
                            melodee_ingestion::IngestionError::InspectionTask(message) => {
                                eprintln!("inspection task error: {message}");
                                std::process::exit(1);
                            }
                        },
                    };
                    for result in results {
                        print_reconcile_result(&result);
                    }
                    println!("reconciliation finished");
                }
                IngestCommands::Status => {
                    let items = match store.list(&pool, None, 10_000, 0).await {
                        Ok(items) => items,
                        Err(e) => {
                            eprintln!("error: {e}");
                            std::process::exit(1);
                        }
                    };

                    let mut states = HashMap::new();
                    for item in &items {
                        *states.entry(item.state).or_insert(0_u64) += 1;
                    }
                    println!("ingest work queue: {} total", items.len());
                    if states.is_empty() {
                        println!("no work items");
                    } else {
                        for (state, count) in states {
                            println!("  {:<10} {count}", state.as_str());
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
            }
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
