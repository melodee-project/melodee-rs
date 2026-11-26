//! Migration and maintenance entry points used by the updater (ADR-0213).
//!
//! `melodee-cli migrate` remains the single upgrade entry point. Its flags are
//! safe before and after the upgrade:
//!
//! * `--preflight` reports capacity, store binding and eligibility facts
//!   without mutating anything.
//! * `--status` reports resumable progress without mutating anything.
//! * `--verify` runs the independent manifest evidence check and exits nonzero
//!   on any discrepancy.
//! * no flag resumes the normal upgrade, which performs the independent check
//!   before finalization.

use clap::Args;
use melodee_persistence::{SearchStoreError, manifest_evidence};
use serde::Serialize;
use time::OffsetDateTime;

use super::common::pool;

#[derive(Debug, Args)]
pub(crate) struct MigrateArgs {
    /// Read-only capacity, store binding and eligibility report.
    #[arg(long, conflicts_with_all = ["status", "verify"])]
    pub preflight: bool,
    /// Read-only upgrade progress.
    #[arg(long, conflicts_with_all = ["preflight", "verify"])]
    pub status: bool,
    /// Independent integrity check; exits nonzero on any discrepancy.
    #[arg(long, conflicts_with_all = ["preflight", "status"])]
    pub verify: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationPreflight {
    database_reachable: bool,
    migrations_applied: i64,
    migrations_available: i64,
    migrations_current: bool,
    asset_store_bound: bool,
    asset_store_root: Option<String>,
    manifest_rows: i64,
    manifest_inline_rows: i64,
    manifest_evidence_rows: i64,
    manifest_evidence_bytes: i64,
    manifest_filesystem_bytes: i64,
    storage_headroom_bytes: Option<u64>,
    search_outbox_pending: i64,
    search_outbox_processed_retained: i64,
    provider_eligibility: Vec<ProviderEligibilityReport>,
    provider_eligibility_resolved: bool,
    blocking_conditions: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderEligibilityReport {
    provider: String,
    revision: i64,
    metadata_eligible: bool,
    artwork_eligible: bool,
}

pub(crate) async fn run(args: MigrateArgs) {
    let pool = pool().await;
    if args.preflight {
        match preflight(&pool).await {
            Ok(preflight) => report(&preflight, 0),
            Err(error) => fail(&error.to_string()),
        }
        return;
    }
    if args.status {
        let status = manifest_evidence::status(&pool).await;
        match status {
            Ok(status) => report(&status, 0),
            Err(error) => fail(&error.to_string()),
        }
        return;
    }
    if args.verify {
        let verification = match manifest_evidence::verify_all(&pool, 100).await {
            Ok(verification) => verification,
            Err(error) => fail(&error.to_string()),
        };
        let clean = verification.is_clean();
        report(&verification, i32::from(!clean));
        return;
    }
    if let Err(error) = melodee_persistence::migrate(&pool).await {
        fail(&format!("migration failed: {error}"));
    }
    // The normal upgrade path already performed the independent check before
    // finalization; report the durable state so an operator can record it.
    match manifest_evidence::status(&pool).await {
        Ok(status) => report(&status, 0),
        Err(error) => fail(&error.to_string()),
    }
}

async fn preflight(pool: &sqlx::PgPool) -> Result<MigrationPreflight, SearchStoreError> {
    let status = manifest_evidence::status(pool).await.map_err(|error| {
        SearchStoreError::InvalidRequest(format!("manifest evidence status failed: {error}"))
    })?;
    let migrations = melodee_persistence::migration_status(pool).await?;
    let outbox_pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox
          WHERE processed_at IS NULL AND failed_at IS NULL",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(0);
    let outbox_processed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM catalog_projection_outbox WHERE processed_at IS NOT NULL",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(0);
    let eligibility = melodee_persistence::ProviderEligibilityStore::list(pool)
        .await
        .map_err(|error| {
            SearchStoreError::InvalidRequest(format!("provider eligibility read failed: {error}"))
        })?;
    let root = std::env::var(melodee_persistence::asset_files::ENV_ASSET_ROOT).ok();
    let headroom = root.as_ref().and_then(|root| {
        rustix::fs::statvfs(std::path::Path::new(root))
            .ok()
            .map(|stat| stat.f_bavail * stat.f_frsize)
    });
    let mut blocking = Vec::new();
    if status.store_id.is_none() {
        blocking.push("asset store is not bound to this database".to_string());
    }
    if !migrations.is_current() {
        blocking.push(format!(
            "migrations are not current (applied {}, available {})",
            migrations.applied, migrations.latest_available
        ));
    }
    if status.inline_remaining > 0 || status.evidence_rows < status.manifests_migrated {
        blocking.push(format!(
            "{} manifest rows still require the evidence upgrade",
            status.inline_remaining
        ));
    }
    Ok(MigrationPreflight {
        database_reachable: true,
        migrations_applied: migrations.applied,
        migrations_available: migrations.latest_available,
        migrations_current: migrations.is_current(),
        asset_store_bound: status.store_id.is_some(),
        asset_store_root: root,
        manifest_rows: status.pending + status.evidence_rows,
        manifest_inline_rows: status.inline_remaining,
        manifest_evidence_rows: status.evidence_rows,
        manifest_evidence_bytes: status.evidence_bytes,
        manifest_filesystem_bytes: status.filesystem_bytes,
        storage_headroom_bytes: headroom,
        search_outbox_pending: outbox_pending,
        search_outbox_processed_retained: outbox_processed,
        provider_eligibility_resolved: !eligibility.is_empty(),
        provider_eligibility: eligibility
            .into_iter()
            .map(|row| ProviderEligibilityReport {
                provider: row.provider,
                revision: row.revision,
                metadata_eligible: row.metadata_eligible,
                artwork_eligible: row.artwork_eligible,
            })
            .collect(),
        blocking_conditions: blocking,
    })
}

fn report<T: Serialize>(value: &T, exit_code: i32) {
    match serde_json::to_string_pretty(value) {
        Ok(document) => println!("{document}"),
        Err(error) => {
            eprintln!("error: cannot serialize report: {error}");
            std::process::exit(1);
        }
    }
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1);
}

/// Prints the UTC timestamp the upgrade started at, for the runbook operator.
#[allow(dead_code)]
fn started_at() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}
