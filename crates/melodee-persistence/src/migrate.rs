//! Migration runner. Migrations are explicit, source-controlled SQL under
//! `crates/melodee-persistence/migrations/`, embedded at compile time and
//! runnable from an empty database. Phase-specific additive indexes are kept
//! here as migrations as well, including serving projections and predicate-
//! aligned indexes, so an upgraded catalog receives the same query-plan
//! guarantees as a freshly created database.

use std::borrow::Cow;

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

struct CommentOnlyMigrationHistory {
    version: i64,
    canonical_checksum: &'static str,
    historical_checksums: &'static [&'static str],
}

const COMMENT_ONLY_MIGRATION_HISTORIES: &[CommentOnlyMigrationHistory] = &[
    CommentOnlyMigrationHistory {
        version: 24,
        canonical_checksum: "3aa53396b0f151b3bd65ea46f0f8ac9c13a6bee852606caec13c5c324ee10642819b5abc520c1635df1b8517aa44ce53",
        historical_checksums: &[
            "ef2d48a7d644462526ac3b0edcae09acede7a68780bb83e5104168ede40838e813b8dafb5b68486649ea4ec15ffb9b2d",
            "db215eaedae3d7b073ec8e11529a80f37bcc3eb80434440a0b2b1451b186d99b53eece7913a4fff6ca9c26ae75ad4bdc",
            "519fcfb3fea9a11333051a96aa48eb098a230ae017b79bc06e3f836e18731ba74dd9ad372f6191278b17900c0dca5c86",
        ],
    },
    CommentOnlyMigrationHistory {
        version: 25,
        canonical_checksum: "4ff37ec22e6990de431a314ae814b433c708ccf5f5a4f01b707147bafe166bf18d2f904aee21312144f05dcda1eeffa3",
        historical_checksums: &[
            "930549083b8d47b46209d572355e6949e8a3d35c65d8d46c289f5c6260f4132845156e41175303509dc8b3a111f753b4",
            "48e7e25d82831e4359150cec632b265e6423f87d72d8616639d3407b78a7b61b6427b258afce0c25c3e405ca817ed3fc",
            "67df569fcd50efd7c45e4901e719c5634ab5cffc9320961ad73fd780b8d64e44313122265af23b4e1ab6b2645ab1ca0b",
        ],
    },
    CommentOnlyMigrationHistory {
        version: 26,
        canonical_checksum: "d62fdba16524b8cf6d9c49b02e4da58d74459d3f5dbb6fa54a96fd032096c5805ac27ee2f8f791cd462738af59c09e77",
        historical_checksums: &[
            "c3683e0214abdb1e7e7455e4ad2147b5f3cd023695583b2a1d12a1f876bb9dcd5d8e90c7ce0b71122a9d6938e0b9ca91",
            "d7b2aca713222fa08e0d17ff68548c20834cf6a597ec516bbbdd20f528be201b0cd681d717bae60e6d7ddd2a7da6f362",
            "da94503238dbc6bc5d2af03aa0e3ad1979a22ef2e527bf3f6029ed12610cf7fd0a983a4a8feea7451b45fa9be4ceb32d",
        ],
    },
    CommentOnlyMigrationHistory {
        version: 27,
        canonical_checksum: "f615ea59eb70cf05d9fa5cda69d5e71d3de670e1c748bcebfa38cda5e096d70de2dcc4079b41b51ad2200c22451a5952",
        historical_checksums: &[
            "a69c28d0a6c0858a1afc0e0ce22a19b14e413fc22fe3aea358fe574e0b89736cab475477127c03358ea4e0dd5313ce12",
            "2afde0c080b0d4ed0d62e64bedeea40d1c573a5f3b490a24692ac18c4a970e9bdf0fd82c360e8e6f62465d4770b801d4",
            "b48fe85eeff029b19660b46fd3744d3fd8d0ffda9a7de9dae1853b23e596f11172ea3cd4b976a980ac5f530d33472465",
        ],
    },
    CommentOnlyMigrationHistory {
        version: 28,
        canonical_checksum: "fe1bb7cbbd01ce1d5e92c8b85009525e04dbfcbe99ffe0376a3fdbebda5f5951f1f44d58d29efae2e54907a1a0e6dca7",
        historical_checksums: &[
            "aff9741d36948ab4270ef82a06e812ff5e30f46f33ba7f48ce0e2774c89388bbbddf1803d9183247688b75f421ff72cf",
            "dd2445592ab91b0fe06460383364b19bf20cde4c61fcec17a21332494402553b0e772fbf9121536c3f3ab0ef7e3e561e",
            "7bb98df26fef9cce58c1807f9f2a1c1e1f5ef8d415507dc17937a9028d3eb065ab63993c13aa7b1cbcbfaa7a51dd4415",
        ],
    },
    CommentOnlyMigrationHistory {
        version: 29,
        canonical_checksum: "cbeb431a3684ba0c307e89e6f96a17e69d671bea79c5ef1b2549eaf980b03ff662250aefa0b3769d38179a61394aad15",
        historical_checksums: &[
            "e262a49d4695c56444b693f86b0ede34e20e481068533737bef5dbd5864399aafdd0e6aae22c448784d5138928c06251",
            "b8b31795fcb8a3e6ad10e822ef31edf5c4852218f5c592b07a1465e6afe3eeb46f5733024347dffa4cdf817de1046151",
        ],
    },
];

#[derive(Debug, thiserror::Error)]
enum MigrationCompatibilityInvariant {
    #[error("embedded migration {0} is missing from the canonical migration set")]
    MissingCanonicalMigration(i64),
    #[error(
        "embedded migration {0} no longer matches its frozen canonical checksum; add a new migration instead of editing an applied migration"
    )]
    CanonicalMigrationChanged(i64),
}

/// Applies all pending migrations. Idempotent (sqlx tracks applied versions).
///
/// # Errors
/// Returns the migration error describing the failed migration.
pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError> {
    match compatible_migrator(pool).await? {
        Some(migrator) => migrator.run(pool).await,
        None => MIGRATOR.run(pool).await,
    }?;
    crate::catalog_mutations::repair_import_overlay_references(pool)
        .await
        .map_err(|error| MigrateError::Source(Box::new(error)))
}

/// Builds a one-run migrator for exact historical checksums created by
/// comment-only repository edits to migrations 24 through 29. SQLx still
/// rejects every unknown checksum. Database migration metadata is never
/// rewritten.
async fn compatible_migrator(pool: &PgPool) -> Result<Option<Migrator>, MigrateError> {
    for history in COMMENT_ONLY_MIGRATION_HISTORIES {
        let canonical = MIGRATOR
            .migrations
            .iter()
            .find(|migration| migration.version == history.version)
            .ok_or_else(|| {
                compatibility_source_error(
                    MigrationCompatibilityInvariant::MissingCanonicalMigration(history.version),
                )
            })?;
        if checksum_hex(canonical.checksum.as_ref()) != history.canonical_checksum {
            return Err(compatibility_source_error(
                MigrationCompatibilityInvariant::CanonicalMigrationChanged(history.version),
            ));
        }
    }

    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !table_exists {
        return Ok(None);
    }

    let applied_checksums: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version, checksum FROM _sqlx_migrations \
         WHERE version BETWEEN 24 AND 29 AND success ORDER BY version",
    )
    .fetch_all(pool)
    .await?;

    let mut migrations = MIGRATOR.migrations.to_vec();
    let mut used_compatibility = false;
    for (version, applied_checksum) in applied_checksums {
        let Some(history) = COMMENT_ONLY_MIGRATION_HISTORIES
            .iter()
            .find(|history| history.version == version)
        else {
            continue;
        };
        let applied_checksum_hex = checksum_hex(&applied_checksum);
        if !history
            .historical_checksums
            .contains(&applied_checksum_hex.as_str())
        {
            continue;
        }

        tracing::warn!(
            migration_version = version,
            checksum = %applied_checksum_hex,
            "accepting an exact known comment-only historical SQLx migration checksum"
        );
        let migration = migrations
            .iter_mut()
            .find(|migration| migration.version == version)
            .ok_or_else(|| {
                compatibility_source_error(
                    MigrationCompatibilityInvariant::MissingCanonicalMigration(version),
                )
            })?;
        migration.checksum = Cow::Owned(applied_checksum);
        used_compatibility = true;
    }

    if !used_compatibility {
        return Ok(None);
    }

    Ok(Some(Migrator {
        migrations: Cow::Owned(migrations),
        ignore_missing: MIGRATOR.ignore_missing,
        locking: MIGRATOR.locking,
        no_tx: MIGRATOR.no_tx,
        table_name: MIGRATOR.table_name.clone(),
        create_schemas: MIGRATOR.create_schemas.clone(),
    }))
}

fn compatibility_source_error(error: MigrationCompatibilityInvariant) -> MigrateError {
    MigrateError::Source(Box::new(error))
}

fn checksum_hex(checksum: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(checksum.len() * 2);
    for byte in checksum {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Migration-currency evidence for the onboarding preflight (PRD-SETUP-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Highest applied migration version (0 when the tracking table is
    /// missing or empty).
    pub applied: i64,
    /// Highest migration version compiled into this binary.
    pub latest_available: i64,
    /// No applied migration row recorded a failure.
    pub all_successful: bool,
}

impl MigrationStatus {
    /// Current means: every compiled migration is applied and none failed.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.applied >= self.latest_available && self.all_successful
    }
}

/// Reads the sqlx migration tracking state without applying anything.
///
/// # Errors
/// Returns the database error; a missing `_sqlx_migrations` table is not an
/// error but reports `applied = 0` (never migrated).
pub async fn migration_status(pool: &PgPool) -> Result<MigrationStatus, sqlx::Error> {
    let latest_available = MIGRATOR
        .migrations
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0);
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !table_exists {
        return Ok(MigrationStatus {
            applied: 0,
            latest_available,
            all_successful: false,
        });
    }
    let row: (Option<i64>, i64) = sqlx::query_as(
        "SELECT MAX(version), COUNT(*) FILTER (WHERE NOT success) FROM _sqlx_migrations",
    )
    .fetch_one(pool)
    .await?;
    Ok(MigrationStatus {
        applied: row.0.unwrap_or(0),
        latest_available,
        all_successful: row.1 == 0 && row.0.is_some(),
    })
}
