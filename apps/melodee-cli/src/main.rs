use std::{env, process::Command};

use clap::{Parser, Subcommand};
use melodee_domain::{IngestionState, ReleaseWorkItem};
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
            println!("Current phase: Phase 1A — durable Release work execution");
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
    }
}
