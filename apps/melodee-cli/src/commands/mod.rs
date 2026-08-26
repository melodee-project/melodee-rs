//! CLI command groups. Each submodule owns its clap subcommand enum(s) and
//! the handler that executes them; `main.rs` keeps only argument parsing and
//! dispatch. Shared database access lives in `common`.

pub(crate) mod catalog;
pub(crate) mod charts;
pub(crate) mod common;
pub(crate) mod config;
pub(crate) mod doctor;
pub(crate) mod groups;
pub(crate) mod ingest;
pub(crate) mod jobs;
pub(crate) mod notifications;
pub(crate) mod podcasts;
pub(crate) mod policy;
pub(crate) mod rollback;
pub(crate) mod scripts;
pub(crate) mod search;
pub(crate) mod users;
pub(crate) mod work;
pub(crate) mod worker_pools;
