//! Podcast stuck-download and orphan recovery commands (lease-coordinated).

use clap::Subcommand;

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum PodcastCommands {
    /// List failed downloads and stuck (expired-lease) downloads, oldest first.
    Stuck {
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Requeue stuck downloads (expired leases only — a live worker's claim is
    /// never stolen) and reconcile orphan files under the configured, owned
    /// podcast storage root (MELODEE__PODCAST__STORAGE_ROOT).
    Recover {
        /// Maximum stuck rows to requeue in this pass (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Also requeue permanently Failed downloads, resetting their attempt
        /// counters so they get a fresh bounded retry budget.
        #[arg(long, default_value_t = false)]
        retry_failed: bool,
    },
}

pub(crate) async fn run(command: PodcastCommands) {
    let pool = pool().await;
    match command {
        PodcastCommands::Stuck { limit } => {
            match melodee_application::podcast_worker::list_stuck_downloads(
                &pool,
                i64::from(limit.min(500)),
            )
            .await
            {
                Ok(rows) => {
                    for row in &rows {
                        println!(
                            "{}  episode={} channel={} {:<11} attempts={} lease={} lease_expires={:?} error={:?} updated={}",
                            row.public_api_id,
                            row.episode_id,
                            row.channel_id,
                            row.download_status,
                            row.download_attempt_count,
                            row.download_lease_owner
                                .map(|worker| worker.to_string())
                                .unwrap_or_else(|| "-".to_owned()),
                            row.download_lease_expires_at,
                            row.download_error,
                            row.updated_at,
                        );
                        println!(
                            "    channel={:?} episode={:?}",
                            row.channel_title, row.episode_title
                        );
                    }
                    println!("-- {} stuck download(s)", rows.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        PodcastCommands::Recover {
            limit,
            retry_failed,
        } => {
            let recovered = match melodee_application::podcast_worker::recover_stuck_downloads(
                &pool,
                i64::from(limit.min(500)),
                retry_failed,
            )
            .await
            {
                Ok(recovered) => recovered,
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            };
            println!(
                "recovered {recovered} stuck download(s) back to Queued (retry_failed={retry_failed})"
            );
            let podcast_config = melodee_application::podcast_worker::PodcastConfig::from_env()
                .unwrap_or_else(|error| {
                    eprintln!("error: invalid podcast configuration: {error}");
                    std::process::exit(2);
                });
            if podcast_config.storage_root.is_some() {
                if let Err(error) = podcast_config.require_owned_root() {
                    eprintln!("error: podcast storage root failed ownership verification: {error}");
                    std::process::exit(2);
                }
                match melodee_application::podcast_worker::reconcile_owned_files(
                    &pool,
                    &podcast_config,
                )
                .await
                {
                    Ok(removed) => {
                        println!("orphan reconciliation: removed {removed} file(s)");
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!(
                    "orphan reconciliation: skipped (MELODEE__PODCAST__STORAGE_ROOT is not set)"
                );
            }
        }
    }
}
