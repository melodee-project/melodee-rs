//! Library access group management (mirrors the admin API semantics).

use clap::Subcommand;
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::{parse_nonblank, pool};

#[derive(Debug, Subcommand)]
pub(crate) enum GroupCommands {
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

pub(crate) async fn run(command: GroupCommands) {
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
            .create_group(&pool, &name, now)
            .await
            .map(|group| format!("created: {} ({})", group.name, group.public_api_id)),
        GroupCommands::Delete { name } => store
            .delete_group(&pool, &name)
            .await
            .map(|()| format!("deleted: {name}")),
        GroupCommands::AddUser { group, username } => store
            .add_user(&pool, &group, &username, now)
            .await
            .map(|added| {
                if added {
                    format!("added {username} to {group}")
                } else {
                    format!("{username} is already a member of {group}")
                }
            }),
        GroupCommands::RemoveUser { group, username } => store
            .remove_user(&pool, &group, &username, now)
            .await
            .map(|removed| {
                if removed {
                    format!("removed {username} from {group}")
                } else {
                    format!("{username} was not a member of {group}")
                }
            }),
        GroupCommands::BindLibrary { group, library_id } => store
            .bind_library(&pool, &group, library_id, now)
            .await
            .map(|bound| {
                if bound {
                    format!("bound library {library_id} to {group}")
                } else {
                    format!("library {library_id} is already bound to {group}")
                }
            }),
        GroupCommands::UnbindLibrary { group, library_id } => store
            .unbind_library(&pool, &group, library_id, now)
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
