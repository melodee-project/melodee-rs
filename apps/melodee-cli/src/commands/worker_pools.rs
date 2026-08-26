//! Inspect and durably pause/resume background worker pools.

use clap::Subcommand;
use time::OffsetDateTime;

use super::common::{parse_nonblank, pool};

#[derive(Debug, Subcommand)]
pub(crate) enum WorkerPoolCommands {
    /// List every registered worker pool with its durable pause state and
    /// last-pause audit.
    List,
    /// Durably pause a pool: workers stop claiming new work between batches;
    /// in-flight claims finish or are released per the lease contract.
    Pause {
        /// Pool name (e.g. podcast, policy, discovery).
        #[arg(value_parser = parse_pool_name)]
        pool: String,
        /// Stable operator identity recorded with the pause audit.
        #[arg(long, value_parser = parse_nonblank)]
        by: String,
    },
    /// Durably resume a paused pool. The last-pause audit is retained.
    Resume {
        /// Pool name (e.g. podcast, policy, discovery).
        #[arg(value_parser = parse_pool_name)]
        pool: String,
        /// Stable operator identity recorded with the resume request.
        #[arg(long, value_parser = parse_nonblank)]
        by: String,
    },
}

fn parse_pool_name(raw: &str) -> Result<String, String> {
    if melodee_persistence::is_valid_pool_name(raw) {
        Ok(raw.to_owned())
    } else {
        Err("pool name must match ^[a-z][a-z0-9_-]{0,63}$".to_owned())
    }
}

fn print_pool_state(state: &melodee_persistence::WorkerPoolState) {
    println!(
        "{:<20} paused={:<5} paused_by={:?} paused_at={:?} resume_requested_at={:?} updated_at={}",
        state.pool,
        state.paused,
        state.paused_by.as_deref().unwrap_or("-"),
        state.paused_at,
        state.resume_requested_at,
        state.updated_at,
    );
}

pub(crate) async fn run(command: WorkerPoolCommands) {
    let paused = matches!(command, WorkerPoolCommands::Pause { .. });
    let pool = pool().await;
    let store = melodee_persistence::WorkerPoolStore;
    match command {
        WorkerPoolCommands::List => match store.list(&pool).await {
            Ok(pools) => {
                for state in &pools {
                    print_pool_state(state);
                }
                println!("-- {} pool(s)", pools.len());
            }
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        },
        WorkerPoolCommands::Pause { pool: name, by }
        | WorkerPoolCommands::Resume { pool: name, by } => {
            match store
                .set_paused(&pool, &name, paused, &by, OffsetDateTime::now_utc())
                .await
            {
                Ok(state) => {
                    print_pool_state(&state);
                    println!(
                        "{}: {} (in-flight claims finish or are released per the lease contract)",
                        if paused { "paused" } else { "resumed" },
                        state.pool
                    );
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}
