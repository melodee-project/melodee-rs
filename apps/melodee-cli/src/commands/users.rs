//! User bootstrap and operator account recovery commands.

use std::env;
use std::path::PathBuf;

use clap::Subcommand;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::{parse_nonblank, pool};

/// Environment variable carrying the bootstrap-admin password. It is never
/// accepted as a CLI argument: shell history and process listings are not a
/// secret store.
const ENV_BOOTSTRAP_ADMIN_PASSWORD: &str = "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD";

#[derive(Debug, Subcommand)]
pub(crate) enum UserCommands {
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
    /// Import portable user state (likes, dislikes, ratings, pins, play
    /// counts, play history, playlists, smart playlists, bookmarks, groups,
    /// and playback preferences) exported from a C# Melodee snapshot by
    /// `scripts/export_legacy_user_state.py` (ADR-0200).
    ///
    /// The import is additive: it never deletes rows, older exports cannot
    /// downgrade newer local changes, play counts are monotonic, and playlist
    /// items are only appended. Missing music is skipped and reported. Re-run
    /// the same export after adding music to import newly matched state.
    ImportState {
        /// Export directory containing manifest.json and the NDJSON streams.
        #[arg(long, value_name = "DIR")]
        input: PathBuf,
        /// Validate, resolve identities, and print the plan without writing.
        #[arg(long)]
        dry_run: bool,
        /// Skip missing users with --no-create-users. Missing catalog entities
        /// are always skipped and reported by default.
        #[arg(long)]
        allow_unmatched: bool,
        /// Fail if any catalog entity is missing or not live, before writing.
        #[arg(long, conflicts_with = "allow_unmatched")]
        require_all_matches: bool,
        /// Do not create missing accounts; every exported user must already
        /// exist. Creating missing accounts is the default.
        #[arg(long = "no-create-users")]
        no_create_users: bool,
        /// Keep the exported administrator/editor flags on newly created
        /// accounts. Off by default: existing accounts never change privilege,
        /// and a created account is unprivileged unless you ask for this.
        #[arg(long)]
        grant_privileged_roles: bool,
        /// Rows per committed transaction.
        #[arg(long, default_value_t = 1000)]
        batch_size: i64,
        /// Emit the machine-readable summary as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn run(command: UserCommands) {
    let enabled = matches!(command, UserCommands::Enable { .. });
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
                        username: &username,
                        email: &email,
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
            let user = match resolve_user_public_id(&pool, &username).await {
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
        UserCommands::ImportState {
            input,
            dry_run,
            allow_unmatched,
            require_all_matches,
            no_create_users,
            grant_privileged_roles,
            batch_size,
            json,
        } => {
            let options = crate::user_state_import::ImportOptions {
                dry_run,
                allow_unmatched,
                require_all_matches,
                create_users: !no_create_users,
                grant_privileged_roles,
                batch_size,
            };
            match crate::user_state_import::import(&pool, &input, options).await {
                Ok(summary) => {
                    if json {
                        match serde_json::to_string_pretty(&summary) {
                            Ok(rendered) => println!("{rendered}"),
                            Err(error) => {
                                eprintln!("error: cannot render summary: {error}");
                                std::process::exit(1);
                            }
                        }
                    } else {
                        println!("{}", crate::user_state_import::render(&summary));
                    }
                }
                Err(message) => {
                    eprintln!("user-state import failed: {message}");
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
