//! Durable Release work-item inspection and operator recovery commands.

use clap::Subcommand;
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_persistence::{WorkEngineConfig, WorkItemStore};
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum WorkCommands {
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
    /// Requeue a failed or quarantined work item for processing. Uses the
    /// identical operator state-machine arm as the Admin UI reprocess
    /// endpoint (`failed|quarantined -> ready`, audited).
    Retry {
        /// Work item id (UUID).
        id: Uuid,
        /// Audited operator reason (defaults to "operator reprocess").
        #[arg(long)]
        reason: Option<String>,
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

pub(crate) async fn run(command: WorkCommands) {
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
        WorkCommands::Retry { id, reason } => {
            let item = match store.get(&pool, id).await {
                Ok(item) => item,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
            if !matches!(
                item.state,
                IngestionState::Failed | IngestionState::Quarantined
            ) {
                eprintln!(
                    "error: work item is '{}'; only failed or quarantined items can be reprocessed",
                    item.state.as_str()
                );
                std::process::exit(2);
            }
            let reason = reason
                .map(|reason| reason.trim().to_string())
                .filter(|reason| !reason.is_empty());
            let now = OffsetDateTime::now_utc();
            match store
                .transition(
                    &pool,
                    item.id,
                    None,
                    melodee_domain::TransitionKind::Operator,
                    IngestionState::Ready,
                    Some(reason.as_deref().unwrap_or("operator reprocess")),
                    now,
                )
                .await
            {
                Ok(updated) => {
                    println!(
                        "requeued {} ({} -> ready): {}",
                        updated.id,
                        item.state.as_str(),
                        updated.source_path
                    );
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        WorkCommands::SeedTest {
            count,
            i_know_what_im_doing,
        } => {
            if !i_know_what_im_doing {
                eprintln!("error: seed-test writes synthetic data; pass --i-know-what-im-doing");
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
                eprintln!("error: claim-test mutates lease state; pass --i-know-what-im-doing");
                std::process::exit(2);
            }
            let worker = melodee_persistence::WorkerId::new();
            let now = OffsetDateTime::now_utc();
            let config = WorkEngineConfig {
                claim_batch_size: count.max(1),
                ..config
            };
            match store.claim(&pool, &worker, &config, now).await {
                Ok(items) => {
                    for item in &items {
                        print_item(item);
                        if let Err(e) = store.release_lease(&pool, item.id, &worker, now).await {
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
