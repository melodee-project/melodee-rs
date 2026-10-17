//! legacy (C#) migration tooling — ADR-0148.
//!
//! The `melodee legacy` command group migrates the frozen C# PostgreSQL model
//! into the Rust model. The source database is opened strictly read-only, the
//! run model is durable and fenced (`legacy_migration.runs`), resume is
//! idempotent on `(run, domain, source_pk)`, and nothing unrepresentable is
//! silently dropped (every anomaly is a durable issue row). `baseline` and
//! `freeze` provide the immutable-snapshot and cutover gates around the run.

mod baseline;
mod delta;
mod freeze;
mod import;
mod import_avatars;
mod import_catalog;
mod import_contrib;
mod import_notification_templates;
mod import_state;
mod import_state2;
mod import_state3;
mod import_state4;
mod import_theme_packs;
mod import_tracks;
mod import_users;
mod map;
mod media;
mod runs;
mod source;
mod validate;
mod verify;

use clap::{Args, Subcommand};
use sqlx::PgPool;

/// Batch bound for keyset pagination over legacy tables. Every batch is one
/// target transaction containing the inserts, the mapping rows, and the
/// checkpoint advance, fenced by the run lease.
pub const DEFAULT_BATCH_SIZE: i64 = 500;

#[derive(Debug, Subcommand)]
pub enum LegacyCommands {
    /// Read-only source inventory: schema fingerprint, counts, anomalies.
    Inventory(LegacySourceArgs),
    /// Dry-run plan: compares inventory with target state and records a
    /// durable planned run without writing target domain data.
    Plan(LegacyPlanArgs),
    /// Import the legacy catalog, identity, and user state with durable
    /// checkpoints and idempotent resume.
    Import(LegacyRunArgs),
    /// Verify source/target parity for an imported run and record mismatches
    /// as durable issues.
    Verify(LegacyVerifyArgs),
    /// Apply an incremental delta for rows changed since the last import.
    Delta(LegacyRunArgs),
    /// Show run state, per-domain checkpoints, and issue totals.
    Status(LegacyStatusArgs),
    /// Capture the immutable rehearsal baseline (ADR-0149): source identity,
    /// snapshot marker, counts, media totals, and a redacted configuration
    /// fingerprint; recorded durably into the run with `--run`.
    Baseline(LegacyBaselineArgs),
    /// Final freeze pass (ADR-0149): prove source write-stillness across the
    /// final delta, re-verify the gate, record freeze evidence, and only with
    /// `--acknowledge-cutover` transition the run to `cutover-ready`.
    Freeze(LegacyFreezeArgs),
    /// Validate migrated media under a supplied canonical root; optionally
    /// apply only explicitly confirmed target byte-size metadata repairs.
    Validate(LegacyValidateArgs),
}

#[derive(Debug, Args)]
pub struct LegacyBaselineArgs {
    #[command(flatten)]
    source: LegacySourceArgs,
    /// Run to record the baseline into; omitted prints without persisting.
    #[arg(long)]
    run: Option<uuid::Uuid>,
}

#[derive(Debug, Args)]
pub struct LegacyFreezeArgs {
    #[command(flatten)]
    source: LegacySourceArgs,
    /// Run to freeze (must be `delta-ready`).
    #[arg(long)]
    run: uuid::Uuid,
    /// Rows per keyset batch for the final delta.
    #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
    batch_size: i64,
    /// Root mapping, repeatable (see `plan`).
    #[arg(long = "library-path-map")]
    library_path_maps: Vec<String>,
    /// Mounted canonical storage root for read-only adopted-media validation.
    #[arg(long)]
    canonical_root: Option<std::path::PathBuf>,
    /// Legacy `security.secretKey` for re-sealing legacy-protected secrets
    /// during the final delta (see `import`).
    #[arg(long)]
    legacy_secret_key: Option<String>,
    /// Legacy installation root for file-based legacy assets (notification
    /// templates, theme packs, custom content blocks).
    #[arg(long)]
    legacy_root: Option<std::path::PathBuf>,
    /// Record the operator cutover acknowledgement and transition the run to
    /// `cutover-ready`. Requires --note.
    #[arg(long)]
    acknowledge_cutover: bool,
    /// Operator statement recorded with the acknowledgement (auditable).
    #[arg(long)]
    note: Option<String>,
}

#[derive(Debug, Args)]
pub struct LegacySourceArgs {
    /// Read-only connection URL for the legacy snapshot/replica database.
    #[arg(long, env = "MELODEE_LEGACY_SOURCE_DATABASE_URL")]
    source_db: String,
    /// Emit stable machine-readable JSON instead of human-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub struct LegacyPlanArgs {
    #[command(flatten)]
    source: LegacySourceArgs,
    /// Root mapping, repeatable:
    /// `--library-path-map /legacy/library/root=relative/prefix`.
    #[arg(long = "library-path-map")]
    library_path_maps: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LegacyRunArgs {
    #[command(flatten)]
    source: LegacySourceArgs,
    /// Existing run to resume; omitted creates a new planned run and imports.
    #[arg(long)]
    run: Option<uuid::Uuid>,
    /// Rows per keyset batch (one target transaction per batch).
    #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
    batch_size: i64,
    /// Root mapping, repeatable (see `plan`).
    #[arg(long = "library-path-map")]
    library_path_maps: Vec<String>,
    /// Mounted canonical storage root for read-only existence/size/symlink
    /// validation of adopted media (ADR-0148 §D.3).
    #[arg(long)]
    canonical_root: Option<std::path::PathBuf>,
    /// The legacy deployment's `security.secretKey`, used in memory only to
    /// re-seal legacy-protected per-user secrets (ADR-0148 §D.8). Never
    /// logged or persisted.
    #[arg(long)]
    legacy_secret_key: Option<String>,
    /// Legacy installation root for file-based legacy assets (notification
    /// templates, theme packs, custom content blocks). The Templates library
    /// lives under `<legacy-root>/Templates/<culture>/` and the Theme library
    /// under `<legacy-root>/Themes/`.
    #[arg(long)]
    legacy_root: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
pub struct LegacyVerifyArgs {
    #[command(flatten)]
    source: LegacySourceArgs,
    /// Run to verify (must be past `importing`).
    #[arg(long)]
    run: uuid::Uuid,
    /// Maximum mismatch samples printed per domain.
    #[arg(long, default_value_t = 20)]
    sample_limit: i64,
    /// Mounted canonical storage root for read-only adopted-media validation.
    #[arg(long)]
    canonical_root: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
pub struct LegacyStatusArgs {
    /// Run to inspect; omitted lists all runs.
    #[arg(long)]
    run: Option<uuid::Uuid>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub struct LegacyValidateArgs {
    /// Imported migration run whose mapped media is being checked.
    #[arg(long)]
    run: uuid::Uuid,
    /// Existing canonical media root. It is opened read-only and must not
    /// contain symlink components.
    #[arg(long)]
    canonical_root: std::path::PathBuf,
    /// Print the deterministic repair plan and apply target metadata repairs.
    #[arg(long)]
    fix: bool,
    /// Required with --fix. This acknowledges target metadata mutation only;
    /// source media is never modified.
    #[arg(long)]
    confirm: bool,
    /// Stable operator identity recorded in repair issues.
    #[arg(long, default_value = "legacy-validate")]
    actor: String,
    #[arg(long)]
    json: bool,
}

/// Operator identity for run fencing: hostname plus pid, overridable.
pub fn default_owner() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string());
    format!("{host}:{}", std::process::id())
}

/// Parsed `--library-path-map` entry: legacy absolute library path prefix ->
/// canonical-relative prefix (normalized, never absolute).
#[derive(Debug, Clone)]
pub struct LibraryPathMap {
    pub legacy_path: String,
    pub relative_prefix: String,
}

pub fn parse_library_path_maps(values: &[String]) -> Result<Vec<LibraryPathMap>, String> {
    let mut maps = Vec::with_capacity(values.len());
    for value in values {
        let (legacy, prefix) = value
            .split_once('=')
            .ok_or_else(|| format!("invalid --library-path-map {value:?}: expected A=B"))?;
        if legacy.contains('\0') || prefix.contains('\0') {
            return Err(format!(
                "invalid --library-path-map {value:?}: NUL is not allowed"
            ));
        }
        let legacy = legacy.trim().trim_end_matches('/');
        let raw_prefix = prefix.trim();
        if raw_prefix.starts_with('/') || raw_prefix.contains('\\') {
            return Err(format!(
                "invalid --library-path-map {value:?}: relative prefix must not be absolute or contain backslashes"
            ));
        }
        let prefix = raw_prefix.trim_matches('/');
        if legacy.is_empty() || !legacy.starts_with('/') {
            return Err(format!(
                "invalid --library-path-map {value:?}: legacy path must be absolute"
            ));
        }
        if legacy.split('/').any(|component| component == "..") {
            return Err(format!(
                "invalid --library-path-map {value:?}: legacy path may not contain '..'"
            ));
        }
        // An empty prefix maps the legacy library root onto the canonical
        // root itself (single-library deployments); any non-empty prefix must
        // be a clean relative path.
        if !prefix.is_empty()
            && prefix
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err(format!(
                "invalid --library-path-map {value:?}: relative prefix has unsafe components"
            ));
        }
        if maps
            .iter()
            .any(|map: &LibraryPathMap| map.legacy_path == legacy)
        {
            return Err(format!(
                "invalid --library-path-map {value:?}: duplicate legacy root"
            ));
        }
        maps.push(LibraryPathMap {
            legacy_path: legacy.to_string(),
            relative_prefix: prefix.to_string(),
        });
    }
    Ok(maps)
}

/// Dispatches the `legacy` command group. `target` is the Rust database pool.
///
/// # Errors
/// Returns a human-actionable error string; the CLI prints it and exits 1.
pub async fn run(command: LegacyCommands, target: PgPool) -> Result<(), String> {
    match command {
        LegacyCommands::Inventory(args) => {
            let source = source::SourceDb::connect(args.source_db()).await?;
            let inventory = source::collect_inventory(&source).await?;
            inventory.render(args.json);
            Ok(())
        }
        LegacyCommands::Plan(args) => {
            let maps = parse_library_path_maps(&args.library_path_maps)?;
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            let inventory = source::collect_inventory(&source).await?;
            let identity = source.source_identity().await?;
            let marker = source.snapshot_marker().await?;
            let run_id = runs::record_plan(
                &target,
                &inventory,
                &maps,
                &identity,
                &marker,
                args.source.json,
            )
            .await?;
            inventory.render_plan(&maps, run_id, args.source.json);
            Ok(())
        }
        LegacyCommands::Import(args) => {
            let maps = parse_library_path_maps(&args.library_path_maps)?;
            let validator = args
                .canonical_root
                .as_deref()
                .map(media::CanonicalRootValidator::new)
                .transpose()?;
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            import::import_all(
                &source,
                &target,
                args.run,
                args.batch_size,
                &maps,
                true,
                validator.as_ref(),
                args.legacy_secret_key.as_deref(),
                args.legacy_root.as_deref(),
            )
            .await
        }
        LegacyCommands::Verify(args) => {
            let validator = args
                .canonical_root
                .as_deref()
                .map(media::CanonicalRootValidator::new)
                .transpose()?;
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            verify::verify_run(
                &source,
                &target,
                args.run,
                args.sample_limit,
                args.source.json,
                validator.as_ref(),
            )
            .await
        }
        LegacyCommands::Delta(args) => {
            let maps = parse_library_path_maps(&args.library_path_maps)?;
            let validator = args
                .canonical_root
                .as_deref()
                .map(media::CanonicalRootValidator::new)
                .transpose()?;
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            delta::apply_delta(
                &source,
                &target,
                args.run,
                args.batch_size,
                &maps,
                validator.as_ref(),
                true,
                args.legacy_secret_key.as_deref(),
                args.legacy_root.as_deref(),
            )
            .await
        }
        LegacyCommands::Status(args) => runs::status(&target, args.run, args.json).await,
        LegacyCommands::Baseline(args) => {
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            baseline::record_baseline(&source, &target, args.run, args.source.json).await
        }
        LegacyCommands::Freeze(args) => {
            let maps = parse_library_path_maps(&args.library_path_maps)?;
            let validator = args
                .canonical_root
                .as_deref()
                .map(media::CanonicalRootValidator::new)
                .transpose()?;
            let source = source::SourceDb::connect(args.source.source_db()).await?;
            freeze::freeze(
                &source,
                &target,
                args.run,
                args.batch_size,
                &maps,
                validator.as_ref(),
                args.legacy_secret_key.as_deref(),
                args.legacy_root.as_deref(),
                args.acknowledge_cutover,
                args.note.as_deref(),
                args.source.json,
            )
            .await
        }
        LegacyCommands::Validate(args) => {
            validate::validate(
                &target,
                args.run,
                &args.canonical_root,
                args.fix,
                args.confirm,
                args.json,
                &args.actor,
            )
            .await
        }
    }
}

impl LegacySourceArgs {
    pub fn source_db(&self) -> &str {
        &self.source_db
    }
}

#[cfg(test)]
mod tests {
    use super::parse_library_path_maps;

    #[test]
    fn path_maps_reject_unsafe_or_ambiguous_configuration() {
        for value in [
            "relative=library",
            "/music=../library",
            "/music=/absolute",
            "/music=library\\nested",
            "/music/../other=library",
            "/music=library\0suffix",
        ] {
            assert!(
                parse_library_path_maps(&[value.to_string()]).is_err(),
                "{value:?}"
            );
        }
        assert!(
            parse_library_path_maps(&["/music=library".to_string(), "/music=other".to_string(),])
                .is_err()
        );
    }

    #[test]
    fn path_maps_preserve_longest_root_matching_inputs() -> Result<(), String> {
        let maps = parse_library_path_maps(&[
            "/srv/music=library".to_string(),
            "/srv/music/live=live".to_string(),
        ])?;
        assert_eq!(maps[0].legacy_path, "/srv/music");
        assert_eq!(maps[1].relative_prefix, "live");
        Ok(())
    }
}
