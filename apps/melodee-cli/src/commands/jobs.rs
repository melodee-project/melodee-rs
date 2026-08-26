//! Durable scheduled-job history inspection and audited retention cleanup.

use clap::Subcommand;
use melodee_persistence::JobRunHistoryStore;
use time::OffsetDateTime;

use super::common::{parse_nonblank, pool};

#[derive(Debug, Subcommand)]
pub(crate) enum JobCommands {
    /// List durable tick and manual job runs.
    History {
        /// Restrict the output to one job name.
        #[arg(long, value_parser = parse_nonblank)]
        job_name: Option<String>,
        /// Maximum rows to print.
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=500))]
        limit: u32,
        /// Number of rows to skip.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// Emit one machine-readable JSON document.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Delete completed job history older than the requested retention.
    Cleanup {
        /// Retention period in days.
        #[arg(long, default_value_t = 90, value_parser = clap::value_parser!(i32).range(1..=3650))]
        retention_days: i32,
        /// Stable operator identity recorded in the cleanup audit.
        #[arg(long, value_parser = parse_nonblank)]
        actor: String,
        /// Required confirmation for this destructive history operation.
        #[arg(long, default_value_t = false)]
        confirm: bool,
    },
}

pub(crate) async fn run(command: JobCommands) {
    let pool = pool().await;
    let store = JobRunHistoryStore;
    match command {
        JobCommands::History {
            job_name,
            limit,
            offset,
            json,
        } => match store.list(&pool, job_name.as_deref(), limit, offset).await {
            Ok((items, total)) if json => {
                let document = serde_json::json!({
                    "items": items,
                    "total": total,
                    "limit": limit,
                    "offset": offset,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&document).unwrap_or_else(|error| {
                        eprintln!("error: cannot serialize job history: {error}");
                        std::process::exit(1);
                    })
                );
            }
            Ok((items, total)) => {
                for item in items {
                    println!(
                        "{} {:<24} success={:?} manual={} started={} error={}",
                        item.id,
                        item.job_name,
                        item.success,
                        item.was_manual_trigger,
                        item.started_at,
                        item.error_message.as_deref().unwrap_or("-")
                    );
                }
                println!("-- {total} total row(s)");
            }
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        },
        JobCommands::Cleanup {
            retention_days,
            actor,
            confirm,
        } => {
            if !confirm {
                eprintln!("error: --confirm is required for job history cleanup");
                std::process::exit(2);
            }
            match store
                .cleanup(&pool, retention_days, &actor, OffsetDateTime::now_utc())
                .await
            {
                Ok(removed) => println!("removed {removed} completed job history row(s)"),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}
