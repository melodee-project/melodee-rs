//! Migration integration tests covering from-empty and repeatability.

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod catalog_common;
mod common;
#[path = "common/historical_catalog.rs"]
mod historical_catalog;
mod phase5_common;

use std::borrow::Cow;

use common::TestDb;
use melodee_persistence::{MIGRATOR, migrate};
use sqlx::migrate::{MigrateError, Migrator};

/// A stepwise migration test that imports catalog data runs *today's* catalog
/// code against a historical schema snapshot. The Artist status vocabulary
/// changed in migration 0044 (ADR-0190: `provisional` -> `active`), so the
/// snapshot has to accept what the current importer writes without pretending
/// 0044 has run.
async fn allow_current_artist_status_vocabulary(pool: &sqlx::PgPool) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "ALTER TABLE artists DROP CONSTRAINT IF EXISTS artists_status_check;\
         ALTER TABLE artists ADD CONSTRAINT artists_status_check \
         CHECK (status IN ('active','verified','merged','deprecated'));",
    ))
    .execute(pool)
    .await
    .expect("install the current Artist status vocabulary on the historical snapshot");
}

const COMMENT_ONLY_CANONICAL_CHECKSUMS: &[(i64, &str)] = &[
    (
        24,
        "3aa53396b0f151b3bd65ea46f0f8ac9c13a6bee852606caec13c5c324ee10642819b5abc520c1635df1b8517aa44ce53",
    ),
    (
        25,
        "4ff37ec22e6990de431a314ae814b433c708ccf5f5a4f01b707147bafe166bf18d2f904aee21312144f05dcda1eeffa3",
    ),
    (
        26,
        "d62fdba16524b8cf6d9c49b02e4da58d74459d3f5dbb6fa54a96fd032096c5805ac27ee2f8f791cd462738af59c09e77",
    ),
    (
        27,
        "f615ea59eb70cf05d9fa5cda69d5e71d3de670e1c748bcebfa38cda5e096d70de2dcc4079b41b51ad2200c22451a5952",
    ),
    (
        28,
        "fe1bb7cbbd01ce1d5e92c8b85009525e04dbfcbe99ffe0376a3fdbebda5f5951f1f44d58d29efae2e54907a1a0e6dca7",
    ),
    (
        29,
        "cbeb431a3684ba0c307e89e6f96a17e69d671bea79c5ef1b2549eaf980b03ff662250aefa0b3769d38179a61394aad15",
    ),
];
const COMMENT_ONLY_HISTORICAL_LINEAGES: &[&[(i64, &str)]] = &[
    &[
        (
            24,
            "ef2d48a7d644462526ac3b0edcae09acede7a68780bb83e5104168ede40838e813b8dafb5b68486649ea4ec15ffb9b2d",
        ),
        (
            25,
            "930549083b8d47b46209d572355e6949e8a3d35c65d8d46c289f5c6260f4132845156e41175303509dc8b3a111f753b4",
        ),
        (
            26,
            "c3683e0214abdb1e7e7455e4ad2147b5f3cd023695583b2a1d12a1f876bb9dcd5d8e90c7ce0b71122a9d6938e0b9ca91",
        ),
        (
            27,
            "a69c28d0a6c0858a1afc0e0ce22a19b14e413fc22fe3aea358fe574e0b89736cab475477127c03358ea4e0dd5313ce12",
        ),
        (
            28,
            "aff9741d36948ab4270ef82a06e812ff5e30f46f33ba7f48ce0e2774c89388bbbddf1803d9183247688b75f421ff72cf",
        ),
        (
            29,
            "e262a49d4695c56444b693f86b0ede34e20e481068533737bef5dbd5864399aafdd0e6aae22c448784d5138928c06251",
        ),
    ],
    &[
        (
            24,
            "db215eaedae3d7b073ec8e11529a80f37bcc3eb80434440a0b2b1451b186d99b53eece7913a4fff6ca9c26ae75ad4bdc",
        ),
        (
            25,
            "48e7e25d82831e4359150cec632b265e6423f87d72d8616639d3407b78a7b61b6427b258afce0c25c3e405ca817ed3fc",
        ),
        (
            26,
            "d7b2aca713222fa08e0d17ff68548c20834cf6a597ec516bbbdd20f528be201b0cd681d717bae60e6d7ddd2a7da6f362",
        ),
        (
            27,
            "2afde0c080b0d4ed0d62e64bedeea40d1c573a5f3b490a24692ac18c4a970e9bdf0fd82c360e8e6f62465d4770b801d4",
        ),
        (
            28,
            "dd2445592ab91b0fe06460383364b19bf20cde4c61fcec17a21332494402553b0e772fbf9121536c3f3ab0ef7e3e561e",
        ),
        (
            29,
            "b8b31795fcb8a3e6ad10e822ef31edf5c4852218f5c592b07a1465e6afe3eeb46f5733024347dffa4cdf817de1046151",
        ),
    ],
    &[
        (
            24,
            "519fcfb3fea9a11333051a96aa48eb098a230ae017b79bc06e3f836e18731ba74dd9ad372f6191278b17900c0dca5c86",
        ),
        (
            25,
            "67df569fcd50efd7c45e4901e719c5634ab5cffc9320961ad73fd780b8d64e44313122265af23b4e1ab6b2645ab1ca0b",
        ),
        (
            26,
            "da94503238dbc6bc5d2af03aa0e3ad1979a22ef2e527bf3f6029ed12610cf7fd0a983a4a8feea7451b45fa9be4ceb32d",
        ),
        (
            27,
            "b48fe85eeff029b19660b46fd3744d3fd8d0ffda9a7de9dae1853b23e596f11172ea3cd4b976a980ac5f530d33472465",
        ),
        (
            28,
            "7bb98df26fef9cce58c1807f9f2a1c1e1f5ef8d415507dc17937a9028d3eb065ab63993c13aa7b1cbcbfaa7a51dd4415",
        ),
        (
            29,
            "b8b31795fcb8a3e6ad10e822ef31edf5c4852218f5c592b07a1465e6afe3eeb46f5733024347dffa4cdf817de1046151",
        ),
    ],
];

#[tokio::test]
async fn migrations_run_from_empty_database() {
    let db = TestDb::create("migrations_empty").await;
    // Tables exist and are queryable.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_items")
        .fetch_one(&db.pool)
        .await
        .expect("query work items");
    assert_eq!(count.0, 0);
    let events: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM release_work_item_events")
        .fetch_one(&db.pool)
        .await
        .expect("query events");
    assert_eq!(events.0, 0);
    db.drop().await;
}

#[tokio::test]
async fn migrations_are_repeatable() {
    let db = TestDb::create("migrations_repeat").await;
    migrate(&db.pool).await.expect("second migrate run");
    migrate(&db.pool).await.expect("third migrate run");
    db.drop().await;
}

#[test]
fn migrations_0024_through_0029_canonical_checksums_are_frozen() {
    for (version, expected_checksum) in COMMENT_ONLY_CANONICAL_CHECKSUMS {
        let migration = MIGRATOR
            .migrations
            .iter()
            .find(|migration| migration.version == *version)
            .expect("canonical migration");
        assert_eq!(
            checksum_hex(migration.checksum.as_ref()),
            *expected_checksum,
            "applied migrations are immutable; add a new migration instead"
        );
    }
}

#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn migrations_0024_through_0029_accept_exact_comment_only_histories() {
    for (index, lineage) in COMMENT_ONLY_HISTORICAL_LINEAGES.iter().enumerate() {
        let db = TestDb::create_unmigrated(&format!("migration_comment_history_{index}")).await;
        let checksums: Vec<(i64, Vec<u8>)> = lineage
            .iter()
            .map(|(version, checksum)| (*version, decode_checksum(checksum)))
            .collect();
        migrator_with_checksums(&checksums)
            .run(&db.pool)
            .await
            .expect("apply schema with historical migration metadata");

        migrate(&db.pool)
            .await
            .expect("known comment-only checksums remain upgradeable");
        let stored_checksums: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT version, checksum FROM _sqlx_migrations \
             WHERE version BETWEEN 24 AND 29 ORDER BY version",
        )
        .fetch_all(&db.pool)
        .await
        .expect("read stored migration checksums");
        assert_eq!(
            stored_checksums, checksums,
            "compatibility must not rewrite migration metadata"
        );
        db.drop().await;
    }
}

#[tokio::test]
async fn comment_only_migration_compatibility_rejects_an_unknown_checksum() {
    let db = TestDb::create_unmigrated("migration_unknown_checksum").await;
    migrator_with_checksums(&[(25, vec![0xa5; 48])])
        .run(&db.pool)
        .await
        .expect("apply schema with deliberately unknown migration metadata");

    let error = migrate(&db.pool)
        .await
        .expect_err("unknown checksum must fail closed");
    assert!(matches!(error, MigrateError::VersionMismatch(25)));
    db.drop().await;
}

fn migrator_with_checksums(checksums: &[(i64, Vec<u8>)]) -> Migrator {
    let mut migrations = MIGRATOR.migrations.to_vec();
    for (version, checksum) in checksums {
        migrations
            .iter_mut()
            .find(|migration| migration.version == *version)
            .expect("migration")
            .checksum = Cow::Owned(checksum.clone());
    }
    Migrator {
        migrations: Cow::Owned(migrations),
        ignore_missing: MIGRATOR.ignore_missing,
        locking: MIGRATOR.locking,
        no_tx: MIGRATOR.no_tx,
        table_name: MIGRATOR.table_name.clone(),
        create_schemas: MIGRATOR.create_schemas.clone(),
    }
}

fn decode_checksum(checksum: &str) -> Vec<u8> {
    (0..checksum.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&checksum[index..index + 2], 16).expect("valid checksum hex")
        })
        .collect()
}

fn checksum_hex(checksum: &[u8]) -> String {
    checksum.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn job_history_is_durable_idempotent_and_operator_audited() {
    let db = TestDb::create("migrations_job_history").await;
    let store = melodee_persistence::JobRunHistoryStore;
    let now = time::OffsetDateTime::now_utc();
    let tick = melodee_persistence::scheduled_tick(now, std::time::Duration::from_secs(60));

    let first = store
        .start_tick(&db.pool, "discovery", tick)
        .await
        .expect("start first tick")
        .expect("first process owns tick");
    assert!(
        store
            .start_tick(&db.pool, "discovery", tick)
            .await
            .expect("duplicate tick insert")
            .is_none()
    );

    let finished = store
        .finish(
            &db.pool,
            first.id,
            now + time::Duration::milliseconds(25),
            false,
            Some("fixture failure"),
        )
        .await
        .expect("finish failed tick");
    assert_eq!(finished.success, Some(false));
    assert_eq!(finished.error_message.as_deref(), Some("fixture failure"));

    let manual = store
        .start_manual(&db.pool, "discovery", "admin-fixture", now)
        .await
        .expect("start manual run");
    store
        .finish(&db.pool, manual.id, now, true, None)
        .await
        .expect("finish manual run");
    let (rows, total) = store
        .list(&db.pool, Some("discovery"), 50, 0)
        .await
        .expect("list job history");
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        store
            .failure_count_since(&db.pool, now - time::Duration::hours(1))
            .await
            .expect("failure summary"),
        1
    );
    assert_eq!(
        store
            .cleanup(&db.pool, 1, "admin-fixture", now)
            .await
            .expect("cleanup history"),
        0
    );
    let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM job_run_history_cleanup_audit")
        .fetch_one(&db.pool)
        .await
        .expect("cleanup audit count");
    assert_eq!(audit_count, 1);
    db.drop().await;
}

#[tokio::test]
async fn job_history_advisory_claim_allows_one_process_per_tick() {
    let db = TestDb::create("migrations_job_history_advisory").await;
    let store = melodee_persistence::JobRunHistoryStore;
    let tick = melodee_persistence::scheduled_tick(
        time::OffsetDateTime::now_utc(),
        std::time::Duration::from_secs(60),
    );
    let (left, right) = tokio::join!(
        store.start_tick(&db.pool, "discovery", tick),
        store.start_tick(&db.pool, "discovery", tick),
    );
    let claimed = [left.expect("left claim"), right.expect("right claim")]
        .into_iter()
        .filter(Option::is_some)
        .count();
    assert_eq!(claimed, 1);
    db.drop().await;
}

#[tokio::test]
async fn metadata_edit_audits_preserve_field_deltas_and_reject_invalid_states() {
    let db = TestDb::create("migrations_metadata_edit_audits").await;
    let release_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            release_date, status, revision, created_at, updated_at
         ) VALUES (uuidv7(), uuidv7(), 'Audit Release', 'audit release',
                   'Audit Release', DATE '2024-01-01', 'active', 1, now(), now())
         RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seed metadata audit release");
    let audit_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO metadata_edit_audits (
            id, release_id, operation_name, operation_params, affected_files,
            field_deltas, applied_by, status, recovery_evidence, created_at
         ) VALUES ($1,$2,'set_year','{}',1,
                   '[{\"field\":\"recordingDate\",\"old\":\"2023\",\"new\":\"2024\"}]',
                   'audit-operator','success','{\"files\":[]}',now())",
    )
    .bind(audit_id)
    .bind(release_id)
    .execute(&db.pool)
    .await
    .expect("insert metadata audit");
    let stored: (String, i32, serde_json::Value) = sqlx::query_as(
        "SELECT status, affected_files, field_deltas
         FROM metadata_edit_audits WHERE id=$1",
    )
    .bind(audit_id)
    .fetch_one(&db.pool)
    .await
    .expect("read metadata audit");
    assert_eq!(stored.0, "success");
    assert_eq!(stored.1, 1);
    assert_eq!(stored.2[0]["field"], "recordingDate");

    let invalid_status = sqlx::query(
        "INSERT INTO metadata_edit_audits (
            id, release_id, operation_name, operation_params, affected_files,
            field_deltas, applied_by, status, created_at
         ) VALUES (uuidv7(),$1,'set_year','{}',0,'[]','audit-operator','unknown',now())",
    )
    .bind(release_id)
    .execute(&db.pool)
    .await;
    assert!(
        invalid_status.is_err(),
        "unknown audit states must fail closed"
    );

    let delete_release = sqlx::query("DELETE FROM releases WHERE id=$1")
        .bind(release_id)
        .execute(&db.pool)
        .await;
    assert!(
        delete_release.is_err(),
        "audit history must prevent release deletion"
    );
    db.drop().await;
}

#[tokio::test]
async fn schema_rejects_impossible_states() {
    let db = TestDb::create("migrations_constraints").await;

    // Unknown state is rejected by the CHECK constraint.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'bogus',
                  gen_random_uuid(), now(), now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "unknown state must be rejected");

    // Lease inconsistency is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at, lease_owner
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'ready',
                  gen_random_uuid(), now(), now(), now(), 'worker-without-timestamps')",
    )
    .execute(&db.pool)
    .await;
    assert!(
        result.is_err(),
        "lease owner without timestamps must be rejected"
    );

    // Negative attempt count is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at, attempt_count
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'ready',
                  gen_random_uuid(), now(), now(), now(), -1)",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "negative attempt_count must be rejected");

    // Terminal state without completed_at is rejected.
    let result = sqlx::query(
        "INSERT INTO release_work_items (
            id, api_key, source_path, current_path, state, correlation_id,
            discovered_at, updated_at, created_at
        ) VALUES (gen_random_uuid(), gen_random_uuid(), '/x', '/x', 'indexed',
                  gen_random_uuid(), now(), now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(
        result.is_err(),
        "terminal state without completed_at must be rejected"
    );

    db.drop().await;
}

/// ADR-0142 / migration 0029 data integrity: every admin browse index exists
/// as a FULL (non-partial) index on the intended table, the two nullable sort
/// columns carry the stores' DESC NULLS LAST ordering, and the browse stores
/// still return correct active-identity results once the indexes are present.
#[tokio::test]
async fn admin_browse_indexes_are_full_and_browse_results_stay_correct() {
    let db = TestDb::create("migrations_admin_browse_indexes").await;

    let expected: [(&str, &str); 13] = [
        ("artists", "artists_admin_browse_name_idx"),
        ("artists", "artists_admin_browse_created_idx"),
        ("artists", "artists_admin_browse_updated_idx"),
        ("releases", "releases_admin_browse_title_idx"),
        ("releases", "releases_admin_browse_date_idx"),
        ("releases", "releases_admin_browse_created_idx"),
        ("releases", "releases_admin_browse_updated_idx"),
        ("releases", "releases_admin_search_title_idx"),
        ("tracks", "tracks_admin_browse_duration_idx"),
        (
            "observed_artists",
            "observed_artists_admin_queue_created_idx",
        ),
        (
            "observed_artists",
            "observed_artists_admin_queue_display_idx",
        ),
        (
            "release_work_items",
            "release_work_items_admin_browse_created_idx",
        ),
        (
            "artist_match_decisions",
            "artist_match_decisions_observed_outcome_idx",
        ),
    ];
    for (table, index) in expected {
        let definition: Option<String> = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes
              WHERE schemaname = 'public' AND tablename = $1 AND indexname = $2",
        )
        .bind(table)
        .bind(index)
        .fetch_optional(&db.pool)
        .await
        .expect("query pg_indexes");
        let definition = definition.unwrap_or_else(|| panic!("missing index {index}"));
        assert!(
            !definition.to_ascii_uppercase().contains(" WHERE "),
            "admin browse index {index} must be FULL (non-partial): {definition}"
        );
    }
    // The nullable sort columns must match the stores' NULLS LAST ordering; a
    // plain btree cannot serve ORDER BY … DESC NULLS LAST (ADR-0142 finding 2).
    for index in [
        "releases_admin_browse_date_idx",
        "tracks_admin_browse_duration_idx",
    ] {
        let definition: String = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes WHERE schemaname = 'public' AND indexname = $1",
        )
        .bind(index)
        .fetch_one(&db.pool)
        .await
        .expect("query pg_indexes");
        assert!(
            definition.contains("DESC NULLS LAST"),
            "index {index} must carry DESC NULLS LAST ordering: {definition}"
        );
    }

    // Store-level correctness: the unfiltered admin listing excludes retired
    // identity tombstones and orders by sort_name with the id tiebreak.
    let now = time::OffsetDateTime::now_utc();
    let seed = |canonical: &str, status: melodee_domain::ArtistStatus| {
        melodee_domain::Artist::new(melodee_domain::NewArtist {
            id: melodee_domain::ArtistId::new(),
            public_api_id: melodee_domain::ArtistPublicId::from_uuid(uuid::Uuid::now_v7()),
            canonical_name: canonical.to_string(),
            normalized_name: canonical.to_lowercase(),
            sort_name: canonical.to_string(),
            artist_type: melodee_domain::ArtistType::Person,
            disambiguation: None,
            active_dates: None,
            status,
            created_at: now,
        })
        .expect("valid artist")
    };
    for artist in [
        seed("Beta", melodee_domain::ArtistStatus::Verified),
        seed("Alpha", melodee_domain::ArtistStatus::Deprecated),
        seed("Gamma", melodee_domain::ArtistStatus::Active),
    ] {
        melodee_persistence::CatalogStore
            .create_artist(&db.pool, &artist)
            .await
            .expect("create artist");
    }
    let (rows, total) = melodee_persistence::CatalogStore
        .list_catalog_artists(
            &db.pool,
            None,
            50,
            0,
            melodee_persistence::CatalogArtistSort::NameAsc,
        )
        .await
        .expect("list catalog artists");
    assert_eq!(total, 2, "deprecated Artist tombstones are not browse rows");
    let names: Vec<&str> = rows.iter().map(|row| row.canonical_name.as_str()).collect();
    assert_eq!(names, ["Beta", "Gamma"]);

    db.drop().await;
}

/// ADR-0167 / migration 0039: native substring predicates and their trigram
/// indexes must name the same normalized columns. Display-column indexes do
/// not make these expressions indexable.
#[tokio::test]
async fn native_normalized_search_indexes_match_query_predicates() {
    let db = TestDb::create("migrations_native_normalized_search").await;
    for (index, column, partial_predicate) in [
        (
            "artists_native_search_normalized_name_idx",
            "normalized_name",
            Some("status"),
        ),
        (
            "artist_aliases_native_search_normalized_alias_idx",
            "normalized_alias",
            None,
        ),
        (
            "releases_native_search_normalized_title_idx",
            "normalized_title",
            Some("status"),
        ),
        (
            "tracks_native_search_normalized_title_idx",
            "normalized_title",
            None,
        ),
    ] {
        let definition: String = sqlx::query_scalar(
            "SELECT indexdef FROM pg_indexes
             WHERE schemaname = 'public' AND indexname = $1",
        )
        .bind(index)
        .fetch_one(&db.pool)
        .await
        .expect("read native normalized search index");
        assert!(
            definition.contains("USING gin"),
            "{index} must be a GIN index: {definition}"
        );
        assert!(
            definition.contains(&format!("{column} gin_trgm_ops")),
            "{index} does not match its query predicate: {definition}"
        );
        assert_eq!(
            definition.contains(" WHERE "),
            partial_predicate.is_some(),
            "{index} partial-index shape changed: {definition}"
        );
        if let Some(predicate) = partial_predicate {
            assert!(definition.contains(predicate));
        }
    }
    db.drop().await;
}

/// ADR-0143 / migration 0030 data integrity: `catalog_entity_images` enforces
/// its entity-type domain, typed-column pairing, mime domain, and the
/// 1 byte–10 MiB byte bound at the schema level; the typed partial unique
/// indexes give one override per entity; FKs reject dangling entities. The
/// `CatalogImageStore` round trip (upsert → read → replace → delete) and its
/// atomic audit evidence are covered here at the lowest layer.
#[tokio::test]
async fn catalog_entity_images_schema_enforces_bounds_and_store_round_trip() {
    let db = TestDb::create("migrations_catalog_entity_images").await;

    // Seed one Artist to anchor typed FK references.
    let now = time::OffsetDateTime::now_utc();
    let artist = melodee_domain::Artist::new(melodee_domain::NewArtist {
        id: melodee_domain::ArtistId::new(),
        public_api_id: melodee_domain::ArtistPublicId::from_uuid(uuid::Uuid::now_v7()),
        canonical_name: "Override Artist".to_string(),
        normalized_name: "override artist".to_string(),
        sort_name: "Override Artist".to_string(),
        artist_type: melodee_domain::ArtistType::Person,
        disambiguation: None,
        active_dates: None,
        status: melodee_domain::ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid artist");
    melodee_persistence::CatalogStore
        .create_artist(&db.pool, &artist)
        .await
        .expect("create artist");
    let artist_id = artist.id().into_uuid();

    // Schema-level rejections (no invalid row may be constructed).
    let valid = melodee_persistence::asset_files::put(&db.pool, b"png")
        .await
        .expect("store fixture");
    let columns = "entity_type, artist_id, release_id, image_mime_type, asset_key, \
                   uploaded_by, created_at, updated_at";
    let dangling = uuid::Uuid::now_v7();
    let cases: [(&str, String); 8] = [
        (
            "unknown entity_type",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'track', '{artist_id}', NULL, 'image/png', '{valid}',
                 'admin', now(), now())"
            ),
        ),
        (
            "entity_type/typed-column mismatch",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', NULL, '{dangling}', 'image/png', '{valid}',
                 'admin', now(), now())"
            ),
        ),
        (
            "no typed column set",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', NULL, NULL, 'image/png', '{valid}',
                 'admin', now(), now())"
            ),
        ),
        (
            "unsupported mime type",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/gif', '{valid}',
                 'admin', now(), now())"
            ),
        ),
        (
            "empty bytes",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/png', '',
                 'admin', now(), now())"
            ),
        ),
        (
            "bytes beyond the 10 MiB bound",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/png',
                 repeat('0',64) || '-10485761', 'admin', now(), now())"
            ),
        ),
        (
            "dangling artist FK",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{dangling}', NULL, 'image/png', '{valid}',
                 'admin', now(), now())"
            ),
        ),
        (
            "dangling release FK",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'release', NULL, '{dangling}', 'image/png', '{valid}',
                 'admin', now(), now())"
            ),
        ),
    ];
    for (case, statement) in cases {
        // Statements are built from fixed templates plus generated UUIDs.
        let result = sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&db.pool)
            .await;
        assert!(result.is_err(), "{case} must be rejected");
    }

    // Store-level rejections stay ahead of the schema backstop.
    let oversized = vec![0_u8; melodee_persistence::MAX_ENTITY_IMAGE_BYTES + 1];
    let result = melodee_persistence::CatalogImageStore::set_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        "image/gif",
        b"GIF89a",
        melodee_persistence::CatalogImageAudit {
            actor: "admin",
            reason: "test",
            now,
        },
    )
    .await;
    assert!(
        matches!(
            result,
            Err(melodee_persistence::CatalogImageError::InvalidRequest(_))
        ),
        "unsupported mime must be rejected by the store"
    );
    let result = melodee_persistence::CatalogImageStore::set_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        "image/png",
        &oversized,
        melodee_persistence::CatalogImageAudit {
            actor: "admin",
            reason: "test",
            now,
        },
    )
    .await;
    assert!(
        matches!(
            result,
            Err(melodee_persistence::CatalogImageError::InvalidRequest(_))
        ),
        "oversized bytes must be rejected by the store"
    );

    // Round trip: upsert → read → replace → idempotent delete.
    let first = melodee_persistence::CatalogImageStore::set_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        "image/png",
        b"\x89PNG-first",
        melodee_persistence::CatalogImageAudit {
            actor: "admin-public-id",
            reason: "operator artwork upload",
            now,
        },
    )
    .await
    .expect("set image");
    assert_eq!(first.image_mime_type, "image/png");
    assert_eq!(first.image_bytes, b"\x89PNG-first");
    assert_eq!(first.uploaded_by, "admin-public-id");

    // The typed unique index rejects a second raw row for the same entity.
    let duplicate = sqlx::query(
        "INSERT INTO catalog_entity_images (
            entity_type, artist_id, image_mime_type, asset_key,
            uploaded_by, created_at, updated_at
         ) VALUES ('artist', $1, 'image/png', $2, 'admin', now(), now())",
    )
    .bind(artist_id)
    .bind(&valid)
    .execute(&db.pool)
    .await;
    assert!(duplicate.is_err(), "one override per entity must hold");

    let replaced = melodee_persistence::CatalogImageStore::set_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        "image/jpeg",
        b"\xff\xd8\xff-replacement",
        melodee_persistence::CatalogImageAudit {
            actor: "admin-public-id",
            reason: "operator artwork replacement",
            now,
        },
    )
    .await
    .expect("replace image");
    assert_eq!(replaced.image_bytes, b"\xff\xd8\xff-replacement");
    assert_eq!(replaced.created_at, first.created_at);
    let stored = melodee_persistence::CatalogImageStore::image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
    )
    .await
    .expect("read image")
    .expect("override present");
    assert_eq!(stored, replaced);

    let removed = melodee_persistence::CatalogImageStore::delete_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        melodee_persistence::CatalogImageAudit {
            actor: "admin-public-id",
            reason: "artwork no longer wanted",
            now,
        },
    )
    .await
    .expect("delete image");
    assert!(removed, "existing override must report removal");
    let absent = melodee_persistence::CatalogImageStore::delete_image(
        &db.pool,
        melodee_persistence::CatalogImageEntity::Artist,
        artist_id,
        melodee_persistence::CatalogImageAudit {
            actor: "admin-public-id",
            reason: "artwork no longer wanted",
            now,
        },
    )
    .await
    .expect("repeat delete");
    assert!(
        !absent,
        "deleting an absent override is a no-op (ADR-0143 §4)"
    );
    assert!(
        melodee_persistence::CatalogImageStore::image(
            &db.pool,
            melodee_persistence::CatalogImageEntity::Artist,
            artist_id,
        )
        .await
        .expect("read after delete")
        .is_none()
    );

    // Atomic audit evidence: two sets + one removal, all attributed to the
    // acting administrator; the no-op delete wrote nothing.
    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, actor FROM catalog_audit_events
         WHERE entity_type = 'artist' AND entity_id = $1
           AND event_type LIKE 'image_%'
         ORDER BY created_at, event_id",
    )
    .bind(artist_id)
    .fetch_all(&db.pool)
    .await
    .expect("audit events");
    let event_types: Vec<&str> = events.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        event_types,
        [
            "image_override_set",
            "image_override_set",
            "image_override_removed"
        ]
    );
    assert!(
        events.iter().all(|(_, actor)| actor == "admin-public-id"),
        "every mutation carries the acting administrator id"
    );

    db.drop().await;
}

/// ADR-0148 / migration 0032: a database sitting at the pre-contract schema
/// (migrations 0001–0031) with representative production-shaped rows upgrades
/// forward cleanly, the `legacy_migration` schema and
/// `releases.original_release_date` appear, and no pre-existing row content
/// is altered by the upgrade.
#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn migration_0032_forward_from_prior_schema_preserves_existing_data() {
    use melodee_persistence::MIGRATOR;
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("migrations_forward_0032").await;

    // Apply exactly the schema versions that existed before 0032.
    let prior: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 31)
        .cloned()
        .collect();
    assert_eq!(prior.len(), 31, "expected migrations 0001..=0031");
    let prior_migrator = Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    };
    prior_migrator
        .run(&db.pool)
        .await
        .expect("apply pre-0032 schema");

    // Representative pre-0032 data: one artist, one release with a filled
    // edition date, one work item. Content is asserted verbatim after the
    // forward migration.
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, revision, created_at, updated_at
         ) VALUES (uuidv7(), '99999999-9999-9999-9999-999999999999',
                   'Forward Artist', 'forward artist', 'Forward Artist',
                   'group', 'verified', 1, now(), now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed artist");
    sqlx::query(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            release_date, status, revision, created_at, updated_at
         ) VALUES (uuidv7(), '88888888-8888-8888-8888-888888888888',
                   'Forward Release', 'forward release', 'Forward Release',
                   DATE '2001-09-11', 'active', 1, now(), now())",
    )
    .execute(&db.pool)
    .await
    .expect("seed release");

    // The forward migration applies only 0032.
    migrate(&db.pool).await.expect("forward migration to 0032");

    // New contract objects exist.
    let schema_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.schemata
          WHERE schema_name = 'legacy_migration')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("schema check");
    assert!(schema_exists, "legacy_migration schema must exist");
    let original: Option<time::Date> = sqlx::query_scalar(
        "SELECT original_release_date FROM releases
         WHERE public_api_id = '88888888-8888-8888-8888-888888888888'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("original_release_date column must exist");
    assert_eq!(original, None, "existing rows gain NULL, never a rewrite");

    // Existing row content is byte-identical after the upgrade.
    let row: (String, Option<time::Date>) = sqlx::query_as(
        "SELECT title, release_date FROM releases
         WHERE public_api_id = '88888888-8888-8888-8888-888888888888'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("release after upgrade");
    assert_eq!(row.0, "Forward Release");
    assert_eq!(
        row.1.map(|d| d.year()),
        Some(2001),
        "existing edition date untouched"
    );
    let artist_name: String = sqlx::query_scalar(
        "SELECT canonical_name FROM artists
         WHERE public_api_id = '99999999-9999-9999-9999-999999999999'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("artist after upgrade");
    assert_eq!(artist_name, "Forward Artist");

    db.drop().await;
}

/// ADR-0153 / migration 0034: an existing importer-created Track Artist
/// identity is redirected to the sole exact matching Release Artist without
/// discarding either raw observation or the prior append-only decision.
#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn migration_0034_repairs_only_the_same_release_artist_identity() {
    use melodee_persistence::MIGRATOR;
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("migrations_forward_0034_artist_repair").await;
    let prior: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 33)
        .cloned()
        .collect();
    assert_eq!(prior.len(), 33, "expected migrations 0001..=0033");
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("apply pre-0034 schema");

    sqlx::raw_sql(sqlx::AssertSqlSafe(
        r#"
        DO $$
        DECLARE
            release_id uuid := uuidv7();
            target_artist_id uuid := uuidv7();
            source_artist_id uuid := uuidv7();
            audio_id uuid := uuidv7();
            media_id uuid := uuidv7();
            track_id uuid := uuidv7();
            release_observation_id uuid := uuidv7();
            track_observation_id uuid := uuidv7();
            release_artist_row_id uuid := uuidv7();
            track_artist_row_id uuid := uuidv7();
            release_decision_id uuid := uuidv7();
            track_decision_id uuid := uuidv7();
        BEGIN
            INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, revision, created_at, updated_at
            ) VALUES
                (target_artist_id, uuidv7(), 'Exact Artist', 'exact artist',
                 'Exact Artist', 'group', 'provisional', 1, now(), now()),
                (source_artist_id, uuidv7(), 'Exact Artist', 'exact artist',
                 'Exact Artist', 'group', 'provisional', 1, now(), now());
            INSERT INTO releases (
                id, public_api_id, title, normalized_title, sort_title,
                status, revision, created_at, updated_at
            ) VALUES (
                release_id, uuidv7(), 'Repair Release', 'repair release',
                'Repair Release', 'active', 1, now(), now()
            );
            INSERT INTO audio_identities (
                id, status, revision, created_at, updated_at
            ) VALUES (audio_id, 'provisional', 1, now(), now());
            INSERT INTO media_assets (
                id, canonical_relative_path, byte_hash, byte_size, mime_type, created_at
            ) VALUES (
                media_id, 'releases/repair/track.flac', repeat('a', 64), 1,
                'audio/flac', now()
            );
            INSERT INTO tracks (
                id, public_api_id, release_id, audio_identity_id,
                playable_media_asset_id, ordinal, title, normalized_title,
                sort_title, revision, created_at, updated_at
            ) VALUES (
                track_id, uuidv7(), release_id, audio_id, media_id, 1,
                'Repair Track', 'repair track', 'Repair Track', 1, now(), now()
            );
            INSERT INTO track_media_assets (
                track_id, media_asset_id, purpose, ordinal, created_at
            ) VALUES (track_id, media_id, 'playable', 1, now());
            INSERT INTO observed_artists (
                id, release_id, track_id, scope, source_ordinal, raw_value,
                normalized_value, display_value, source_pointer, source_payload, created_at
            ) VALUES
                (release_observation_id, release_id, NULL, 'release', 1,
                 'Exact Artist', 'exact artist', 'Exact Artist',
                 '/release_observation/artists/0', '{}'::jsonb, now()),
                (track_observation_id, release_id, track_id, 'track', 1,
                 'Exact Artist', 'exact artist', 'Exact Artist',
                 '/tracks/0/artist', '{}'::jsonb, now());
            INSERT INTO release_artists (
                id, release_id, artist_id, observed_artist_id, role,
                presentation_order, display_value, join_phrase, created_at
            ) VALUES (
                release_artist_row_id, release_id, target_artist_id,
                release_observation_id, 'primary', 1, 'Exact Artist', '', now()
            );
            INSERT INTO track_artists (
                id, track_id, artist_id, observed_artist_id, role,
                presentation_order, display_value, join_phrase, created_at
            ) VALUES (
                track_artist_row_id, track_id, source_artist_id,
                track_observation_id, 'primary', 1, 'Exact Artist', '', now()
            );
            INSERT INTO artist_match_decisions (
                id, observed_artist_id, artist_id, outcome, authority,
                confidence, reason, is_locked, payload, created_at
            ) VALUES
                (release_decision_id, release_observation_id, target_artist_id,
                 'confirmed', 'automated', 6000,
                 'created a distinct provisional identity from manifest context',
                 false, '{}'::jsonb, now()),
                (track_decision_id, track_observation_id, source_artist_id,
                 'confirmed', 'automated', 6000,
                 'created a distinct provisional identity from manifest context',
                 false, '{}'::jsonb, now());
            INSERT INTO artist_match_current (
                observed_artist_id, decision_id, updated_at
            ) VALUES
                (release_observation_id, release_decision_id, now()),
                (track_observation_id, track_decision_id, now());
            INSERT INTO artist_track_associations (
                artist_id, track_id, association_type, participation_kind,
                source_key, source_artist_id, confidence, is_performance, created_at
            ) VALUES (
                source_artist_id, track_id, 'primary', 'direct',
                track_artist_row_id, NULL, 6000, true, now()
            );
        END $$;
        "#,
    ))
    .execute(&db.pool)
    .await
    .expect("seed pre-0034 importer identity split");

    migrate(&db.pool).await.expect("forward migration to 0034");

    let rows: Vec<(uuid::Uuid, String, Option<uuid::Uuid>)> =
        sqlx::query_as("SELECT id, status, merged_into_artist_id FROM artists ORDER BY status, id")
            .fetch_all(&db.pool)
            .await
            .expect("read repaired Artists");
    assert_eq!(rows.len(), 2, "the audit tombstone is retained");
    let source = rows
        .iter()
        .find(|(_, status, _)| status == "merged")
        .expect("merged source Artist");
    let target = rows
        .iter()
        .find(|(_, status, _)| status == "active")
        .expect("visible target Artist");
    assert_eq!(source.2, Some(target.0));
    assert_eq!(
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT artist_id FROM track_artists")
            .fetch_one(&db.pool)
            .await
            .expect("Track Artist identity"),
        target.0
    );
    let current: (uuid::Uuid, String) = sqlx::query_as(
        "SELECT decision.artist_id, decision.reason
           FROM observed_artists observed
           JOIN artist_match_current current_match
             ON current_match.observed_artist_id = observed.id
           JOIN artist_match_decisions decision ON decision.id = current_match.decision_id
          WHERE observed.scope = 'track'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("repaired current match");
    assert_eq!(current.0, target.0);
    assert_eq!(
        current.1,
        "resolved to the sole matching Release Artist from manifest context"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM observed_artists")
            .fetch_one(&db.pool)
            .await
            .expect("observations retained"),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artist_match_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("decision history retained"),
        3
    );

    db.drop().await;
}

/// ADR-0170 / migration 0040: a Track identity created under the old
/// singleton-only rule is redirected when exactly one of several primary
/// Release Artist observations matches. The second billing identity and all
/// immutable observation/decision evidence remain intact.
#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn migration_0040_repairs_a_unique_match_on_a_multi_artist_release() {
    use melodee_persistence::MIGRATOR;
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("migrations_forward_0040_multi_artist_repair").await;
    let prior: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 39)
        .cloned()
        .collect();
    assert_eq!(prior.len(), 39, "expected migrations 0001..=0039");
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("apply pre-0040 schema");

    sqlx::raw_sql(sqlx::AssertSqlSafe(
        r#"
        DO $$
        DECLARE
            release_id uuid := uuidv7();
            target_artist_id uuid := uuidv7();
            other_artist_id uuid := uuidv7();
            source_artist_id uuid := uuidv7();
            audio_id uuid := uuidv7();
            media_id uuid := uuidv7();
            track_id uuid := uuidv7();
            target_observation_id uuid := uuidv7();
            other_observation_id uuid := uuidv7();
            track_observation_id uuid := uuidv7();
            target_release_artist_row_id uuid := uuidv7();
            other_release_artist_row_id uuid := uuidv7();
            track_artist_row_id uuid := uuidv7();
            target_decision_id uuid := uuidv7();
            other_decision_id uuid := uuidv7();
            track_decision_id uuid := uuidv7();
        BEGIN
            INSERT INTO artists (
                id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, status, revision, created_at, updated_at
            ) VALUES
                (target_artist_id, uuidv7(), '$NOT', '$not', '$NOT',
                 'group', 'provisional', 1, now(), now()),
                (other_artist_id, uuidv7(), '$NOT/Jeht', '$not/jeht', '$NOT/Jeht',
                 'group', 'provisional', 1, now(), now()),
                (source_artist_id, uuidv7(), '$NOT', '$not', '$NOT',
                 'group', 'provisional', 1, now(), now());
            INSERT INTO releases (
                id, public_api_id, title, normalized_title, sort_title,
                status, revision, created_at, updated_at
            ) VALUES (
                release_id, uuidv7(), 'High Risk', 'high risk',
                'High Risk', 'active', 1, now(), now()
            );
            INSERT INTO audio_identities (
                id, status, revision, created_at, updated_at
            ) VALUES (audio_id, 'provisional', 1, now(), now());
            INSERT INTO media_assets (
                id, canonical_relative_path, byte_hash, byte_size, mime_type, created_at
            ) VALUES (
                media_id, 'releases/multi-artist/track.flac', repeat('b', 64), 1,
                'audio/flac', now()
            );
            INSERT INTO tracks (
                id, public_api_id, release_id, audio_identity_id,
                playable_media_asset_id, ordinal, title, normalized_title,
                sort_title, revision, created_at, updated_at
            ) VALUES (
                track_id, uuidv7(), release_id, audio_id, media_id, 1,
                'Locked In', 'locked in', 'Locked In', 1, now(), now()
            );
            INSERT INTO track_media_assets (
                track_id, media_asset_id, purpose, ordinal, created_at
            ) VALUES (track_id, media_id, 'playable', 1, now());
            INSERT INTO observed_artists (
                id, release_id, track_id, scope, source_ordinal, raw_value,
                normalized_value, display_value, source_pointer, source_payload, created_at
            ) VALUES
                (target_observation_id, release_id, NULL, 'release', 1,
                 '$NOT', '$not', '$NOT',
                 '/release_observation/artists/0', '{}'::jsonb, now()),
                (other_observation_id, release_id, NULL, 'release', 2,
                 '$NOT/Jeht', '$not/jeht', '$NOT/Jeht',
                 '/release_observation/artists/1', '{}'::jsonb, now()),
                (track_observation_id, release_id, track_id, 'track', 1,
                 '$NOT', '$not', '$NOT',
                 '/tracks/0/artist', '{}'::jsonb, now());
            INSERT INTO release_artists (
                id, release_id, artist_id, observed_artist_id, role,
                presentation_order, display_value, join_phrase, created_at
            ) VALUES
                (target_release_artist_row_id, release_id, target_artist_id,
                 target_observation_id, 'primary', 1, '$NOT', '', now()),
                (other_release_artist_row_id, release_id, other_artist_id,
                 other_observation_id, 'primary', 2, '$NOT/Jeht', '', now());
            INSERT INTO track_artists (
                id, track_id, artist_id, observed_artist_id, role,
                presentation_order, display_value, join_phrase, created_at
            ) VALUES (
                track_artist_row_id, track_id, source_artist_id,
                track_observation_id, 'primary', 1, '$NOT', '', now()
            );
            INSERT INTO artist_match_decisions (
                id, observed_artist_id, artist_id, outcome, authority,
                confidence, reason, is_locked, payload, created_at
            ) VALUES
                (target_decision_id, target_observation_id, target_artist_id,
                 'confirmed', 'automated', 6000,
                 'created a distinct provisional identity from manifest context',
                 false, '{}'::jsonb, now()),
                (other_decision_id, other_observation_id, other_artist_id,
                 'confirmed', 'automated', 6000,
                 'created a distinct provisional identity from manifest context',
                 false, '{}'::jsonb, now()),
                (track_decision_id, track_observation_id, source_artist_id,
                 'confirmed', 'automated', 6000,
                 'created a distinct provisional identity from manifest context',
                 false, '{}'::jsonb, now());
            INSERT INTO artist_match_current (
                observed_artist_id, decision_id, updated_at
            ) VALUES
                (target_observation_id, target_decision_id, now()),
                (other_observation_id, other_decision_id, now()),
                (track_observation_id, track_decision_id, now());
            INSERT INTO artist_track_associations (
                artist_id, track_id, association_type, participation_kind,
                source_key, source_artist_id, confidence, is_performance, created_at
            ) VALUES (
                source_artist_id, track_id, 'primary_artist', 'direct',
                track_artist_row_id, NULL, 6000, true, now()
            );
        END $$;
        "#,
    ))
    .execute(&db.pool)
    .await
    .expect("seed pre-0040 multi-Artist identity split");

    migrate(&db.pool).await.expect("forward migration to 0040");

    let visible: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, canonical_name FROM artists
          WHERE status IN ('active','verified') ORDER BY canonical_name, id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read visible Artists");
    assert_eq!(visible.len(), 2, "one repaired source becomes a tombstone");
    let target_id = visible
        .iter()
        .find(|(_, name)| name == "$NOT")
        .map(|(id, _)| *id)
        .expect("visible $NOT target");
    assert!(
        visible.iter().any(|(_, name)| name == "$NOT/Jeht"),
        "nonmatching Release Artist remains distinct"
    );
    assert_eq!(
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT artist_id FROM track_artists")
            .fetch_one(&db.pool)
            .await
            .expect("repaired Track Artist"),
        target_id
    );
    let current: (uuid::Uuid, String, bool) = sqlx::query_as(
        "SELECT decision.artist_id, decision.reason,
                decision.predecessor_decision_id IS NOT NULL
           FROM observed_artists observed
           JOIN artist_match_current current_match
             ON current_match.observed_artist_id=observed.id
           JOIN artist_match_decisions decision ON decision.id=current_match.decision_id
          WHERE observed.scope='track'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read repaired current decision");
    assert_eq!(current.0, target_id);
    assert_eq!(
        current.1,
        "resolved to the uniquely matching Release Artist from manifest context"
    );
    assert!(current.2, "append-only predecessor is retained");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM observed_artists")
            .fetch_one(&db.pool)
            .await
            .expect("observations retained"),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artist_match_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("decision history retained"),
        4
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artist_merge_events
              WHERE actor='catalog.same-release-unique-artist.v1'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("audited merge event"),
        1
    );

    db.drop().await;
}

/// ADR-0177 / migration 0041: only the exact obsolete built-in v1
/// POL-UNKNOWN decision is reconciled to Accept. A custom Unknown review
/// decision remains pending and withheld.
#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn migration_0041_reconciles_only_builtin_unknown_reviews() {
    use melodee_domain::{
        Disposition, IngestionState, PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleSetId,
        PolicyRuleType,
    };
    use melodee_persistence::{PolicyStore, WorkItemStore, WorkerId};
    use serde_json::json;
    use time::OffsetDateTime;

    let db = TestDb::create_unmigrated("migrations_forward_0041_policy_unknown").await;
    let prior: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 40)
        .cloned()
        .collect();
    assert_eq!(prior.len(), 40, "expected migrations 0001..=0040");
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("apply pre-0041 schema");
    allow_current_artist_status_vocabulary(&db.pool).await;

    let store = PolicyStore;
    let now = OffsetDateTime::now_utc();
    let v1_rules = [
        (
            "POL-ADMIN-OVERRIDE",
            PolicyRuleType::AdministratorOverride,
            Disposition::Review,
        ),
        (
            "POL-CORRUPT",
            PolicyRuleType::CorruptMedia,
            Disposition::Quarantine,
        ),
        (
            "POL-GH-UNCERTAIN",
            PolicyRuleType::GreatestHitsUncertain,
            Disposition::Review,
        ),
        (
            "POL-GH-NOVEL",
            PolicyRuleType::GreatestHitsNovelty,
            Disposition::Review,
        ),
        (
            "POL-GH-REDUNDANT",
            PolicyRuleType::GreatestHitsRedundancy,
            Disposition::Ignore,
        ),
        (
            "POL-UNKNOWN",
            PolicyRuleType::UnknownClassification,
            Disposition::Review,
        ),
    ]
    .into_iter()
    .map(|(id, rule_type, disposition)| PolicyRule {
        id: id.to_string(),
        rule_type,
        predicate_json: json!({}),
        disposition,
        confidence_threshold: None,
    })
    .collect();
    let v1_id = PolicyRuleSetId::from_uuid(
        uuid::Uuid::parse_str("01989f00-7000-7000-8000-000500000001")
            .expect("stable v1 rule-set id"),
    );
    let v1 = PolicyRuleSet::new(PolicyRuleSet {
        id: v1_id,
        name: "phase5-default-rules".to_string(),
        version: 1,
        rules: v1_rules,
        created_at: OffsetDateTime::UNIX_EPOCH,
        created_by: PolicyActor::System {
            policy_rule_set_version: 1,
        },
    })
    .expect("historical v1 default rule set");
    store
        .ensure_rule_set(&db.pool, &v1, now)
        .await
        .expect("persist historical v1 defaults");

    let source = TestDb::create("policy_fixture_source").await;
    let builtin_title = "Ordinary Unknown Built-in Review";
    let builtin = historical_catalog::import(&source, &db, 91, builtin_title).await;
    let worker = WorkerId::new();
    let claim = historical_catalog::policy_claim(&db, &builtin, builtin_title, &worker).await;
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load built-in context");
    let mut commit = phase5_common::unknown_review_commit(&context, v1_id, now);
    commit.decision.matched_rule_ids = vec!["POL-UNKNOWN".to_string()];
    let builtin_decision_id = commit.decision.id.into_uuid();
    store
        .commit_evaluation(&db.pool, &worker, &commit)
        .await
        .expect("commit historical built-in review");

    let custom_title = "Explicit Custom Unknown Review";
    let custom = historical_catalog::import(&source, &db, 92, custom_title).await;
    source.drop().await;
    let custom_rules = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &custom_rules, now)
        .await
        .expect("persist custom rule set");
    let custom_worker = WorkerId::new();
    let claim = historical_catalog::policy_claim(&db, &custom, custom_title, &custom_worker).await;
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load custom context");
    store
        .commit_evaluation(
            &db.pool,
            &custom_worker,
            &phase5_common::unknown_review_commit(&context, custom_rules.id, now),
        )
        .await
        .expect("commit custom review");

    assert_eq!(
        WorkItemStore
            .get(&db.pool, builtin.work_item_id)
            .await
            .expect("built-in before upgrade")
            .state,
        IngestionState::NeedsReview
    );
    migrate(&db.pool).await.expect("forward migration to 0041");

    let upgraded: (String, String, serde_json::Value, String, Option<String>) = sqlx::query_as(
        "SELECT release.status, item.state, decision.actor,
                decision.completion_state, decision.override_reason
           FROM policy_decisions decision
           JOIN releases release ON release.id = decision.release_id
           JOIN release_work_items item ON item.id = decision.work_item_id
          WHERE decision.id = $1",
    )
    .bind(builtin_decision_id)
    .fetch_one(&db.pool)
    .await
    .expect("read reconciled decision");
    assert_eq!(upgraded.0, "active");
    assert_eq!(upgraded.1, "indexing");
    assert_eq!(
        upgraded.2,
        json!({
            "authority": "system_upgrade",
            "migration": "0041_accept_ordinary_unknown_releases",
            "policy_rule_set_version": 1
        })
    );
    assert_eq!(upgraded.3, "completed");
    assert_eq!(
        upgraded.4.as_deref(),
        Some("Built-in v1 POL-UNKNOWN review was removed by ADR-0177")
    );
    let audit: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT event, disposition, actor FROM policy_decision_audit
          WHERE id = $1 ORDER BY created_at, event",
    )
    .bind(builtin_decision_id)
    .fetch_all(&db.pool)
    .await
    .expect("read retained decision audit");
    assert_eq!(audit.len(), 2);
    assert!(
        audit
            .iter()
            .any(|row| row.0 == "created" && row.1 == "review")
    );
    assert!(audit.iter().any(|row| {
        row.0 == "completed" && row.1 == "accept" && row.2["authority"] == "system_upgrade"
    }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_work_item_events
              WHERE work_item_id = $1
                AND reason_code = 'policy.default_unknown_v2_reconciled'
                AND from_state = 'needs_review' AND to_state = 'indexing'",
        )
        .bind(builtin.work_item_id)
        .fetch_one(&db.pool)
        .await
        .expect("count reconciliation event"),
        1
    );

    let untouched: (String, String, String, String) = sqlx::query_as(
        "SELECT release.status, item.state, decision.disposition,
                decision.completion_state
           FROM policy_decisions decision
           JOIN releases release ON release.id = decision.release_id
           JOIN release_work_items item ON item.id = decision.work_item_id
          WHERE decision.release_id = $1",
    )
    .bind(custom.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read untouched custom review");
    assert_eq!(
        untouched,
        (
            "deprecated".to_string(),
            "needs_review".to_string(),
            "review".to_string(),
            "pending".to_string()
        )
    );

    db.drop().await;
}

/// ADR-0148 §D.5/D.6 schema contract: the run state machine, uuidv7 primary
/// keys, non-negative checkpoint counters, the entity-map row-hash shape, and
/// the issue severity vocabulary are enforced declaratively.
#[tokio::test]
async fn legacy_migration_schema_enforces_the_run_contract() {
    let db = TestDb::create("migrations_legacy_contract").await;

    // Invalid run state is rejected.
    let result = sqlx::query(
        "INSERT INTO legacy_migration.runs (
            id, state, source_identity, source_snapshot_marker, created_at, updated_at
         ) VALUES (uuidv7(), 'bogus', 'src', 'marker', now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "unknown run state must be rejected");

    // Non-v7 run id is rejected.
    let result = sqlx::query(
        "INSERT INTO legacy_migration.runs (
            id, state, source_identity, source_snapshot_marker, created_at, updated_at
         ) VALUES (gen_random_uuid(), 'planned', 'src', 'marker', now(), now())",
    )
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "non-v7 run id must be rejected");

    let run_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO legacy_migration.runs (
                id, state, source_identity, source_snapshot_marker, created_at, updated_at
             ) VALUES (uuidv7(), 'planned', 'src', 'marker', now(), now())
             RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .expect("valid run");

    // Negative checkpoint counters are rejected.
    let result = sqlx::query(
        "INSERT INTO legacy_migration.checkpoints (
            run_id, domain, last_source_pk, processed, inserted, updated,
            issues, elapsed_ms, updated_at
         ) VALUES ($1, 'artists', -1, 0, 0, 0, 0, 0, now())",
    )
    .bind(run_id)
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "negative checkpoint must be rejected");

    // Malformed row hash is rejected; a real 64-hex hash is accepted.
    let result = sqlx::query(
        "INSERT INTO legacy_migration.entity_map (
            run_id, domain, source_pk, target_id, created_at, row_hash
         ) VALUES ($1, 'users', 1, uuidv7(), now(), 'not-hex')",
    )
    .bind(run_id)
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "malformed row_hash must be rejected");
    sqlx::query(
        "INSERT INTO legacy_migration.entity_map (
            run_id, domain, source_pk, target_id, created_at, row_hash
         ) VALUES ($1, 'users', 1, uuidv7(), now(),
                   'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')",
    )
    .bind(run_id)
    .execute(&db.pool)
    .await
    .expect("valid mapping row");

    // Invalid issue severity is rejected.
    let result = sqlx::query(
        "INSERT INTO legacy_migration.issues (
            id, run_id, domain, severity, code, message, created_at
         ) VALUES (uuidv7(), $1, 'users', 'fatal', 'x', 'y', now())",
    )
    .bind(run_id)
    .execute(&db.pool)
    .await;
    assert!(result.is_err(), "unknown issue severity must be rejected");

    db.drop().await;
}

#[tokio::test]
#[ignore = "slow historical-schema compatibility; run with `make test-migrations` (nightly gate)"]
async fn all_six_legacy_file_owners_survive_an_interrupted_upgrade() {
    use melodee_persistence::asset_files;
    let db = TestDb::create_unmigrated("external_assets").await;
    MIGRATOR
        .run_to(53, &db.pool)
        .await
        .expect("original schema");
    sqlx::raw_sql(sqlx::AssertSqlSafe(r#"
        DO $$
        DECLARE u uuid := uuidv7(); a uuid := uuidv7(); body bytea := convert_to('stored on disk','UTF8');
        BEGIN
          INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,
            password_hash,password_hash_algorithm,stream_signing_key,image_mime_type,image_bytes,created_at,updated_at)
            VALUES(u,uuidv7(),'test','TEST','t@test.invalid','T@TEST.INVALID','test','bcrypt','test','image/png',body,now(),now());
          INSERT INTO artists(id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at)
            VALUES(a,uuidv7(),'Artist','artist','artist','person','verified',now(),now());
          INSERT INTO native_user_avatars VALUES(u,'image/png',body,now());
          INSERT INTO native_playlists(id,public_api_id,owner_user_id,name,image_mime_type,image_bytes,created_at,updated_at)
            VALUES(uuidv7(),uuidv7(),u,'list','image/png',body,now(),now());
          INSERT INTO native_theme_assets VALUES('light','theme.css','text/css',body);
          INSERT INTO catalog_entity_images(entity_type,artist_id,image_mime_type,image_bytes,uploaded_by,created_at,updated_at)
            VALUES('artist',a,'image/png',body,'test',now(),now());
          INSERT INTO catalog_provider_images(id,entity_type,entity_id,provider,source_url,image_mime_type,image_bytes,confidence,observed_at)
            VALUES(uuidv7(),'artist',a,'deezer','https://example.invalid/cover','image/png',body,9000,now());
        END $$;
    "#)).execute(&db.pool).await.expect("all six original owner types");
    // An SQL-only upgrade without a verified receipt must roll back the table rewrites.
    assert!(MIGRATOR.run(&db.pool).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT image FROM native_user_avatars")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        b"stored on disk"
    );
    migrate(&db.pool).await.expect("resume and verify files");
    migrate(&db.pool).await.expect("repeat");
    let keys: Vec<String> = sqlx::query_scalar("SELECT asset_key FROM native_users UNION ALL SELECT asset_key FROM native_user_avatars UNION ALL SELECT asset_key FROM native_playlists UNION ALL SELECT asset_key FROM native_theme_assets UNION ALL SELECT asset_key FROM catalog_entity_images UNION ALL SELECT asset_key FROM catalog_provider_images").fetch_all(&db.pool).await.unwrap();
    assert_eq!(keys.len(), 6);
    for key in &keys {
        assert_eq!(
            asset_files::read(key.clone()).await.unwrap(),
            b"stored on disk"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM asset_files")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    let columns: Vec<String> = sqlx::query_scalar("SELECT table_name||'.'||column_name FROM information_schema.columns WHERE table_schema='public' AND udt_name IN ('bytea','_bytea','oid','lo') ORDER BY 1").fetch_all(&db.pool).await.unwrap();
    assert_eq!(
        columns,
        [
            "_sqlx_migrations.checksum",
            "jellyfin_access_tokens.token_digest",
            "jellyfin_access_tokens.token_salt",
            "native_lastfm_links.session_key_ciphertext",
            "native_lastfm_links.session_key_digest",
            "native_password_reset_tokens.token_digest",
            "native_rate_limit_counters.actor_key_digest",
            "native_refresh_tokens.token_digest"
        ]
    );
    db.drop().await;
}

fn uuidv7() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}

/// PERF-009 / RM-20 populated-upgrade rehearsal.
///
/// Upgrades a populated pre-remediation database (schema through 0059) with
/// representative data and deliberately inconsistent derived state, applying the
/// full chain (0060-0064), and verifies existing rows survive, the new indexes
/// are valid, and the operator repair converges the corrupted aggregate.
#[tokio::test]
async fn perf_remediation_upgrade_from_0059_preserves_data_and_repairs_derived_state() {
    use melodee_persistence::MIGRATOR;
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("migrations_forward_perf").await;
    let prior: Vec<sqlx::migrate::Migration> = MIGRATOR
        .migrations
        .iter()
        .filter(|migration| migration.version <= 59)
        .cloned()
        .collect();
    assert_eq!(prior.len(), 59, "expected migrations 0001..=0059");
    Migrator {
        migrations: Cow::Owned(prior),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("apply pre-remediation schema");

    // Seed with triggers/FKs bypassed: a minimal but valid catalog plus a
    // deliberately wrong genre aggregate and a pending outbox row.
    let mut tx = db.pool.begin().await.expect("begin seed transaction");
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .expect("bypass triggers for fixture seed");
    let release = uuidv7();
    let audio = uuidv7();
    let media = uuidv7();
    let track = uuidv7();
    sqlx::query(
        "INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,
                status, revision, created_at, updated_at)
         VALUES ($1, uuidv7(), 'Upgrade Release', 'UPGRADE RELEASE', 'Upgrade Release',
                'active', 1, now(), now())",
    )
    .bind(release)
    .execute(&mut *tx)
    .await
    .expect("seed release");
    sqlx::query(
        "INSERT INTO audio_identities (id, status, revision, created_at, updated_at)
         VALUES ($1, 'verified', 1, now(), now())",
    )
    .bind(audio)
    .execute(&mut *tx)
    .await
    .expect("seed audio identity");
    sqlx::query(
        "INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size,
                mime_type, created_at)
         VALUES ($1, 'upgrade/track.flac', repeat('a', 64), 1000, 'audio/flac', now())",
    )
    .bind(media)
    .execute(&mut *tx)
    .await
    .expect("seed media asset");
    sqlx::query(
        "INSERT INTO tracks (id, public_api_id, release_id, audio_identity_id,
                playable_media_asset_id, ordinal, title, normalized_title, sort_title,
                created_at, updated_at)
         VALUES ($1, uuidv7(), $2, $3, $4, 1, 'Upgrade Track', 'UPGRADE TRACK',
                'Upgrade Track', now(), now())",
    )
    .bind(track)
    .bind(release)
    .bind(audio)
    .bind(media)
    .execute(&mut *tx)
    .await
    .expect("seed track");
    sqlx::query(
        "INSERT INTO catalog_genre_terms (track_id, release_id, surface, display_value, source_order)
         VALUES ($1, $2, 'native', 'Rock', 0)",
    )
    .bind(track)
    .bind(release)
    .execute(&mut *tx)
    .await
    .expect("seed genre term");
    sqlx::query(
        "INSERT INTO catalog_genre_aggregates (surface, display_value, song_count, release_count)
         VALUES ('native', 'Rock', 9, 4)",
    )
    .execute(&mut *tx)
    .await
    .expect("seed corrupted aggregate");
    let outbox_id: i64 = sqlx::query_scalar(
        "INSERT INTO catalog_projection_outbox (projection_name, entity_type, entity_id,
                operation, entity_revision, available_at, created_at)
         VALUES ('tantivy.catalog', 'release', $1, 'upsert', 1, now(), now())
         RETURNING id",
    )
    .bind(release)
    .fetch_one(&mut *tx)
    .await
    .expect("seed pending outbox row");
    sqlx::query("SET LOCAL session_replication_role = origin")
        .execute(&mut *tx)
        .await
        .expect("restore triggers");
    tx.commit().await.expect("commit seed transaction");

    migrate(&db.pool)
        .await
        .expect("apply remediation migrations");

    // Existing authoritative rows survive verbatim.
    let preserved: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM releases WHERE id = $1),
                (SELECT count(*) FROM tracks WHERE id = $2),
                (SELECT count(*) FROM catalog_projection_outbox WHERE id = $3)",
    )
    .bind(release)
    .bind(track)
    .bind(outbox_id)
    .fetch_one(&db.pool)
    .await
    .expect("check preserved rows");
    assert_eq!(preserved, (1, 1, 1));

    // Every new index exists and is valid.
    let invalid: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_index index
         JOIN pg_class relation ON relation.oid = index.indexrelid
         WHERE relation.relname IN (
             'tracks_playable_media_asset_idx',
             'catalog_genre_terms_release_surface_idx',
             'artist_match_decisions_artist_observed_idx',
             'native_artist_user_states_positive_rating_idx',
             'catalog_identity_reconciliation_seed_created_idx',
             'search_rebuild_snapshot_outbox_pkey',
             'search_rebuild_applied_outbox_pkey'
         ) AND NOT index.indisvalid",
    )
    .fetch_one(&db.pool)
    .await
    .expect("validate new indexes");
    assert_eq!(invalid, 0, "all remediation indexes must be valid");

    // New state tables exist.
    for table in [
        "catalog_identity_reconciliation_seed",
        "search_rebuild_snapshot_outbox",
        "search_rebuild_applied_outbox",
        "native_discovery_progress",
    ] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table}"))
            .fetch_one(&db.pool)
            .await
            .expect("table existence");
        assert!(exists, "{table} must exist after upgrade");
    }

    // The corrupted aggregate is detectable and the repair converges it.
    let (checked, repaired): (i32, i32) =
        sqlx::query_as("SELECT * FROM repair_catalog_genre_aggregates(true, 1_000)")
            .fetch_one(&db.pool)
            .await
            .expect("apply genre repair on upgraded data");
    assert!(checked >= 1, "the corrupted aggregate is detected");
    assert_eq!(checked, repaired);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT song_count, release_count FROM catalog_genre_aggregates
          WHERE surface = 'native' AND display_value = 'Rock'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read repaired aggregate");
    assert_eq!(counts, (1, 1));

    // The discovery progress table is seeded with one cursor per work class.
    let classes: i64 = sqlx::query_scalar("SELECT count(*) FROM native_discovery_progress")
        .fetch_one(&db.pool)
        .await
        .expect("count discovery classes");
    assert_eq!(classes, 5);

    db.drop().await;
}
