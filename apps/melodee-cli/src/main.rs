use std::{collections::BTreeMap, env, io::Read, path::PathBuf, process::Command};

use clap::{Parser, Subcommand, ValueEnum};
use melodee_catalog::{CatalogConfig, import_once};
use melodee_domain::{IngestionState, ReleaseWorkItem};
use melodee_ingestion::{IngestionConfig, publish_once, reconcile_all_sources, validate_once};
use melodee_persistence::{
    CatalogStore, MAX_CHART_CSV_BYTES, NativeChartImport, NativeDiscoveryStore,
    PolicyAdminOverrideRequest, PolicyStore, WorkEngineConfig, WorkItemStore, WorkerId, migrate,
};
use melodee_policy::{
    PolicyConfig, PolicyEngine, PolicyEvaluation, default_rule_set, evaluate_catalog_context,
};
use melodee_search::{
    SearchConfig, autocomplete_active, diagnostic, query_active, request_rebuild, run_rebuild_once,
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
    /// Phase 6 MQL, Tantivy projection, rebuild, and diagnostics.
    Search {
        #[command(subcommand)]
        command: SearchCommands,
    },
    /// Import a bounded chart CSV and link unique exact catalog matches.
    Charts {
        #[command(subcommand)]
        command: ChartCommands,
    },
    /// Podcast stuck-download and orphan recovery (lease-coordinated).
    Podcasts {
        #[command(subcommand)]
        command: PodcastCommands,
    },
    /// Inspect and durably pause/resume background worker pools.
    WorkerPools {
        #[command(subcommand)]
        command: WorkerPoolCommands,
    },
    /// User bootstrap and operator account recovery.
    Users {
        #[command(subcommand)]
        command: UserCommands,
    },
    /// Library access group management (mirrors the admin API semantics).
    Groups {
        #[command(subcommand)]
        command: GroupCommands,
    },
    /// Notification template administration and delivery test-send
    /// (mirrors the admin API semantics; fail-degraded and audited).
    Notifications {
        #[command(subcommand)]
        command: NotificationCommands,
    },
    /// Scripting hook administration: list registered scripts and evaluate
    /// one against a ctx mock in the real sandbox (mirrors the admin API;
    /// every test run is recorded in the script_runs audit).
    Scripts {
        #[command(subcommand)]
        command: ScriptCommands,
    },
    /// Server settings: list effective layered values (secrets always
    /// redacted), validate preflight, and export/import with preview
    /// (PRD-CONFIG-001/002, PRD-SETUP-001).
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
    /// List every known setting's effective value with its source layer.
    /// Secret values are always redacted — there is no show-secrets flag.
    List,
    /// Run the onboarding preflight checks and validate the effective
    /// configuration; exits non-zero when any required check fails.
    Validate,
    /// Export the database+defaults configuration layer as a JSON document
    /// (secrets redacted to a fixed marker, with a header warning).
    Export {
        /// Write to this path instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Import a configuration export document. Default is preview (diff
    /// only); pass --apply to write. Unknown keys are rejected unless
    /// --allow-unknown. The redaction marker is never accepted as a value.
    Import {
        /// Path to the export document.
        path: PathBuf,
        /// Apply the validated import (default: preview only).
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Admit keys outside the 190-key registry as custom rows.
        #[arg(long, default_value_t = false)]
        allow_unknown: bool,
    },
}

#[derive(Debug, Subcommand)]
enum NotificationCommands {
    /// Notification template operations.
    Templates {
        #[command(subcommand)]
        command: NotificationTemplateCommands,
    },
    /// Render a template and attempt a real delivery through the configured
    /// transport, printing the honestly recorded audit row (sent / failed /
    /// skipped) — including when email delivery is disabled.
    TestSend {
        /// Template key (e.g. user.password_reset).
        #[arg(long, value_parser = parse_nonblank)]
        template: String,
        /// Recipient email address.
        #[arg(long, value_parser = parse_nonblank)]
        to: String,
        /// Locale tag (defaults to en; missing locales fall back to en).
        #[arg(long)]
        locale: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum NotificationTemplateCommands {
    /// List stored notification templates (bounded), key then locale.
    List {
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

#[derive(Debug, Subcommand)]
enum ScriptCommands {
    /// List registered scripts (bounded), event then name.
    List {
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Evaluate a script against a ctx mock (JSON file) in the real sandbox
    /// and print the honestly recorded outcome. Without --source, every
    /// enabled script for the event runs in registry order with production
    /// short-circuit; test-runs never trip the circuit breaker.
    Test {
        /// Hook event name (e.g. directoryProcessingStart).
        #[arg(long, value_parser = parse_nonblank)]
        event: String,
        /// Path to the ctx mock JSON file.
        #[arg(long)]
        ctx: PathBuf,
        /// Ad-hoc script source file; omit to evaluate the stored scripts.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Wall-clock budget override in milliseconds (bounded by the
        /// registry ceiling of 10000; ad-hoc default 250).
        #[arg(long)]
        timeout_ms: Option<i32>,
    },
}

#[derive(Debug, Subcommand)]
enum WorkerPoolCommands {
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

#[derive(Debug, Subcommand)]
enum UserCommands {
    /// Create the first administrator. Fails closed when any admin already
    /// exists. The password is read from the
    /// MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD environment variable — never
    /// from a command-line argument (shell history is not a secret store).
    BootstrapAdmin {
        #[arg(long, value_parser = parse_nonblank)]
        username: String,
        #[arg(long, value_parser = parse_nonblank)]
        email: String,
    },
    /// Disable a user (operator recovery): the account stops authenticating
    /// immediately and every refresh family is revoked atomically.
    Disable {
        /// Login username (normalized like sign-in).
        username: String,
    },
    /// Re-enable a previously disabled user.
    Enable {
        /// Login username (normalized like sign-in).
        username: String,
    },
}

#[derive(Debug, Subcommand)]
enum GroupCommands {
    /// List access groups with member and library-binding counts.
    List,
    /// Create an access group. Fails closed on an exact name conflict.
    Create {
        #[arg(value_parser = parse_nonblank)]
        name: String,
    },
    /// Delete an access group. Fails closed while members or library
    /// bindings remain — remove them first.
    Delete {
        #[arg(value_parser = parse_nonblank)]
        name: String,
    },
    /// Add a user to an access group (idempotent).
    AddUser {
        #[arg(value_parser = parse_nonblank)]
        group: String,
        #[arg(long, value_parser = parse_nonblank)]
        username: String,
    },
    /// Remove a user from an access group.
    RemoveUser {
        #[arg(value_parser = parse_nonblank)]
        group: String,
        #[arg(long, value_parser = parse_nonblank)]
        username: String,
    },
    /// Bind a library to a group. Once any group binds a library, only
    /// members of a bound group can access it.
    BindLibrary {
        #[arg(value_parser = parse_nonblank)]
        group: String,
        #[arg(long)]
        library_id: Uuid,
    },
    /// Remove a library binding from a group.
    UnbindLibrary {
        #[arg(value_parser = parse_nonblank)]
        group: String,
        #[arg(long)]
        library_id: Uuid,
    },
}

#[derive(Debug, Subcommand)]
enum PodcastCommands {
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

#[derive(Debug, Subcommand)]
enum ChartCommands {
    /// Atomically create a visible chart from Rank,ArtistName,ReleaseTitle,ReleaseYear CSV rows.
    Import {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, value_parser = parse_nonblank)]
        slug: String,
        #[arg(long, value_parser = parse_nonblank)]
        title: String,
        #[arg(long)]
        source_name: Option<String>,
        #[arg(long)]
        source_url: Option<String>,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long, default_value_t = false)]
        hidden: bool,
        #[arg(long, default_value_t = false)]
        generated_playlist: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SearchCommands {
    /// Show durable backlog, active generation, and on-disk health.
    Status,
    /// Build and atomically activate a fresh generation from PostgreSQL.
    Rebuild {
        /// Stable operator identity recorded with the durable request.
        #[arg(long, value_parser = parse_nonblank)]
        actor: String,
    },
    /// Execute a bounded MQL query against the active generation.
    Query {
        /// MQL expression.
        expression: String,
        /// Result limit. Defaults to MELODEE__SEARCH__DEFAULT_LIMIT.
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Return bounded deterministic prefix suggestions.
    Autocomplete {
        /// artist, release, or track.
        kind: String,
        prefix: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Parse MQL and print the pure AST as JSON.
    Parse { expression: String },
    /// Parse, validate, and print the framework-free query plan.
    Explain { expression: String },
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
            println!("Phase 6: complete — see design/evidence/phase-6/PHASE-6-REPORT.md");
            println!("Phase 7: complete — see design/evidence/phase-7/PHASE-7-REPORT.md");
            println!("Phase 8: active — see IMPLEMENTATION.md");
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
                        None,
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
        Commands::Search { command } => {
            let mut config = SearchConfig::from_env().unwrap_or_else(|error| {
                eprintln!("error: invalid search configuration: {error}");
                std::process::exit(2);
            });
            if !matches!(
                &command,
                SearchCommands::Parse { .. } | SearchCommands::Explain { .. }
            ) {
                let ingestion = IngestionConfig::from_env().unwrap_or_else(|error| {
                    eprintln!("error: invalid ingestion configuration: {error}");
                    std::process::exit(2);
                });
                let mut other_roots = ingestion
                    .sources
                    .iter()
                    .map(|source| source.root.clone())
                    .collect::<Vec<_>>();
                if let Some(roots) = ingestion.publication_roots().unwrap_or_else(|error| {
                    eprintln!("error: invalid publication configuration: {error}");
                    std::process::exit(2);
                }) {
                    other_roots.push(roots.storage_root);
                    other_roots.push(roots.work_area_base);
                }
                config.search_root =
                    config
                        .resolve_root_against(&other_roots)
                        .unwrap_or_else(|error| {
                            eprintln!("error: {}", error.operator_message());
                            std::process::exit(2);
                        });
            }
            match command {
                SearchCommands::Parse { expression } => {
                    let ast = melodee_mql::parse_with_bounds(&expression, &config.mql_bounds())
                        .unwrap_or_else(|error| {
                            eprintln!("error: {error}");
                            std::process::exit(2);
                        });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&ast).unwrap_or_else(|error| {
                            eprintln!("error: cannot serialize MQL AST: {error}");
                            std::process::exit(1);
                        })
                    );
                }
                SearchCommands::Explain { expression } => {
                    let plan =
                        melodee_mql::parse_and_plan_with_bounds(&expression, &config.mql_bounds())
                            .unwrap_or_else(|error| {
                                eprintln!("error: {error}");
                                std::process::exit(2);
                            });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&plan).unwrap_or_else(|error| {
                            eprintln!("error: cannot serialize MQL plan: {error}");
                            std::process::exit(1);
                        })
                    );
                }
                SearchCommands::Status => {
                    let pool = pool().await;
                    let status = diagnostic(&config, &pool).await.unwrap_or_else(|error| {
                        eprintln!("error: {}", error.operator_message());
                        std::process::exit(1);
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&status).unwrap_or_else(|error| {
                            eprintln!("error: cannot serialize search status: {error}");
                            std::process::exit(1);
                        })
                    );
                }
                SearchCommands::Rebuild { actor } => {
                    let pool = pool().await;
                    let job = request_rebuild(&config, &pool, &actor)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("error: {}", error.operator_message());
                            std::process::exit(1);
                        });
                    println!("rebuild requested: {}", job.job_id);
                    let completed =
                        run_rebuild_once(&config, &pool)
                            .await
                            .unwrap_or_else(|error| {
                                eprintln!(
                                    "error: rebuild {} failed: {}",
                                    job.job_id,
                                    error.operator_message()
                                );
                                std::process::exit(1);
                            });
                    println!("rebuild activated: {}", completed.unwrap_or(job.job_id));
                }
                SearchCommands::Query {
                    expression,
                    limit,
                    offset,
                } => {
                    let pool = pool().await;
                    let hits = query_active(
                        &config,
                        &pool,
                        &expression,
                        limit.unwrap_or(config.default_limit),
                        offset,
                    )
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("error: {}", error.operator_message());
                        std::process::exit(1);
                    });
                    for hit in hits {
                        println!(
                            "{}\t{}\t{}\t{}",
                            hit.kind, hit.internal_id, hit.public_id, hit.display
                        );
                    }
                }
                SearchCommands::Autocomplete {
                    kind,
                    prefix,
                    limit,
                } => {
                    let pool = pool().await;
                    let values = autocomplete_active(&config, &pool, &kind, &prefix, limit)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("error: {}", error.operator_message());
                            std::process::exit(1);
                        });
                    for value in values {
                        println!("{value}");
                    }
                }
            }
        }
        Commands::Charts { command } => match command {
            ChartCommands::Import {
                file,
                slug,
                title,
                source_name,
                source_url,
                year,
                hidden,
                generated_playlist,
            } => {
                let mut input = Vec::new();
                let file_handle = std::fs::File::open(&file).unwrap_or_else(|error| {
                    eprintln!("error: cannot open chart CSV {}: {error}", file.display());
                    std::process::exit(2);
                });
                let read_limit = u64::try_from(MAX_CHART_CSV_BYTES + 1).unwrap_or(u64::MAX);
                file_handle
                    .take(read_limit)
                    .read_to_end(&mut input)
                    .unwrap_or_else(|error| {
                        eprintln!("error: cannot read chart CSV {}: {error}", file.display());
                        std::process::exit(2);
                    });
                if input.len() > MAX_CHART_CSV_BYTES {
                    eprintln!("error: chart CSV exceeds {MAX_CHART_CSV_BYTES} bytes");
                    std::process::exit(2);
                }
                let pool = pool().await;
                migrate(&pool).await.unwrap_or_else(|error| {
                    eprintln!("error: migration failed: {error}");
                    std::process::exit(1);
                });
                let result = NativeDiscoveryStore
                    .import_chart_csv(
                        &pool,
                        &NativeChartImport {
                            slug,
                            title,
                            source_name,
                            source_url,
                            chart_year: year,
                            is_visible: !hidden,
                            is_generated_playlist_enabled: generated_playlist,
                        },
                        &input,
                    )
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    });
                println!("chart_id:    {}", result.chart_id);
                println!("items:       {}", result.item_count);
                println!("linked:      {}", result.linked_count);
                println!("unlinked:    {}", result.item_count - result.linked_count);
            }
        },
        Commands::Podcasts { command } => {
            let pool = pool().await;
            match command {
                PodcastCommands::Stuck { limit } => {
                    match melodee_server::podcast_worker::list_stuck_downloads(
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
                    let recovered = match melodee_server::podcast_worker::recover_stuck_downloads(
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
                    let podcast_config = melodee_server::podcast_worker::PodcastConfig::from_env()
                        .unwrap_or_else(|error| {
                            eprintln!("error: invalid podcast configuration: {error}");
                            std::process::exit(2);
                        });
                    if podcast_config.storage_root.is_some() {
                        if let Err(error) = podcast_config.require_owned_root() {
                            eprintln!(
                                "error: podcast storage root failed ownership verification: {error}"
                            );
                            std::process::exit(2);
                        }
                        match melodee_server::podcast_worker::reconcile_owned_files(
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
        Commands::WorkerPools { command } => run_worker_pool_command(&command).await,
        Commands::Users { command } => run_user_command(&command).await,
        Commands::Groups { command } => run_group_command(&command).await,
        Commands::Notifications { command } => run_notification_command(&command).await,
        Commands::Scripts { command } => run_script_command(&command).await,
        Commands::Config { command } => run_config_command(&command).await,
    }
}

/// Configuration commands share the server's stores and onboarding
/// preflight (PRD-CLI-002: never bypass safety), mirroring the admin API
/// semantics DB-direct (the CLI authenticates as the database role, not via
/// HTTP). Secret values are never printed: listings and exports use the
/// fixed redaction marker, and import plans name secret keys only.
async fn run_config_command(command: &ConfigCommands) {
    let pool = pool().await;
    match command {
        ConfigCommands::List => {
            let resolver =
                melodee_persistence::SettingsResolver::from_env(&pool).unwrap_or_else(|error| {
                    eprintln!("error: {error}");
                    std::process::exit(2);
                });
            match resolver.resolve_all().await {
                Ok(settings) => {
                    for setting in &settings {
                        let value = match &setting.value {
                            None => "-".to_string(),
                            Some(_) => setting.to_redacted_json().to_string(),
                        };
                        println!(
                            "{:<52} {:<12} {}",
                            setting.key,
                            setting.source.as_str(),
                            value
                        );
                    }
                    println!("-- {} setting(s)", settings.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        ConfigCommands::Validate => {
            let report = melodee_server::onboarding::evaluate_onboarding(&pool).await;
            for check in &report.checks {
                println!(
                    "{:<14} {:<8} {}{}",
                    check.name,
                    if check.passed { "pass" } else { "FAIL" },
                    check.detail,
                    check
                        .remediation
                        .as_ref()
                        .map(|hint| format!(" — {hint}"))
                        .unwrap_or_default(),
                );
            }
            println!(
                "onboarding: completed={} bypassed={} can_complete={}",
                report.completed, report.bypassed, report.can_complete
            );
            if !report.can_complete {
                eprintln!("error: required preflight checks are failing");
                std::process::exit(1);
            }
        }
        ConfigCommands::Export { output } => {
            let document =
                match melodee_persistence::export_config(&pool, OffsetDateTime::now_utc()).await {
                    Ok(document) => document,
                    Err(error) => {
                        eprintln!("error: {error}");
                        std::process::exit(1);
                    }
                };
            let rendered = serde_json::to_string_pretty(&document).unwrap_or_else(|error| {
                eprintln!("error: cannot serialize export document: {error}");
                std::process::exit(1);
            });
            eprintln!("warning: {}", melodee_persistence::CONFIG_EXPORT_WARNING);
            match output {
                Some(path) => {
                    if let Err(error) = std::fs::write(path, format!("{rendered}\n")) {
                        eprintln!("error: cannot write {}: {error}", path.display());
                        std::process::exit(1);
                    }
                    println!(
                        "exported {} setting(s) to {}",
                        document.settings.len(),
                        path.display()
                    );
                }
                None => println!("{rendered}"),
            }
        }
        ConfigCommands::Import {
            path,
            apply,
            allow_unknown,
        } => {
            let raw = std::fs::read(path).unwrap_or_else(|error| {
                eprintln!("error: cannot read {}: {error}", path.display());
                std::process::exit(2);
            });
            if raw.len() > melodee_persistence::MAX_IMPORT_DOCUMENT_BYTES {
                eprintln!(
                    "error: {} exceeds {} bytes",
                    path.display(),
                    melodee_persistence::MAX_IMPORT_DOCUMENT_BYTES
                );
                std::process::exit(2);
            }
            let document: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_else(|_| {
                eprintln!("error: {} is not valid JSON", path.display());
                std::process::exit(2);
            });
            let store = melodee_persistence::SettingsStore::new();
            let stored = store.list_raw(&pool).await.unwrap_or_else(|error| {
                eprintln!("error: {error}");
                std::process::exit(1);
            });
            let plan = melodee_persistence::plan_config_import(&document, &stored).unwrap_or_else(
                |error| {
                    eprintln!("error: {error}");
                    std::process::exit(2);
                },
            );
            let print_plan = |plan: &melodee_persistence::ConfigImportPlan| {
                println!("added:             {:?}", plan.added);
                println!("changed:           {:?}", plan.changed);
                println!("unchanged:         {}", plan.unchanged.len());
                println!("secrets_reentered: {:?}", plan.secrets_reentered);
                println!("secrets_skipped:   {:?}", plan.secrets_skipped);
                println!("unknown:           {:?}", plan.unknown);
                for warning in &plan.warnings {
                    println!("warning: {warning}");
                }
            };
            if !apply {
                print_plan(&plan);
                println!("preview only; pass --apply to write these settings");
                return;
            }
            match melodee_persistence::apply_config_import(
                &pool,
                &document,
                &plan,
                *allow_unknown,
                "melodee-cli",
                OffsetDateTime::now_utc(),
            )
            .await
            {
                Ok(()) => {
                    print_plan(&plan);
                    println!("import applied (audit recorded in onboarding evidence)");
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Notification commands go through the same stores and delivery service as
/// the server (PRD-CLI-002: never bypass safety), mirroring the admin API
/// semantics. Email settings resolve through the WP-08 layered sources
/// (env/config-file over `server_settings` over defaults); a disabled
/// transport records an honest `skipped` delivery instead of failing the
/// command (PRD-NOTIF-002/003).
async fn run_notification_command(command: &NotificationCommands) {
    let pool = pool().await;
    match command {
        NotificationCommands::Templates {
            command: NotificationTemplateCommands::List { limit },
        } => {
            match melodee_persistence::NotificationTemplateStore::new()
                .list(&pool, *limit, 0)
                .await
            {
                Ok(templates) => {
                    for template in &templates {
                        println!(
                            "{:<24} {:<8} v{} subject={:?} updated={}",
                            template.template_key,
                            template.locale,
                            template.version,
                            template.subject.as_deref().unwrap_or("-"),
                            template.updated_at,
                        );
                    }
                    println!("-- {} template(s)", templates.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        NotificationCommands::TestSend {
            template,
            to,
            locale,
        } => {
            let settings_resolver = melodee_persistence::SettingsResolver::from_env(&pool)
                .unwrap_or_else(|error| {
                    eprintln!("error: invalid settings configuration: {error}");
                    std::process::exit(2);
                });
            let settings =
                melodee_server::notifications::EmailSettings::from_resolved(&settings_resolver)
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("error: invalid email configuration: {error}");
                        std::process::exit(2);
                    });
            let service =
                melodee_server::notifications::NotificationService::new(pool.clone(), &settings)
                    .unwrap_or_else(|error| {
                        eprintln!("error: invalid email configuration: {error}");
                        std::process::exit(2);
                    });
            match service
                .send_templated(template, locale.as_deref().unwrap_or("en"), to.trim(), &[])
                .await
            {
                Ok(delivery) => {
                    println!("delivery_id: {}", delivery.id);
                    println!("template:    {}", delivery.template_key);
                    println!("recipient:   {}", delivery.recipient);
                    println!("status:      {}", delivery.status);
                    println!("error:       {}", delivery.error.as_deref().unwrap_or("-"));
                    println!("created_at:  {}", delivery.created_at);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// WP-11 scripting hook operations (PRD-SCRIPT-001/004, ADR-0135): DB-direct
/// over the same stores and sandboxed executor as the admin API. Outcomes —
/// including error/timeout fail-degraded results — are data, never command
/// failures (exit 1 is reserved for store failures, exit 2 for usage).
async fn run_script_command(command: &ScriptCommands) {
    let pool = pool().await;
    match command {
        ScriptCommands::List { limit } => {
            match melodee_persistence::ScriptRegistryStore::new()
                .list(&pool, *limit, 0)
                .await
            {
                Ok(scripts) => {
                    for script in &scripts {
                        let wired = if melodee_server::scripting::WIRED_EVENTS
                            .contains(&script.event.as_str())
                        {
                            "wired"
                        } else {
                            "inert"
                        };
                        println!(
                            "{:<28} {:<28} v{} enabled={} timeout={}ms {}",
                            script.event,
                            script.name,
                            script.version,
                            script.enabled,
                            script.timeout_ms,
                            wired,
                        );
                    }
                    println!("-- {} script(s)", scripts.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        ScriptCommands::Test {
            event,
            ctx,
            source,
            timeout_ms,
        } => {
            let ctx = read_json_file(ctx);
            let service = melodee_server::scripting::ScriptingService::new(pool.clone());
            match source {
                Some(source_path) => {
                    let source = read_source_file(source_path);
                    let timeout =
                        timeout_ms.unwrap_or(melodee_server::scripting::DEFAULT_TIMEOUT_MS);
                    match service.test_run(event, None, &source, timeout, &ctx).await {
                        Ok((evaluation, run)) => {
                            println!(
                                "outcome:     {} (ad-hoc source)",
                                evaluation.outcome.as_str()
                            );
                            println!(
                                "message:     {}",
                                evaluation.message.as_deref().unwrap_or("-")
                            );
                            println!(
                                "error:       {}",
                                evaluation.error.as_deref().unwrap_or("-")
                            );
                            println!("duration_ms: {}", evaluation.duration_ms);
                            println!("run_id:      {}", run.id);
                        }
                        Err(error) => {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    let scripts = melodee_persistence::ScriptRegistryStore::new()
                        .list_enabled_for_event(&pool, event)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("error: {error}");
                            std::process::exit(1);
                        });
                    if scripts.is_empty() {
                        println!("no enabled scripts for event {event}; verdict: allow (default)");
                        return;
                    }
                    let mut verdict = "allow";
                    for script in &scripts {
                        let timeout = timeout_ms.unwrap_or(script.timeout_ms);
                        let result = service
                            .test_run(event, Some(script.id), &script.source, timeout, &ctx)
                            .await;
                        match result {
                            Ok((evaluation, run)) => {
                                println!(
                                    "script:      {} v{} ({})",
                                    script.name, script.version, script.id
                                );
                                println!(
                                    "outcome:     {} message={} error={} duration_ms={}",
                                    evaluation.outcome.as_str(),
                                    evaluation.message.as_deref().unwrap_or("-"),
                                    evaluation.error.as_deref().unwrap_or("-"),
                                    evaluation.duration_ms,
                                );
                                println!("run_id:      {}", run.id);
                                if evaluation.outcome == melodee_persistence::ScriptRunOutcome::Deny
                                {
                                    verdict = "deny";
                                    break;
                                }
                            }
                            Err(error) => {
                                eprintln!("error: {error}");
                                std::process::exit(1);
                            }
                        }
                    }
                    println!(
                        "verdict: {verdict} (effective onDeny: {})",
                        melodee_server::scripting::EFFECTIVE_ON_DENY
                    );
                }
            }
        }
    }
}

fn read_json_file(path: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(path).unwrap_or_else(|error| {
        eprintln!("error: cannot read {}: {error}", path.display());
        std::process::exit(2);
    });
    if bytes.len() > melodee_server::scripting::MAX_CTX_BYTES {
        eprintln!(
            "error: ctx file exceeds {} bytes",
            melodee_server::scripting::MAX_CTX_BYTES
        );
        std::process::exit(2);
    }
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        eprintln!("error: {} is not valid JSON: {error}", path.display());
        std::process::exit(2);
    })
}

fn read_source_file(path: &std::path::Path) -> String {
    let source = std::fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!("error: cannot read {}: {error}", path.display());
        std::process::exit(2);
    });
    if source.len() > melodee_persistence::MAX_SCRIPT_SOURCE_BYTES {
        eprintln!(
            "error: source file exceeds {} bytes",
            melodee_persistence::MAX_SCRIPT_SOURCE_BYTES
        );
        std::process::exit(2);
    }
    source
}

/// Environment variable carrying the bootstrap-admin password. It is never
/// accepted as a CLI argument: shell history and process listings are not a
/// secret store.
const ENV_BOOTSTRAP_ADMIN_PASSWORD: &str = "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD";

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

async fn run_worker_pool_command(command: &WorkerPoolCommands) {
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
            let paused = matches!(command, WorkerPoolCommands::Pause { .. });
            match store
                .set_paused(&pool, name, paused, by, OffsetDateTime::now_utc())
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

async fn run_user_command(command: &UserCommands) {
    let pool = pool().await;
    let store = melodee_persistence::AuthStore::new();
    match command {
        UserCommands::BootstrapAdmin { username, email } => {
            let password = env::var(ENV_BOOTSTRAP_ADMIN_PASSWORD)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| {
                    eprintln!(
                        "error: {ENV_BOOTSTRAP_ADMIN_PASSWORD} is not set; the bootstrap password is never a CLI argument"
                    );
                    std::process::exit(2);
                });
            let password_hash =
                melodee_auth::hash_password(password.as_bytes()).unwrap_or_else(|error| {
                    eprintln!("error: cannot hash bootstrap password: {error}");
                    std::process::exit(1);
                });
            let stream_signing_key =
                format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
            match store
                .bootstrap_admin(
                    &pool,
                    &melodee_persistence::BootstrapAdminRequest {
                        username,
                        email,
                        password_hash: &password_hash,
                        stream_signing_key: &stream_signing_key,
                        now: OffsetDateTime::now_utc(),
                    },
                )
                .await
            {
                Ok(user) => {
                    println!("bootstrap admin created:");
                    println!("  username:      {}", user.username);
                    println!("  public_api_id: {}", user.public_api_id);
                    println!("  is_admin:      {}", user.is_admin);
                    println!("  roles:         {:?}", user.roles);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        UserCommands::Disable { username } | UserCommands::Enable { username } => {
            let enabled = matches!(command, UserCommands::Enable { .. });
            let user = match resolve_user_public_id(&pool, username).await {
                Ok(user) => user,
                Err(message) => {
                    eprintln!("error: {message}");
                    std::process::exit(1);
                }
            };
            // The admin user-update path bumps auth_revision (fencing
            // in-flight issuance) and atomically revokes every refresh
            // family when the account becomes disabled.
            match melodee_persistence::UserAdminStore
                .update_user(
                    &pool,
                    user,
                    &melodee_persistence::NativeUserAdminUpdate {
                        enabled: Some(enabled),
                        ..melodee_persistence::NativeUserAdminUpdate::default()
                    },
                    OffsetDateTime::now_utc(),
                )
                .await
            {
                Ok(user) => println!(
                    "{}: {} (enabled={}, auth_revision={})",
                    if enabled { "enabled" } else { "disabled" },
                    user.username,
                    user.enabled,
                    user.auth_revision
                ),
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Resolves a login username to its stable public id without exposing any
/// credential material.
async fn resolve_user_public_id(pool: &PgPool, username: &str) -> Result<Uuid, String> {
    let normalized = melodee_persistence::normalize_login(username);
    if normalized.is_empty() {
        return Err("username must not be blank".to_owned());
    }
    match melodee_persistence::AuthStore::new()
        .user_for_auth(pool, melodee_persistence::AuthLookup::Username(&normalized))
        .await
    {
        Ok(Some(record)) => Ok(record.user.public_api_id),
        Ok(None) => Err(format!("no user named '{username}'")),
        Err(error) => Err(error.to_string()),
    }
}

async fn run_group_command(command: &GroupCommands) {
    let pool = pool().await;
    let store = melodee_persistence::GroupAdminStore;
    let now = OffsetDateTime::now_utc();
    let result: Result<String, melodee_persistence::GroupAdminError> = match command {
        GroupCommands::List => store.list_groups(&pool).await.map(|groups| {
            let mut out = String::new();
            for group in &groups {
                out.push_str(&format!(
                    "{}  {:<30} members={} bindings={} updated={}\n",
                    group.public_api_id,
                    group.name,
                    group.member_count,
                    group.binding_count,
                    group.updated_at,
                ));
            }
            out.push_str(&format!("-- {} group(s)", groups.len()));
            out
        }),
        GroupCommands::Create { name } => store
            .create_group(&pool, name, now)
            .await
            .map(|group| format!("created: {} ({})", group.name, group.public_api_id)),
        GroupCommands::Delete { name } => store
            .delete_group(&pool, name)
            .await
            .map(|()| format!("deleted: {name}")),
        GroupCommands::AddUser { group, username } => store
            .add_user(&pool, group, username, now)
            .await
            .map(|added| {
                if added {
                    format!("added {username} to {group}")
                } else {
                    format!("{username} is already a member of {group}")
                }
            }),
        GroupCommands::RemoveUser { group, username } => store
            .remove_user(&pool, group, username, now)
            .await
            .map(|removed| {
                if removed {
                    format!("removed {username} from {group}")
                } else {
                    format!("{username} was not a member of {group}")
                }
            }),
        GroupCommands::BindLibrary { group, library_id } => store
            .bind_library(&pool, group, *library_id, now)
            .await
            .map(|bound| {
                if bound {
                    format!("bound library {library_id} to {group}")
                } else {
                    format!("library {library_id} is already bound to {group}")
                }
            }),
        GroupCommands::UnbindLibrary { group, library_id } => store
            .unbind_library(&pool, group, *library_id, now)
            .await
            .map(|unbound| {
                if unbound {
                    format!("unbound library {library_id} from {group}")
                } else {
                    format!("library {library_id} was not bound to {group}")
                }
            }),
    };
    match result {
        Ok(message) => println!("{message}"),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
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
    fn podcast_recovery_commands_are_bounded_and_explicit() {
        let stuck = Cli::try_parse_from(["melodee", "podcasts", "stuck"])
            .expect("valid stuck listing command");
        assert!(matches!(
            stuck.command,
            Commands::Podcasts {
                command: PodcastCommands::Stuck { limit: 50 }
            }
        ));

        let recover = Cli::try_parse_from([
            "melodee",
            "podcasts",
            "recover",
            "--limit",
            "10",
            "--retry-failed",
        ])
        .expect("valid recovery command");
        assert!(matches!(
            recover.command,
            Commands::Podcasts {
                command: PodcastCommands::Recover {
                    limit: 10,
                    retry_failed: true
                }
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "podcasts", "recover"])
                .expect("recovery defaults")
                .command,
            Commands::Podcasts {
                command: PodcastCommands::Recover {
                    limit: 50,
                    retry_failed: false
                }
            }
        ));
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

    #[test]
    fn search_operator_and_mql_commands_are_bounded_and_explicit() {
        assert!(matches!(
            Cli::try_parse_from(["melodee", "search", "status"])
                .expect("search status")
                .command,
            Commands::Search {
                command: SearchCommands::Status
            }
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "search",
                "rebuild",
                "--actor",
                "search-admin"
            ])
            .expect("search rebuild")
            .command,
            Commands::Search {
                command: SearchCommands::Rebuild { actor }
            } if actor == "search-admin"
        ));
        assert!(Cli::try_parse_from(["melodee", "search", "rebuild", "--actor", "   "]).is_err());
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "search",
                "query",
                "artist:Björk",
                "--limit",
                "25",
                "--offset",
                "50"
            ])
            .expect("search query")
            .command,
            Commands::Search {
                command: SearchCommands::Query {
                    expression,
                    limit: Some(25),
                    offset: 50
                }
            } if expression == "artist:Björk"
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "search", "query", "artist:Björk"])
                .expect("search query with configured default")
                .command,
            Commands::Search {
                command: SearchCommands::Query { limit: None, .. }
            }
        ));
        for command in ["parse", "explain"] {
            assert!(Cli::try_parse_from(["melodee", "search", command, "year:1990..1999"]).is_ok());
        }
    }

    #[test]
    fn worker_pool_commands_require_pool_and_operator_identity() {
        assert!(matches!(
            Cli::try_parse_from(["melodee", "worker-pools", "list"])
                .expect("pool list")
                .command,
            Commands::WorkerPools {
                command: WorkerPoolCommands::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "worker-pools", "pause", "podcast", "--by", "ops-1"])
                .expect("pool pause")
                .command,
            Commands::WorkerPools {
                command: WorkerPoolCommands::Pause { pool, by }
            } if pool == "podcast" && by == "ops-1"
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "worker-pools", "resume", "policy", "--by", "ops-2"])
                .expect("pool resume")
                .command,
            Commands::WorkerPools {
                command: WorkerPoolCommands::Resume { pool, by }
            } if pool == "policy" && by == "ops-2"
        ));
        // The operator audit identity is mandatory and non-blank.
        assert!(Cli::try_parse_from(["melodee", "worker-pools", "pause", "podcast"]).is_err());
        assert!(
            Cli::try_parse_from(["melodee", "worker-pools", "pause", "podcast", "--by", "  "])
                .is_err()
        );
        // Pool names are machine identifiers, validated before any SQL runs.
        assert!(
            Cli::try_parse_from(["melodee", "worker-pools", "pause", "Podcast", "--by", "ops"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["melodee", "worker-pools", "pause", "1pool", "--by", "ops"])
                .is_err()
        );
    }

    #[test]
    fn user_bootstrap_and_recovery_commands_never_take_a_password_argument() {
        let bootstrap = Cli::try_parse_from([
            "melodee",
            "users",
            "bootstrap-admin",
            "--username",
            "admin",
            "--email",
            "admin@example.test",
        ])
        .expect("valid bootstrap command");
        assert!(matches!(
            bootstrap.command,
            Commands::Users {
                command: UserCommands::BootstrapAdmin { username, email }
            } if username == "admin" && email == "admin@example.test"
        ));
        // No positional/flag password surface exists to parse at all.
        assert!(
            Cli::try_parse_from([
                "melodee",
                "users",
                "bootstrap-admin",
                "--username",
                "admin",
                "--email",
                "admin@example.test",
                "--password",
                "secret",
            ])
            .is_err()
        );
        for verb in ["disable", "enable"] {
            assert!(matches!(
                Cli::try_parse_from(["melodee", "users", verb, "operator-user"])
                    .expect("valid recovery command")
                    .command,
                Commands::Users { .. }
            ));
        }
    }

    #[test]
    fn group_commands_cover_membership_and_library_bindings() {
        assert!(matches!(
            Cli::try_parse_from(["melodee", "groups", "list"])
                .expect("groups list")
                .command,
            Commands::Groups {
                command: GroupCommands::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "groups", "create", "Family"])
                .expect("groups create")
                .command,
            Commands::Groups {
                command: GroupCommands::Create { name }
            } if name == "Family"
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "groups", "add-user", "Family", "--username", "kid"])
                .expect("groups add-user")
                .command,
            Commands::Groups {
                command: GroupCommands::AddUser { group, username }
            } if group == "Family" && username == "kid"
        ));
        let library = Uuid::now_v7();
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "groups",
                "bind-library",
                "Family",
                "--library-id",
                &library.to_string(),
            ])
            .expect("groups bind-library")
            .command,
            Commands::Groups {
                command: GroupCommands::BindLibrary { group, library_id }
            } if group == "Family" && library_id == library
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "groups",
                "unbind-library",
                "Family",
                "--library-id",
                &library.to_string(),
            ])
            .expect("groups unbind-library")
            .command,
            Commands::Groups {
                command: GroupCommands::UnbindLibrary { .. }
            }
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "groups",
                "remove-user",
                "Family",
                "--username",
                "kid",
            ])
            .expect("groups remove-user")
            .command,
            Commands::Groups {
                command: GroupCommands::RemoveUser { .. }
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["melodee", "groups", "delete", "Family"])
                .expect("groups delete")
                .command,
            Commands::Groups {
                command: GroupCommands::Delete { name }
            } if name == "Family"
        ));
        // Blank names are rejected before any SQL runs.
        assert!(Cli::try_parse_from(["melodee", "groups", "create", "   "]).is_err());
        assert!(Cli::try_parse_from(["melodee", "groups", "delete", "   "]).is_err());
    }
}
