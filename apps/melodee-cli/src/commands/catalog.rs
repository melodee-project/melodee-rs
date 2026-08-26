//! PostgreSQL catalog import and diagnostics operator commands.

use clap::Subcommand;
use melodee_catalog::import_once;
use melodee_persistence::{CatalogStore, WorkEngineConfig, WorkItemStore};
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum CatalogCommands {
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

pub(crate) async fn run(command: CatalogCommands) {
    let catalog_config = melodee_catalog::CatalogConfig::from_env().unwrap_or_else(|error| {
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
