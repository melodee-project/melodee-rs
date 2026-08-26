//! Doctor command: local prerequisite checks and deep operational health probes.

use std::{env, process::Command};

use melodee_ingestion::{ENV_STORAGE_ROOT, IngestionConfig};

use super::common::pool;

/// Basic local checks (always run) plus optional deep operational health
/// checks (database + search index).
pub(crate) async fn run_doctor(deep: bool) {
    let basic_checks = [
        ("ffmpeg", command_available("ffmpeg", "-version")),
        ("ffprobe", command_available("ffprobe", "-version")),
        ("fpcalc", command_available("fpcalc", "-version")),
        (
            melodee_persistence::ENV_DATABASE_URL,
            env::var_os(melodee_persistence::ENV_DATABASE_URL).is_some(),
        ),
    ];

    for (name, available) in basic_checks {
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
                ENV_STORAGE_ROOT
            ),
            Err(error) => println!("canonical publication: invalid ({error})"),
        },
        Err(error) => println!("canonical publication: invalid ({error})"),
    }

    // Deep operational health checks (database + search index).
    if deep {
        match run_deep_checks().await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("deep check error: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Deep operational health checks for the doctor command (R6.3).
/// Probes PostgreSQL connectivity, migration currency, and search index state.
async fn run_deep_checks() -> Result<(), String> {
    println!("--- Deep operational health checks ---");

    // 1. PostgreSQL connectivity + SELECT 1.
    let pool = pool().await;
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&pool),
    )
    .await
    {
        Ok(Ok(1)) => println!("postgresql: ok (connectivity verified)"),
        Ok(Ok(_)) => {
            return Err("postgresql: unexpected SELECT 1 result".to_string());
        }
        Ok(Err(e)) => {
            return Err(format!("postgresql: connectivity check failed ({e})"));
        }
        Err(_) => {
            return Err("postgresql: connectivity timed out (5s)".to_string());
        }
    }

    // 2. Migration currency check.
    match melodee_persistence::migration_status(&pool).await {
        Ok(status) => {
            if status.is_current() {
                println!(
                    "migrations: current (applied {} / {} available)",
                    status.applied, status.latest_available
                );
            } else {
                println!(
                    "migrations: pending (applied {} / {} available; run `melodee-cli migrate`)",
                    status.applied, status.latest_available
                );
            }
            if !status.all_successful {
                println!("migrations: note — some previous migrations failed");
            }
        }
        Err(e) => {
            eprintln!("migrations: cannot read tracking table ({e})");
        }
    }

    // 3. Catalog entity counts (sanity check the store is responsive).
    match melodee_persistence::CatalogStore.counts(&pool).await {
        Ok(counts) => println!(
            "catalog: artists={} releases={} tracks={} media_assets={}",
            counts.artists, counts.releases, counts.tracks, counts.media_assets
        ),
        Err(e) => eprintln!("catalog: count query failed ({e})"),
    }

    // 4. Search index diagnostic (if search is configured).
    match melodee_search::SearchConfig::from_env() {
        Ok(config) if config.enabled => {
            let timeout = std::time::Duration::from_secs(5);
            match tokio::time::timeout(timeout, melodee_search::diagnostic(&config, &pool)).await {
                Ok(Ok(diagnostic)) => {
                    if diagnostic.generation.is_some() && diagnostic.generation_error.is_none() {
                        println!("search: ok (active generation validated)");
                    } else {
                        println!(
                            "search: degraded — {}",
                            diagnostic
                                .generation_error
                                .unwrap_or_else(|| "no active generation".to_string())
                        );
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("search: diagnostic failed ({})", e.operator_message());
                }
                Err(_) => {
                    eprintln!("search: timed out ({}s)", timeout.as_secs());
                }
            }
        }
        _ => {
            println!("search: disabled (not configured)");
        }
    }

    // 5. PostgreSQL readiness (bounded, same contract as /readyz).
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&pool),
    )
    .await
    {
        Ok(Ok(1)) => println!("readiness: ready"),
        _ => eprintln!("readiness: not_ready (bounded probe failed)"),
    }

    Ok(())
}

fn command_available(command: &str, version_argument: &str) -> bool {
    Command::new(command)
        .arg(version_argument)
        .output()
        .is_ok_and(|result| result.status.success())
}
