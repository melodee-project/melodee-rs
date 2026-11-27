use clap::{Parser, Subcommand};

mod commands;
mod legacy;
mod user_state_import;

use commands::{
    catalog::CatalogCommands, charts::ChartCommands, config::ConfigCommands, groups::GroupCommands,
    ingest::IngestCommands, jobs::JobCommands, notifications::NotificationCommands,
    podcasts::PodcastCommands, policy::PolicyCommands, rollback::RollbackCommands,
    scripts::ScriptCommands, search::SearchCommands, users::UserCommands, work::WorkCommands,
    worker_pools::WorkerPoolCommands,
};

#[derive(Debug, Parser)]
#[command(name = "melodee", version, about = "Melodee Rust administrative CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Check locally available prerequisites without changing the system.
    /// Without flags runs basic checks (media tools, configuration). With
    /// `--deep` also probes the database (connectivity, migration currency,
    /// search index health) for operational health diagnostics.
    Doctor {
        /// Run deep operational health checks: PostgreSQL connectivity,
        /// migration currency, Tantivy search index diagnostic.
        #[arg(long, default_value_t = false)]
        deep: bool,
    },
    /// Apply database migrations (explicit SQL, from empty or current).
    ///
    /// Without flags this resumes the normal upgrade, including the independent
    /// manifest evidence check before finalization.
    Migrate(commands::migrate::MigrateArgs),
    /// Inspect durable Release work items.
    Work {
        #[command(subcommand)]
        command: WorkCommands,
    },
    /// ingestion worker operations and diagnostics.
    Ingest {
        #[command(subcommand)]
        command: IngestCommands,
    },
    /// PostgreSQL catalog import and diagnostics.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommands,
    },
    /// policy classification, novelty, and dry-run diagnostics.
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
    /// MQL, Tantivy projection, rebuild, and diagnostics.
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
    /// Inspect durable scheduled-job history and run audited retention cleanup.
    Jobs {
        #[command(subcommand)]
        command: JobCommands,
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
    /// legacy (C#) migration tooling: inventory, plan, import,
    /// verify, delta, and status against a read-only source snapshot
    /// (ADR-0148).
    Legacy {
        #[command(subcommand)]
        command: legacy::LegacyCommands,
    },
    /// Rollback orchestrator: guides the safe revert to the legacy system
    /// after a failed v1.0 cutover (ADR-0148 §D.9). Does not modify legacy
    /// code or infrastructure — produces audit evidence and documented steps.
    Rollback {
        #[command(subcommand)]
        command: RollbackCommands,
    },
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor { deep } => commands::doctor::run_doctor(deep).await,
        Commands::Migrate(args) => commands::migrate::run(args).await,
        Commands::Work { command } => commands::work::run(command).await,
        Commands::Ingest { command } => commands::ingest::run(command).await,
        Commands::Policy { command } => commands::policy::run(command).await,
        Commands::Catalog { command } => commands::catalog::run(command).await,
        Commands::Search { command } => commands::search::run(command).await,
        Commands::Charts { command } => commands::charts::run(command).await,
        Commands::Podcasts { command } => commands::podcasts::run(command).await,
        Commands::WorkerPools { command } => commands::worker_pools::run(command).await,
        Commands::Jobs { command } => commands::jobs::run(command).await,
        Commands::Users { command } => commands::users::run(command).await,
        Commands::Groups { command } => commands::groups::run(command).await,
        Commands::Notifications { command } => commands::notifications::run(command).await,
        Commands::Scripts { command } => commands::scripts::run(command).await,
        Commands::Config { command } => commands::config::run(command).await,
        Commands::Legacy { command } => {
            if let Err(message) = legacy::run(command, commands::common::pool().await).await {
                eprintln!("legacy migration error: {message}");
                std::process::exit(1);
            }
        }
        Commands::Rollback { command } => commands::rollback::run(command).await,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use commands::policy::{PolicyDispositionArg, require_policy_test_evaluation};
    use uuid::Uuid;

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
    fn legacy_path_review_commands_require_explicit_evidence() {
        let legacy = Uuid::now_v7().to_string();
        let reviewed = Uuid::now_v7().to_string();
        let cli = Cli::try_parse_from([
            "melodee",
            "ingest",
            "resolve-legacy-path",
            "--legacy",
            &legacy,
            "--reviewed",
            &reviewed,
            "--receipt-tree-hash",
            "0",
            "--actor",
            "operator",
            "--reason",
            "reviewed",
        ])
        .expect("valid review arguments");
        assert!(matches!(
            cli.command,
            Commands::Ingest {
                command: IngestCommands::ResolveLegacyPath { .. }
            }
        ));
        assert!(
            Cli::try_parse_from([
                "melodee",
                "ingest",
                "resolve-legacy-path",
                "--legacy",
                &legacy
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["melodee", "ingest", "legacy-paths"]).is_ok());
    }

    #[test]
    fn purge_guard_recovery_requires_exact_fence_and_all_attestations() {
        let id = Uuid::now_v7().to_string();
        let token = Uuid::now_v7().to_string();
        assert!(Cli::try_parse_from(["melodee", "ingest", "purge-guards"]).is_ok());
        let args = [
            "melodee",
            "ingest",
            "recover-purge-guard",
            "--work-item-id",
            &id,
            "--fence-token",
            &token,
            "--guard-owner",
            "worker-1",
            "--actor",
            "ops",
            "--reason",
            "backup and media inspection complete",
            "--stopped-all-writers",
            "--inspected-media",
            "--backup-verified",
            "--confirm",
        ];
        assert!(matches!(
            Cli::try_parse_from(args)
                .expect("complete operator review")
                .command,
            Commands::Ingest {
                command: IngestCommands::RecoverPurgeGuard { .. }
            }
        ));
        for flag in [
            "--stopped-all-writers",
            "--inspected-media",
            "--backup-verified",
            "--confirm",
        ] {
            assert!(Cli::try_parse_from(args.into_iter().filter(|arg| *arg != flag)).is_err());
        }
        for flag in [
            "--work-item-id",
            "--fence-token",
            "--guard-owner",
            "--actor",
            "--reason",
        ] {
            let position = args
                .iter()
                .position(|arg| *arg == flag)
                .expect("flag present");
            assert!(
                Cli::try_parse_from(args.into_iter().enumerate().filter_map(|(index, arg)| {
                    (index != position && index != position + 1).then_some(arg)
                }))
                .is_err()
            );
        }
        let mut blank_reason = args;
        blank_reason[12] = "   ";
        assert!(Cli::try_parse_from(blank_reason).is_err());
        let mut exact_owner = args;
        exact_owner[8] = " worker-1 ";
        assert!(matches!(
            Cli::try_parse_from(exact_owner).expect("owner kept byte-for-byte").command,
            Commands::Ingest {
                command: IngestCommands::RecoverPurgeGuard { guard_owner, .. }
            } if guard_owner == " worker-1 "
        ));
    }

    #[test]
    fn retained_obligation_listing_is_bounded_and_keyset_paginated() {
        assert!(matches!(
            Cli::try_parse_from(["melodee", "ingest", "retained-obligations"])
                .expect("default retained listing")
                .command,
            Commands::Ingest {
                command: IngestCommands::RetainedObligations {
                    after: None,
                    limit: 100
                }
            }
        ));
        let after = Uuid::now_v7();
        assert!(matches!(
            Cli::try_parse_from(["melodee", "ingest", "retained-obligations", "--after", &after.to_string(), "--limit", "128"])
                .expect("bounded retained page").command,
            Commands::Ingest {
                command: IngestCommands::RetainedObligations { after: Some(parsed), limit: 128 }
            } if parsed == after
        ));
        for limit in ["0", "129", "-1"] {
            assert!(
                Cli::try_parse_from([
                    "melodee",
                    "ingest",
                    "retained-obligations",
                    "--limit",
                    limit
                ])
                .is_err()
            );
        }
        assert!(
            Cli::try_parse_from([
                "melodee",
                "ingest",
                "retained-obligations",
                "--after",
                "not-a-uuid"
            ])
            .is_err()
        );
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
    fn work_retry_parses_id_and_optional_reason() {
        let id = Uuid::now_v7();
        assert!(matches!(
            Cli::try_parse_from(["melodee", "work", "retry", &id.to_string()])
                .expect("retry without reason")
                .command,
            Commands::Work {
                command: WorkCommands::Retry { id: parsed, reason: None }
            } if parsed == id
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "melodee",
                "work",
                "retry",
                &id.to_string(),
                "--reason",
                "tags refreshed after rescan"
            ])
            .expect("retry with reason")
            .command,
            Commands::Work {
                command: WorkCommands::Retry {
                    id: parsed,
                    reason: Some(reason)
                }
            } if parsed == id && reason == "tags refreshed after rescan"
        ));
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
    fn user_state_import_defaults_to_partial_catalog_matching_with_optional_strict_mode() {
        let command = ["melodee", "users", "import-state", "--input", "export"];
        assert!(matches!(
            Cli::try_parse_from(command)
                .expect("default import")
                .command,
            Commands::Users {
                command: UserCommands::ImportState {
                    require_all_matches: false,
                    allow_unmatched: false,
                    no_create_users: false,
                    ..
                }
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(command.into_iter().chain(["--require-all-matches"]))
                .expect("strict import")
                .command,
            Commands::Users {
                command: UserCommands::ImportState {
                    require_all_matches: true,
                    ..
                }
            }
        ));
        assert!(
            Cli::try_parse_from(
                command
                    .into_iter()
                    .chain(["--require-all-matches", "--allow-unmatched"])
            )
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
