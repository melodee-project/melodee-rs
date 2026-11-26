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
    /// Report or repair the genre serving aggregates from authoritative terms.
    ///
    /// Check mode (default) is read-only and reports the number of discrepant
    /// aggregate keys in a bounded page. Apply mode is idempotent and resumable
    /// and converges the stored song/distinct-release counts, including removing
    /// keys with no active contributors. Run it after applying the forward
    /// migrations and under the ADR-0166 maintenance fence, or with the same
    /// serialization fence live writers use (the repair takes the same advisory
    /// keys).
    RepairGenres {
        /// Apply corrections instead of only reporting them.
        #[arg(long, default_value_t = false)]
        apply: bool,
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
        CatalogCommands::RepairGenres { apply } => {
            let page: i32 = 1_000;
            if apply {
                let mut total_repaired: i64 = 0;
                loop {
                    let result: Result<(i32, i32), sqlx::Error> =
                        sqlx::query_as("SELECT * FROM repair_catalog_genre_aggregates(true, $1)")
                            .bind(page)
                            .fetch_one(&pool)
                            .await;
                    match result {
                        Ok((checked, repaired)) => {
                            total_repaired += i64::from(repaired);
                            if checked == 0 {
                                break;
                            }
                            println!(
                                "genre aggregate repair: checked={checked} repaired={repaired} total_repaired={total_repaired}"
                            );
                        }
                        Err(error) => {
                            eprintln!("error: genre aggregate repair failed: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                println!("genre aggregate repair complete: total_repaired={total_repaired}");
            } else {
                match sqlx::query_as::<_, (i32, i32)>(
                    "SELECT * FROM repair_catalog_genre_aggregates(false, $1)",
                )
                .bind(page)
                .fetch_one(&pool)
                .await
                {
                    Ok((checked, _)) => {
                        println!(
                            "genre aggregate check: discrepancies_in_page={checked} (page={page}); run with --apply to converge"
                        );
                        if checked == 0 {
                            println!("genre aggregates are consistent");
                        }
                    }
                    Err(error) => {
                        eprintln!("error: genre aggregate check failed: {error}");
                        std::process::exit(1);
                    }
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
