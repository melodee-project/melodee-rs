//! Server settings operator commands: list effective layered values
//! (secrets always redacted), validate preflight, export/import.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::{pool, read_bounded_file};

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommands {
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

/// Configuration commands share the server's stores and onboarding
/// preflight (PRD-CLI-002: never bypass safety), mirroring the admin API
/// semantics DB-direct (the CLI authenticates as the database role, not via
/// HTTP). Secret values are never printed: listings and exports use the
/// fixed redaction marker, and import plans name secret keys only.
pub(crate) async fn run(command: ConfigCommands) {
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
                    if let Err(error) = write_export_file(&path, format!("{rendered}\n").as_bytes())
                    {
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
            let raw = read_bounded_file(&path, melodee_persistence::MAX_IMPORT_DOCUMENT_BYTES)
                .unwrap_or_else(|error| {
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
                allow_unknown,
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

/// Writes a config export to a new regular file without following a parent or
/// leaf symlink. Existing destinations are refused so an export cannot
/// overwrite an operator-selected file during a path replacement race.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn write_export_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{ffi::OsString, io::Write, os::unix::ffi::OsStrExt, path::Component};

    use rustix::fs::{AtFlags, CWD, Mode, OFlags, RenameFlags, openat, renameat_with, unlinkat};

    if path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "export path is empty",
        ));
    }
    let relative_parent;
    let absolute_parent = if path.is_absolute() {
        path.parent().unwrap_or_else(|| Path::new("/"))
    } else {
        relative_parent =
            std::env::current_dir()?.join(path.parent().unwrap_or_else(|| Path::new(".")));
        &relative_parent
    };
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "export path has no file name",
        )
    })?;
    let mut parent = openat(
        CWD,
        b"/".as_slice(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    for component in absolute_parent.components() {
        let Component::Normal(component) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "export parent contains traversal",
            ));
        };
        parent = openat(
            &parent,
            component.as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
    }

    let temporary = OsString::from(format!(
        ".melodee-config-export-{}.tmp",
        Uuid::now_v7().simple()
    ));
    let file = openat(
        &parent,
        temporary.as_os_str().as_bytes(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(std::io::Error::from)?;
    let mut file = std::fs::File::from(file);
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = unlinkat(&parent, temporary.as_os_str().as_bytes(), AtFlags::empty());
        return Err(error);
    }
    drop(file);
    let result = renameat_with(
        &parent,
        temporary.as_os_str().as_bytes(),
        &parent,
        name.as_bytes(),
        RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from);
    if result.is_err() {
        let _ = unlinkat(&parent, temporary.as_os_str().as_bytes(), AtFlags::empty());
    }
    result
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn write_export_file(_path: &Path, _bytes: &[u8]) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "race-resistant config export is unsupported on this platform",
    ))
}
