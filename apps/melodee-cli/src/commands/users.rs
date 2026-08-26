//! User bootstrap and operator account recovery commands.

use std::env;

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
