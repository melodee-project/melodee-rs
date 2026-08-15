//! Migration integration tests: from-empty and repeatability (work-package
//! parts XIV, XXXII).

// Integration tests: expect/panic on setup or assertion failure IS the
// failure-reporting mechanism; production-code lint stays strict.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_persistence::migrate;

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
/// still return correct, complete results (merged/deprecated rows included)
/// once the indexes are present.
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

    // Store-level correctness: the unfiltered admin listing must include
    // non-active rows (the reason the partial indexes could not serve it) and
    // order by sort_name with the id tiebreak.
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
        seed("Gamma", melodee_domain::ArtistStatus::Provisional),
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
    assert_eq!(total, 3, "deprecated/provisional rows must be listed");
    let names: Vec<&str> = rows.iter().map(|row| row.canonical_name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta", "Gamma"]);

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
    let columns = "entity_type, artist_id, release_id, image_mime_type, image_bytes, \
                   uploaded_by, created_at, updated_at";
    let dangling = uuid::Uuid::now_v7();
    let cases: [(&str, String); 8] = [
        (
            "unknown entity_type",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'track', '{artist_id}', NULL, 'image/png', '\\x89504e47'::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "entity_type/typed-column mismatch",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', NULL, '{dangling}', 'image/png', '\\x89504e47'::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "no typed column set",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', NULL, NULL, 'image/png', '\\x89504e47'::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "unsupported mime type",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/gif', '\\x47494638'::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "empty bytes",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/png', ''::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "bytes beyond the 10 MiB bound",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{artist_id}', NULL, 'image/png',
                 decode(repeat('00', 10485761), 'hex'), 'admin', now(), now())"
            ),
        ),
        (
            "dangling artist FK",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'artist', '{dangling}', NULL, 'image/png', '\\x89504e47'::bytea,
                 'admin', now(), now())"
            ),
        ),
        (
            "dangling release FK",
            format!(
                "INSERT INTO catalog_entity_images ({columns}) VALUES (
                 'release', NULL, '{dangling}', 'image/png', '\\x89504e47'::bytea,
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
            entity_type, artist_id, image_mime_type, image_bytes,
            uploaded_by, created_at, updated_at
         ) VALUES ('artist', $1, 'image/png', '\\x89504e47'::bytea, 'admin', now(), now())",
    )
    .bind(artist_id)
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
