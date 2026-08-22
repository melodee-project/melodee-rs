//! Read-only legacy source access, frozen-baseline fingerprint gate, and
//! inventory collection (ADR-0148 §D.1).

use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};

/// Frozen legacy schema fingerprint: the final `__EFMigrationsHistory` row at
/// baseline commit `4028b58d`.
pub const FROZEN_SCHEMA_FINAL_MIGRATION: &str =
    "20260714155936_AddDeleteSourceResidueAfterIngestSetting";

/// Legacy tables in the domains the migration reads, with the domain key used
/// for checkpoints and issues, plus the source-row key expression used for
/// keyset bounds. Join tables without a surrogate `Id` use the same synthetic
/// composite key the importer records in the mapping (`playlist_items`:
/// `PlaylistId * 1_000_000 + PlaylistOrder`; `request_seen`:
/// `RequestId * 2^31 + UserId`).
pub const INVENTORY_TABLES: &[(&str, &str, &str)] = &[
    ("users", "Users", "\"Id\"::bigint"),
    ("libraries", "Libraries", "\"Id\"::bigint"),
    ("artists", "Artists", "\"Id\"::bigint"),
    ("releases", "Albums", "\"Id\"::bigint"),
    ("tracks", "Songs", "\"Id\"::bigint"),
    ("contributions", "Contributors", "\"Id\"::bigint"),
    ("playlists", "Playlists", "\"Id\"::bigint"),
    ("playlist_items", "PlaylistSong", PLAYLIST_ITEM_KEY_SQL),
    ("shares", "Shares", "\"Id\"::bigint"),
    ("bookmarks", "Bookmarks", "\"Id\"::bigint"),
    ("user_track_states", "UserSongs", "\"Id\"::bigint"),
    ("user_release_states", "UserAlbums", "\"Id\"::bigint"),
    ("user_artist_states", "UserArtists", "\"Id\"::bigint"),
    ("track_plays", "UserSongPlayHistories", "\"Id\"::bigint"),
    ("podcast_channels", "PodcastChannels", "\"Id\"::bigint"),
    ("podcast_episodes", "PodcastEpisodes", "\"Id\"::bigint"),
    (
        "podcast_episode_bookmarks",
        "PodcastEpisodeBookmarks",
        "\"Id\"::bigint",
    ),
    (
        "podcast_play_history",
        "UserPodcastEpisodePlayHistories",
        "\"Id\"::bigint",
    ),
    ("radio_stations", "RadioStations", "\"Id\"::bigint"),
    ("settings", "Settings", "\"Id\"::bigint"),
    ("smart_playlists", "SmartPlaylists", "\"Id\"::bigint"),
    ("user_groups", "UserGroups", "\"Id\"::bigint"),
    ("user_group_members", "UserGroupMembers", "\"Id\"::bigint"),
    (
        "playback_settings",
        "UserPlaybackSettings",
        "\"Id\"::bigint",
    ),
    (
        "equalizer_presets",
        "UserEqualizerPresets",
        "\"Id\"::bigint",
    ),
    ("social_logins", "UserSocialLogins", "\"Id\"::bigint"),
    ("user_pins", "UserPins", "\"Id\"::bigint"),
    ("device_profiles", "UserDeviceProfiles", "\"Id\"::bigint"),
    ("requests", "Requests", "\"Id\"::bigint"),
    ("request_comments", "RequestComments", "\"Id\"::bigint"),
    ("request_seen", "RequestUserStates", REQUEST_SEEN_KEY_SQL),
    // Retired by ADR-0133/0140/0141: inventoried as excluded-by-decision.
    ("party_sessions_excluded", "PartySessions", "\"Id\"::bigint"),
    ("play_queues_excluded", "PlayQues", "\"Id\"::bigint"),
];

/// Synthetic source key for legacy `PlaylistSong` rows (no surrogate Id in
/// the frozen schema): matches the importer's mapping key.
pub const PLAYLIST_ITEM_KEY_SQL: &str =
    "(\"PlaylistId\"::bigint * 1000000::bigint + \"PlaylistOrder\")";
/// Synthetic source key for legacy `RequestUserStates` rows:
/// `RequestId * 2^31 + UserId` (both int4; the product cannot overflow i64).
pub const REQUEST_SEEN_KEY_SQL: &str = "(\"RequestId\"::bigint * 2147483648::bigint + \"UserId\")";

/// Frozen-schema tables included in the write-stillness marker. Tables with
/// `LastUpdatedAt` contribute both their row count and maximum timestamp;
/// append-only or join tables contribute their row count. The list is kept
/// separate from `INVENTORY_TABLES` because a source write must invalidate a
/// freeze even when the table is intentionally excluded from migration by an
/// accepted decision.
const SNAPSHOT_MARKER_TABLES: &[(&str, bool)] = &[
    ("Albums", true),
    ("ArtistRelation", true),
    ("Artists", true),
    ("Bookmarks", true),
    ("ChartItems", false),
    ("Charts", false),
    ("Contributors", true),
    ("JellyfinAccessTokens", false),
    ("JobHistories", false),
    ("Libraries", true),
    ("LibraryAccessControls", true),
    ("LibraryScanHistories", false),
    ("PartyAuditEvents", true),
    ("PartyPlaybackStates", true),
    ("PartyQueueItems", true),
    ("PartySessionEndpoints", true),
    ("PartySessionParticipants", false),
    ("PartySessions", true),
    ("PlayQues", true),
    ("Players", true),
    ("PlaylistSong", false),
    ("PlaylistUploadedFileItems", true),
    ("PlaylistUploadedFiles", true),
    ("Playlists", true),
    ("PodcastChannels", true),
    ("PodcastEpisodeBookmarks", false),
    ("PodcastEpisodes", true),
    ("RadioStations", true),
    ("RefreshTokens", true),
    ("RequestComments", false),
    ("RequestParticipants", false),
    ("RequestUserStates", false),
    ("Requests", false),
    ("SearchHistories", false),
    ("Settings", true),
    ("ShareActivities", false),
    ("Shares", true),
    ("SmartPlaylists", true),
    ("Songs", true),
    ("UserAlbums", true),
    ("UserArtists", true),
    ("UserDeviceProfiles", true),
    ("UserEqualizerPresets", true),
    ("UserGroupMembers", true),
    ("UserGroups", true),
    ("UserPins", true),
    ("UserPlaybackSettings", true),
    ("UserPodcastEpisodePlayHistories", false),
    ("UserSocialLogins", true),
    ("UserSongPlayHistories", false),
    ("UserSongs", true),
    ("Users", true),
];

/// Stable, non-sensitive facts proving that a source connection is using the
/// read-only boundary. The role may have wider privileges on an isolated
/// snapshot, but every migration session must still be transaction-read-only.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadOnlyStatus {
    pub transaction_read_only: bool,
    pub default_transaction_read_only: bool,
    pub in_recovery: bool,
}

/// A read-only handle to the legacy snapshot. Every session enforces
/// `default_transaction_read_only = on` so a write fails at the database
/// regardless of a tooling bug (ADR-0148 §D.1).
pub struct SourceDb {
    pool: PgPool,
    fingerprint: String,
}

impl SourceDb {
    /// Connects with read-only session enforcement and verifies the frozen
    /// schema fingerprint. Fails closed on any other schema version.
    ///
    /// # Errors
    /// Returns an actionable message on connection failure, a missing
    /// migration-history table, or an unsupported schema fingerprint.
    pub async fn connect(url: &str) -> Result<Self, String> {
        let options: PgConnectOptions = url
            .parse()
            .map_err(|e| format!("invalid --source-db URL: {e}"))?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // One statement per call: the extended query protocol
                    // rejects multi-statement strings, and a hook failure
                    // surfaces only as an opaque pool-acquire timeout.
                    sqlx::query("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET SESSION statement_timeout = '120s'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(|e| format!("cannot connect to legacy source database: {e}"))?;

        let fingerprint = sqlx::query_scalar::<_, String>(
            r#"SELECT "MigrationId" FROM "__EFMigrationsHistory"
               ORDER BY "MigrationId" DESC LIMIT 1"#,
        )
        .fetch_optional(&pool)
        .await
        .map_err(|e| {
            format!(
                "cannot read legacy schema fingerprint (__EFMigrationsHistory): {e}; \
                 is this a Melodee 2.2.x legacy database?"
            )
        })?
        .ok_or_else(|| {
            format!(
                "unsupported legacy schema version: __EFMigrationsHistory is empty, \
                 expected a final row of {FROZEN_SCHEMA_FINAL_MIGRATION:?} (frozen baseline)"
            )
        })?;
        if fingerprint != FROZEN_SCHEMA_FINAL_MIGRATION {
            return Err(format!(
                "unsupported legacy schema version: final migration is {fingerprint:?}, \
                 expected {FROZEN_SCHEMA_FINAL_MIGRATION:?} (frozen baseline). \
                 Restore the supported snapshot or record a new compatibility decision."
            ));
        }
        let source = Self { pool, fingerprint };
        source.ensure_read_only().await?;
        Ok(source)
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Returns the session/database read-only facts used by baseline evidence.
    /// `transaction_read_only` is checked rather than inferred from the URL so
    /// a future connection-pool change cannot silently weaken the source
    /// boundary.
    pub async fn read_only_status(&self) -> Result<ReadOnlyStatus, String> {
        let row = sqlx::query(
            "SELECT current_setting('transaction_read_only'),\n\
                    current_setting('default_transaction_read_only'),\n\
                    pg_is_in_recovery()",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("cannot inspect source read-only settings: {e}"))?;
        let transaction = row.try_get::<String, _>(0).map_err(|e| e.to_string())?;
        let default_transaction = row.try_get::<String, _>(1).map_err(|e| e.to_string())?;
        let in_recovery = row.try_get::<bool, _>(2).map_err(|e| e.to_string())?;
        Ok(ReadOnlyStatus {
            transaction_read_only: transaction == "on",
            default_transaction_read_only: default_transaction == "on",
            in_recovery,
        })
    }

    /// Fails closed unless the pool is transaction-read-only. A standby is
    /// acceptable but not required: an isolated snapshot can be writable at
    /// the PostgreSQL role level while this session remains unable to commit a
    /// source mutation.
    async fn ensure_read_only(&self) -> Result<(), String> {
        let status = self.read_only_status().await?;
        if !status.transaction_read_only || !status.default_transaction_read_only {
            return Err(
                "legacy source connection is not read-only; refusing migration (both transaction_read_only and default_transaction_read_only must be on)".to_string(),
            );
        }
        Ok(())
    }

    /// Write-stillness marker for freeze verification (ADR-0149 §D.2): a
    /// digest of row counts plus the maximum `LastUpdatedAt` across every
    /// table in the frozen source schema. Any insert/delete changes a count;
    /// updates to timestamped tables change their timestamp component. This
    /// is a verification signal — freeze enforcement itself is the operator
    /// procedure.
    ///
    /// # Errors
    /// Returns the database error.
    pub async fn snapshot_marker(&self) -> Result<String, String> {
        let mut material = format!("schema={};", self.fingerprint);
        for (table, has_updated_at) in SNAPSHOT_MARKER_TABLES {
            let query = if *has_updated_at {
                format!(
                    r#"SELECT COUNT(*)::bigint, MAX("LastUpdatedAt")
                       FROM "{table}""#
                )
            } else {
                format!(r#"SELECT COUNT(*)::bigint FROM "{table}""#)
            };
            // The identifier is selected exclusively from the frozen static
            // table list above; no operator input reaches this SQL.
            let row = sqlx::query(sqlx::AssertSqlSafe(query))
                .fetch_one(&self.pool)
                .await
                .map_err(|e| format!("cannot compute source snapshot marker for {table}: {e}"))?;
            let count: i64 = row.try_get(0).map_err(|e| e.to_string())?;
            material.push_str(table);
            material.push('=');
            material.push_str(&count.to_string());
            if *has_updated_at {
                let updated: Option<time::OffsetDateTime> =
                    row.try_get(1).map_err(|e| e.to_string())?;
                material.push(':');
                material.push_str(
                    &updated
                        .map(|timestamp| timestamp.unix_timestamp_nanos().to_string())
                        .unwrap_or_else(|| "none".to_string()),
                );
            }
            material.push(';');
        }
        let digest = Sha256::digest(material.as_bytes());
        Ok(format!("legacy-marker-v2:{}", hex_digest(&digest)))
    }

    /// Stable source identity for run records. The emitted value is a digest,
    /// not a host, database name, or connection URL, so evidence cannot leak
    /// private deployment topology (ADR-0148 §D.5).
    ///
    /// # Errors
    /// Returns the database error.
    pub async fn source_identity(&self) -> Result<String, String> {
        // pg_control_system needs superuser/pg_monitor; rehearsal snapshots
        // may connect with a plainer role, so fall back to stable connection
        // facts. The emitted identity below is a digest, never those facts.
        let system_id: String =
            sqlx::query_scalar("SELECT system_identifier::text FROM pg_control_system()")
                .fetch_one(&self.pool)
                .await
                .unwrap_or_else(|_| "unprivileged".to_string());
        let row = sqlx::query(
            "SELECT current_database(), current_setting('server_version'), \
                    COALESCE(host(inet_server_addr()), 'local'), \
                    COALESCE(inet_server_port()::text, 'local')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("cannot read source identity: {e}"))?;
        let db: String = row.try_get(0).map_err(|e| e.to_string())?;
        let version: String = row.try_get(1).map_err(|e| e.to_string())?;
        let host: String = row.try_get(2).map_err(|e| e.to_string())?;
        let port: String = row.try_get(3).map_err(|e| e.to_string())?;
        let material = format!(
            "system={system_id};host={host};port={port};db={db};server={version};schema={}",
            self.fingerprint
        );
        let digest = Sha256::digest(material.as_bytes());
        Ok(format!(
            "pg-source-sha256:{}:schema:{}",
            hex_digest(&digest),
            self.fingerprint
        ))
    }
}

fn hex_digest(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One table's inventory row: bounds and anomalies relevant to migration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableInventory {
    pub domain: String,
    pub source_table: String,
    pub rows: i64,
    pub min_pk: Option<i64>,
    pub max_pk: Option<i64>,
}

/// Full inventory: schema identity plus per-table counts and anomaly totals.
/// `table_count` is the number of legacy user tables in the source schema
/// (excluding `__EFMigrationsHistory`); rehearsal tooling compares it against
/// the frozen baseline evidence.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub schema_fingerprint: String,
    pub table_count: i64,
    pub tables: Vec<TableInventory>,
    pub anomalies: Vec<Anomaly>,
    pub total_media_bytes: i64,
    pub media_file_count: i64,
}

impl Inventory {
    /// Stable machine-readable shape (ADR-0148 §D.7): schema identity,
    /// per-domain table rows, anomalies, and media totals from metadata.
    pub fn json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": {
                "fingerprint": self.schema_fingerprint,
                "table_count": self.table_count,
            },
            "tables": self.tables,
            "anomalies": self.anomalies,
            "media": {
                "file_count": self.media_file_count,
                "total_bytes": self.total_media_bytes,
            },
        })
    }
}

/// A pre-import anomaly the operator must see before import.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Anomaly {
    pub code: String,
    pub count: i64,
    pub detail: String,
}

impl Inventory {
    /// Human-readable or stable JSON rendering. Credentials and absolute
    /// media-root details are redacted to counts (ADR-0148 §D.7).
    pub fn render(&self, json: bool) {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&self.json_value())
                    .unwrap_or_else(|_| "{}".to_string())
            );
            return;
        }
        println!("legacy schema fingerprint: {}", self.schema_fingerprint);
        println!("legacy user tables: {}", self.table_count);
        println!(
            "{:<26} {:>12} {:>12} {:>12}",
            "domain", "rows", "min_pk", "max_pk"
        );
        for table in &self.tables {
            println!(
                "{:<26} {:>12} {:>12} {:>12}",
                table.domain,
                table.rows,
                table.min_pk.map(|v| v.to_string()).unwrap_or_default(),
                table.max_pk.map(|v| v.to_string()).unwrap_or_default(),
            );
        }
        println!(
            "media: {} files, {} bytes (from Songs.FileSize metadata; no media scanned)",
            self.media_file_count, self.total_media_bytes
        );
        if self.anomalies.is_empty() {
            println!("anomalies: none");
        } else {
            println!("anomalies:");
            for anomaly in &self.anomalies {
                println!(
                    "  {:<28} {:>8}  {}",
                    anomaly.code, anomaly.count, anomaly.detail
                );
            }
        }
    }

    /// Renders the dry-run plan summary next to the recorded run id.
    pub fn render_plan(&self, maps: &[super::LibraryPathMap], run_id: uuid::Uuid, json: bool) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "run_id": run_id,
                    "state": "planned",
                    "library_path_maps": maps.len(),
                    "inventory": self.json_value(),
                })
            );
            return;
        }
        println!("recorded planned run {run_id}");
        println!("library path maps: {}", maps.len());
        for map in maps {
            println!("  {} -> {}", map.legacy_path, map.relative_prefix);
        }
        self.render(false);
    }
}

/// Collects the read-only inventory: per-table counts/bounds, media totals
/// from stored metadata (never by scanning the library), and anomalies
/// (duplicate public IDs, broken foreign keys, invalid timestamps,
/// unsupported credential algorithms, malformed settings).
///
/// # Errors
/// Returns the first database error with table context.
pub async fn collect_inventory(source: &SourceDb) -> Result<Inventory, String> {
    let pool = source.pool();
    let mut tables = Vec::with_capacity(INVENTORY_TABLES.len());
    for (domain, table, key_sql) in INVENTORY_TABLES {
        // Table names and key expressions come exclusively from the
        // hardcoded INVENTORY_TABLES allowlist above; there is no
        // operator-controlled interpolation. The key expressions are bigint
        // by construction (explicit ::bigint casts on the composite forms).
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            r#"SELECT COUNT(*), MIN({key_sql})::bigint, MAX({key_sql})::bigint FROM "{table}""#
        )))
        .fetch_one(pool)
        .await
        .map_err(|e| format!("inventory failed for legacy table {table}: {e}"))?;
        tables.push(TableInventory {
            domain: (*domain).to_string(),
            source_table: (*table).to_string(),
            rows: row.try_get::<i64, _>(0).map_err(|e| e.to_string())?,
            min_pk: row
                .try_get::<Option<i64>, _>(1)
                .map_err(|e| e.to_string())?,
            max_pk: row
                .try_get::<Option<i64>, _>(2)
                .map_err(|e| e.to_string())?,
        });
    }

    let mut anomalies = Vec::new();
    anomaly_count(
        pool,
        &mut anomalies,
        "catalog.duplicate_public_id",
        "rows sharing one ApiKey (public identity must be unique)",
        r#"SELECT COUNT(*) FROM (
             SELECT "ApiKey" FROM "Artists" GROUP BY "ApiKey" HAVING COUNT(*) > 1
             UNION ALL
             SELECT "ApiKey" FROM "Albums" GROUP BY "ApiKey" HAVING COUNT(*) > 1
             UNION ALL
             SELECT "ApiKey" FROM "Songs" GROUP BY "ApiKey" HAVING COUNT(*) > 1
           ) duplicates"#,
    )
    .await?;
    anomaly_count(
        pool,
        &mut anomalies,
        "catalog.broken_album_artist_fk",
        "albums referencing a missing artist",
        r#"SELECT COUNT(*) FROM "Albums" a
           WHERE NOT EXISTS (SELECT 1 FROM "Artists" ar WHERE ar."Id" = a."ArtistId")"#,
    )
    .await?;
    anomaly_count(
        pool,
        &mut anomalies,
        "catalog.broken_song_album_fk",
        "songs referencing a missing album",
        r#"SELECT COUNT(*) FROM "Songs" s
           WHERE NOT EXISTS (SELECT 1 FROM "Albums" a WHERE a."Id" = s."AlbumId")"#,
    )
    .await?;
    anomaly_count(
        pool,
        &mut anomalies,
        "user.unsupported_credential",
        "users without a portable bcrypt credential (password reset required)",
        r#"SELECT COUNT(*) FROM "Users"
           WHERE "PasswordHashAlgorithm" IS DISTINCT FROM 'bcrypt'
              OR "PasswordHash" IS NULL OR "PasswordHash" NOT LIKE '$2%'"#,
    )
    .await?;
    anomaly_count(
        pool,
        &mut anomalies,
        "catalog.invalid_timestamp",
        "rows whose LastUpdatedAt precedes CreatedAt",
        r#"SELECT COUNT(*) FROM "Songs"
           WHERE "LastUpdatedAt" IS NOT NULL AND "LastUpdatedAt" < "CreatedAt""#,
    )
    .await?;
    anomaly_count(
        pool,
        &mut anomalies,
        "media.missing_file_metadata",
        "songs without a file name or size (cannot be adopted in place)",
        r#"SELECT COUNT(*) FROM "Songs" WHERE "FileName" IS NULL OR "FileSize" IS NULL"#,
    )
    .await?;

    let media = sqlx::query(
        r#"SELECT COUNT(*), COALESCE(SUM("FileSize"), 0)::bigint FROM "Songs"
           WHERE "FileName" IS NOT NULL"#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("media totals failed: {e}"))?;

    let table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables
          WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
            AND table_name <> '__EFMigrationsHistory'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("schema table count failed: {e}"))?;

    Ok(Inventory {
        schema_fingerprint: source.fingerprint.clone(),
        table_count,
        tables,
        anomalies,
        media_file_count: media.try_get::<i64, _>(0).map_err(|e| e.to_string())?,
        total_media_bytes: media.try_get::<i64, _>(1).map_err(|e| e.to_string())?,
    })
}

async fn anomaly_count(
    pool: &PgPool,
    anomalies: &mut Vec<Anomaly>,
    code: &str,
    detail: &str,
    sql: &'static str,
) -> Result<(), String> {
    let count: i64 = sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("anomaly check {code} failed: {e}"))?;
    if count > 0 {
        anomalies.push(Anomaly {
            code: code.to_string(),
            count,
            detail: detail.to_string(),
        });
    }
    Ok(())
}
