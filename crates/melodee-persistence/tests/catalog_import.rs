//! Cross-slice PostgreSQL acceptance tests for the v3 catalog handoff.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{
    CatalogFixturePresentation, seed_published_release,
    seed_published_release_with_musicbrainz_ids, seed_published_release_with_release_artists,
};
use common::{TestDb, test_config};
use melodee_domain::{
    FailureClass, IngestionState, PUBLICATION_MANIFEST_VERSION, RetryPolicy, WorkErrorCode,
};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkItemStore, WorkerId};
use time::Duration;

#[tokio::test]
async fn slash_billing_keeps_release_artist_primary_and_creates_featured_track_artists() {
    let db = TestDb::create("catalog_slash_track_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        92,
        CatalogFixturePresentation {
            release_title: "Digital Dancer",
            release_artist: "6arelyhuman",
            track_title: "Abyss",
            track_artist: "6arelyhuman/Baby Jane",
            release_date: "2024-05-03",
        },
        &["6arelyhuman"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim slash billing import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import slash billing");
    assert_eq!(imported.artist_count, 2);

    let release_artists: Vec<String> = sqlx::query_scalar(
        "SELECT artist.canonical_name
           FROM release_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
          WHERE billing.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read Release Artists");
    assert_eq!(release_artists, ["6arelyhuman"]);

    let track_artists: Vec<(String, String, String, i32)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase,
                billing.presentation_order
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read structured Track Artists");
    assert_eq!(
        track_artists,
        vec![
            (
                "6arelyhuman".to_owned(),
                "primary".to_owned(),
                "/".to_owned(),
                1
            ),
            (
                "Baby Jane".to_owned(),
                "featured".to_owned(),
                String::new(),
                2
            ),
        ]
    );
    let display_artist: String =
        sqlx::query_scalar("SELECT display_artist FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read exact Track DisplayArtist");
    assert_eq!(display_artist, "6arelyhuman/Baby Jane");
    let associations: Vec<(String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, association.association_type
           FROM artist_track_associations association
           JOIN artists artist ON artist.id=association.artist_id
           JOIN tracks track ON track.id=association.track_id
          WHERE track.release_id=$1
          ORDER BY artist.canonical_name, association.association_type",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read Artist participation");
    assert!(
        associations
            .iter()
            .any(|row| row == &("Baby Jane".to_owned(), "featured_artist".to_owned()))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists WHERE canonical_name='6arelyhuman/Baby Jane'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count compound identities"),
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn slash_billing_supports_multiple_featured_track_artists() {
    let db = TestDb::create("catalog_multi_feature_slash_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        93,
        CatalogFixturePresentation {
            release_title: "Woounded, Vol. 2",
            release_artist: "9000 Rondae",
            track_title: "Face Hot",
            track_artist: "9000 Rondae/Glockboyz Teejaee/OnFully/Roadrunner Glockboyz Tez",
            release_date: "2023-01-01",
        },
        &["9000 Rondae"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim multiple-feature import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import multiple-feature billing");
    assert_eq!(imported.artist_count, 4);

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read multiple structured Track Artists");
    assert_eq!(
        rows,
        vec![
            (
                "9000 Rondae".to_owned(),
                "primary".to_owned(),
                "/".to_owned()
            ),
            (
                "Glockboyz Teejaee".to_owned(),
                "featured".to_owned(),
                "/".to_owned()
            ),
            ("OnFully".to_owned(), "featured".to_owned(), "/".to_owned()),
            (
                "Roadrunner Glockboyz Tez".to_owned(),
                "featured".to_owned(),
                String::new()
            ),
        ]
    );
    db.drop().await;
}

#[tokio::test]
async fn explicit_track_billing_phrases_create_featured_associations() {
    let db = TestDb::create("catalog_explicit_track_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        94,
        CatalogFixturePresentation {
            release_title: "Explicit Billing",
            release_artist: "Somebody",
            track_title: "Together",
            track_artist: "Somebody Featuring Colin Hay with Guest",
            release_date: "2026-01-01",
        },
        &["Somebody"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim explicit billing import");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import explicit billing");

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read explicit structured Track Artists");
    assert_eq!(
        rows,
        vec![
            (
                "Somebody".to_owned(),
                "primary".to_owned(),
                " Featuring ".to_owned()
            ),
            (
                "Colin Hay".to_owned(),
                "featured".to_owned(),
                " with ".to_owned()
            ),
            ("Guest".to_owned(), "featured".to_owned(), String::new()),
        ]
    );
    db.drop().await;
}

#[tokio::test]
async fn exact_release_artist_with_a_slash_remains_a_literal_identity() {
    let db = TestDb::create("catalog_literal_slash_artist").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        95,
        CatalogFixturePresentation {
            release_title: "Literal Slash",
            release_artist: "$NOT/Jeht",
            track_title: "No Split",
            track_artist: "$NOT/Jeht",
            release_date: "2026-01-01",
        },
        &["$NOT/Jeht"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim literal slash import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import literal slash Artist");
    assert_eq!(imported.artist_count, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM track_artists billing
              JOIN tracks track ON track.id=billing.track_id
             WHERE track.release_id=$1",
        )
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("count literal Track Artist rows"),
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn multi_artist_release_reuses_the_one_exact_track_artist_match() {
    let db = TestDb::create("catalog_multi_artist_exact_match").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        90,
        CatalogFixturePresentation {
            release_title: "High Risk",
            release_artist: "$NOT",
            track_title: "Locked In",
            track_artist: "$NOT",
            release_date: "2026-01-02",
        },
        &["$NOT", "$NOT/Jeht"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim exact-match import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-unique-match-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import multi-Artist Release");
    assert_eq!(
        imported.artist_count, 2,
        "the matching Track must not allocate a third Artist identity"
    );

    let rows: Vec<(String, String, uuid::Uuid)> = sqlx::query_as(
        "SELECT observed.scope, observed.normalized_value, decision.artist_id
           FROM observed_artists observed
           JOIN artist_match_current current ON current.observed_artist_id=observed.id
           JOIN artist_match_decisions decision ON decision.id=current.decision_id
          WHERE observed.release_id=$1
          ORDER BY observed.scope, observed.normalized_value, observed.id",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read multi-Artist decisions");
    assert_eq!(rows.len(), 3, "all raw observations remain distinct");
    let release_not = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "release" && normalized == "$not")
        .expect("release $NOT observation");
    let release_collaboration = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "release" && normalized == "$not/jeht")
        .expect("collaboration Release observation");
    let track_not = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "track" && normalized == "$not")
        .expect("track $NOT observation");
    assert_eq!(track_not.2, release_not.2);
    assert_ne!(track_not.2, release_collaboration.2);
    db.drop().await;
}

#[tokio::test]
async fn multi_artist_release_keeps_equal_normalized_release_matches_ambiguous() {
    let db = TestDb::create("catalog_multi_artist_ambiguous_match").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        89,
        CatalogFixturePresentation {
            release_title: "Ambiguous Billing",
            release_artist: "Same Name",
            track_title: "Do Not Conflate",
            track_artist: "Same Name",
            release_date: "2026-01-01",
        },
        &["Same Name", " SAME NAME "],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim ambiguous import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-unique-match-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import ambiguous multi-Artist Release");
    assert_eq!(
        imported.artist_count, 3,
        "two equal Release observations are not a unique identity match"
    );
    db.drop().await;
}

#[tokio::test]
async fn catalog_import_persists_all_embedded_musicbrainz_families_and_enrichment_work() {
    let db = TestDb::create("catalog_embedded_evidence").await;
    let fixture = seed_published_release_with_musicbrainz_ids(
        &db,
        91,
        CatalogFixturePresentation {
            release_title: "Embedded Evidence",
            release_artist: "Evidence Artist",
            track_title: "Evidence Track",
            track_artist: "Evidence Artist",
            release_date: "2009-08-07",
        },
        serde_json::json!({
            "recording": "aa1b1c1d-0000-7000-8000-000000000001",
            "release": "aa1b1c1d-0000-7000-8000-000000000002",
            "release_group": "aa1b1c1d-0000-7000-8000-000000000003",
            "artist": "aa1b1c1d-0000-7000-8000-000000000004",
            "release_artist": "aa1b1c1d-0000-7000-8000-000000000005",
            "work": "not-a-uuid-but-retained"
        }),
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim catalog import");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("embedded-evidence-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import embedded evidence");

    let external_ids: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT provider, external_id, confidence
         FROM artist_external_identities ORDER BY external_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read Artist external evidence");
    assert_eq!(external_ids.len(), 2);
    assert!(
        external_ids
            .iter()
            .all(|(_, id, confidence)| { id.starts_with("aa1b1c1d-") && *confidence == 9500 })
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_external_identities
             WHERE release_id=$1 AND provider='musicbrainz'",
        )
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Release external evidence"),
        2
    );
    let audio_evidence: Vec<(String, i32)> = sqlx::query_as(
        "SELECT payload->>'family', confidence FROM audio_identity_evidence
         WHERE track_id=(SELECT id FROM tracks WHERE release_id=$1)
         ORDER BY payload->>'family'",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read AudioIdentity evidence");
    assert_eq!(audio_evidence.len(), 6);
    assert!(
        audio_evidence
            .iter()
            .any(|(family, confidence)| family == "work" && *confidence == 1000)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audio_identity_match_decisions
             WHERE outcome='proposed'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read AudioIdentity proposals"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_tasks")
            .fetch_one(&db.pool)
            .await
            .expect("read scheduled enrichment"),
        36
    );

    let genre_terms: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT surface, display_value, match_value, source_order
           FROM catalog_genre_terms ORDER BY surface",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read projected genre terms");
    assert_eq!(
        genre_terms,
        vec![
            (
                "native".to_string(),
                "Fixture".to_string(),
                "Fixture".to_string(),
                1,
            ),
            (
                "opensubsonic".to_string(),
                "Fixture".to_string(),
                "Fixture".to_string(),
                100_001,
            ),
        ]
    );
    let genre_aggregates: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT surface, song_count, release_count
           FROM catalog_genre_aggregates ORDER BY surface",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read projected genre aggregates");
    assert_eq!(
        genre_aggregates,
        vec![
            ("native".to_string(), 1, 1),
            ("opensubsonic".to_string(), 1, 1),
        ]
    );

    sqlx::query("DELETE FROM catalog_genre_terms WHERE surface='native'")
        .execute(&db.pool)
        .await
        .expect("delete projected native term");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_genre_aggregates WHERE surface='native'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read deleted aggregate"),
        0,
        "statement-level delete maintenance removes empty aggregate keys"
    );
    db.drop().await;
}

#[tokio::test]
async fn published_v3_fixture_exposes_complete_catalog_handoff_without_media_files() {
    let db = TestDb::create("catalog_v3_handoff").await;
    let fixture = seed_published_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Catalog Handoff",
            release_artist: "Fixture Artist",
            track_title: "No Media Reread",
            track_artist: "Fixture Artist",
            release_date: "2001-02-03",
        },
    )
    .await;

    let store = WorkItemStore;
    let item = store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read published item");
    let stored_manifest = store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable v3 manifest")
        .expect("manifest exists");
    let publication = store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable publication")
        .expect("publication exists");

    assert_eq!(item.state, IngestionState::Published);
    assert_eq!(
        stored_manifest.schema_version,
        i32::from(PUBLICATION_MANIFEST_VERSION)
    );
    let decoded_manifest: melodee_domain::ReleaseManifest =
        serde_json::from_value(stored_manifest.payload).expect("decode durable v3 manifest");
    assert_eq!(decoded_manifest, fixture.manifest);
    assert_eq!(publication.release_id, fixture.release_id);
    assert_eq!(publication.plan_hash, fixture.plan_hash);
    assert_eq!(
        publication.sealed_manifest_hash.as_deref(),
        Some(fixture.sealed_manifest_hash.as_str())
    );
    assert!(
        !std::path::Path::new(&fixture.source_path).exists(),
        "fixture deliberately provides no readable inbound media"
    );
    assert!(
        !std::path::Path::new(&fixture.canonical_asset_path).exists(),
        "fixture deliberately provides no readable canonical media"
    );

    db.drop().await;
}

#[tokio::test]
async fn import_adopts_reserved_release_id_is_idempotent_and_never_rewrites_v3() {
    let db = TestDb::create("catalog_import_exact_once").await;
    let fixture = seed_published_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Reserved Identity",
            release_artist: "Same Raw Name",
            track_title: "Manifest Only",
            track_artist: "Same Raw Name",
            release_date: "2002-03-04",
        },
    )
    .await;
    let work_store = WorkItemStore;
    let before_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import manifest")
        .expect("pre-import manifest exists");
    let before_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import publication")
        .expect("pre-import publication exists");

    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), catalog_now)
        .await
        .expect("claim catalog import");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, fixture.work_item_id);
    assert_eq!(claimed[0].state, IngestionState::Cataloging);

    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("import manifest v3");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    assert_eq!(imported.track_count, 1);
    assert_eq!(imported.media_asset_count, 1);
    assert_eq!(
        imported.artist_count, 1,
        "the sole matching Track observation reuses the Release Artist identity"
    );
    assert!(imported.set_based_mutation_statement_count > 0);

    let cataloged = work_store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read cataloged item");
    assert_eq!(cataloged.state, IngestionState::Cataloged);
    assert!(cataloged.lease_owner.is_none());
    let counts_after_first = catalog.counts(&db.pool).await.expect("catalog counts");
    assert_eq!(counts_after_first.releases, 1);
    assert_eq!(counts_after_first.tracks, 1);
    assert_eq!(counts_after_first.artists, 1);
    let identity_rows: Vec<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT decision.artist_id, observed.id, observed.scope
           FROM observed_artists observed
           JOIN artist_match_current current ON current.observed_artist_id = observed.id
           JOIN artist_match_decisions decision ON decision.id = current.decision_id
          ORDER BY observed.scope",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read resolved Artist observations");
    assert_eq!(identity_rows.len(), 2, "raw observations remain distinct");
    assert_eq!(identity_rows[0].0, identity_rows[1].0);
    assert_ne!(identity_rows[0].1, identity_rows[1].1);
    assert_eq!(counts_after_first.media_assets, 1);
    assert_eq!(counts_after_first.imports, 1);

    let stored_release: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT id, public_api_id FROM releases WHERE id = $1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read imported Release");
    assert_eq!(stored_release.0, fixture.release_id.into_uuid());
    assert_eq!(
        stored_release.1,
        fixture.release_id.into_uuid(),
        "new public ID starts equal but remains a separate namespace"
    );
    let stored_asset: (String, String, i64) =
        sqlx::query_as("SELECT canonical_relative_path, byte_hash, byte_size FROM media_assets")
            .fetch_one(&db.pool)
            .await
            .expect("read imported canonical asset");
    assert_eq!(
        stored_asset.0,
        format!(
            "{}/{}",
            fixture.plan.target_relative_path, fixture.canonical_asset_path
        )
    );
    assert_eq!(stored_asset.1, fixture.normalized_byte_hash);
    assert_eq!(stored_asset.2, 42);

    let retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("identical uncertain-commit retry");
    assert!(!retry.inserted);
    assert_eq!(retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        counts_after_first
    );

    let conflicting_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase4.different-resolver.v2", "catalog-test"),
            catalog_now,
        )
        .await;
    assert!(conflicting_retry.is_err(), "different binding fails closed");
    assert_eq!(
        catalog.counts(&db.pool).await.expect("conflict counts"),
        counts_after_first
    );

    let after_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import manifest")
        .expect("post-import manifest exists");
    let after_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import publication")
        .expect("post-import publication exists");
    assert_eq!(after_manifest.manifest_hash, before_manifest.manifest_hash);
    assert_eq!(after_manifest.payload, before_manifest.payload);
    assert_eq!(after_publication, before_publication);
    assert!(
        sqlx::query(
            "UPDATE release_work_item_manifests
             SET manifest_hash = $2 WHERE work_item_id = $1",
        )
        .bind(fixture.work_item_id)
        .bind("f".repeat(64))
        .execute(&db.pool)
        .await
        .is_err(),
        "sealed v3 is immutable at the database boundary"
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_lease_expiry_fences_stale_worker_and_direct_reclaim_converges() {
    let db = TestDb::create("catalog_import_lease_fencing").await;
    let fixture = seed_published_release(&db, 3, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let config = test_config();
    let stale_worker = WorkerId::new();
    let winning_worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();

    let first_claim = catalog
        .claim_cataloging(&db.pool, &stale_worker, &config, catalog_now)
        .await
        .expect("initial catalog claim");
    assert_eq!(first_claim.len(), 1);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &winning_worker, &config, catalog_now)
            .await
            .expect("blocked concurrent claim")
            .is_empty(),
        "active lease admits one authoritative worker"
    );

    let after_expiry = catalog_now + config.lease_duration + Duration::seconds(1);
    let reclaimed = catalog
        .claim_cataloging(&db.pool, &winning_worker, &config, after_expiry)
        .await
        .expect("directly reclaim abandoned Cataloging");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, fixture.work_item_id);
    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");

    let stale_result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &stale_worker,
            &request,
            after_expiry,
        )
        .await;
    assert!(
        stale_result.is_err(),
        "stale lease is fenced before catalog writes"
    );
    let empty_counts = catalog.counts(&db.pool).await.expect("post-fence counts");
    assert_eq!(empty_counts.releases, 0);
    assert_eq!(empty_counts.tracks, 0);
    assert_eq!(empty_counts.imports, 0);

    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("winning retry catalogs atomically");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    let final_counts = catalog.counts(&db.pool).await.expect("final counts");
    assert_eq!(final_counts.releases, 1);
    assert_eq!(final_counts.tracks, 1);
    assert_eq!(final_counts.imports, 1);
    let identical_restart_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("completed restart retry finds authoritative result");
    assert!(!identical_restart_retry.inserted);
    assert_eq!(identical_restart_retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        final_counts
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_renewal_cancellation_release_and_retry_are_fenced_and_reclaimable() {
    let db = TestDb::create("catalog_lease_renew_cancel").await;
    let fixture = seed_published_release(&db, 4, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let owner = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let mut config = test_config();
    config.lease_duration = Duration::seconds(2);
    let claimed = catalog
        .claim_cataloging(&db.pool, &owner, &config, now)
        .await
        .expect("claim short catalog lease");
    assert_eq!(claimed.len(), 1);
    let original_expiry = claimed[0].lease_expires_at.expect("initial lease expiry");

    let renewed = catalog
        .renew_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            Duration::seconds(10),
            now + Duration::seconds(1),
        )
        .await
        .expect("renew active catalog lease");
    assert_eq!(renewed.state, IngestionState::Cataloging);
    assert_eq!(renewed.lease_owner.as_deref(), Some(owner.as_str()));
    assert!(renewed.lease_expires_at.expect("renewed expiry") > original_expiry);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
            .await
            .expect("renewal prevents theft")
            .is_empty()
    );

    catalog
        .release_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            now + Duration::seconds(3),
        )
        .await
        .expect("cancellation releases catalog lease");
    let released = WorkItemStore
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read released Cataloging work");
    assert_eq!(released.state, IngestionState::Cataloging);
    assert!(released.lease_owner.is_none());
    assert!(released.lease_expires_at.is_none());
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial rows")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
        .await
        .expect("reclaim cancellation-released work");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.cancel-retry.v1", "catalog-test");
    let result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            now + Duration::seconds(3),
        )
        .await
        .expect("retry after cancellation catalogs atomically");
    assert!(result.inserted);
    assert_eq!(result.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn retryable_catalog_failure_records_backoff_then_direct_reclaim_converges() {
    let db = TestDb::create("catalog_failure_backoff").await;
    let fixture = seed_published_release(&db, 5, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let first_worker = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let config = test_config();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &first_worker, &config, now)
            .await
            .expect("claim catalog work")
            .len(),
        1
    );
    let policy = RetryPolicy {
        base_delay: Duration::seconds(5),
        max_delay: Duration::seconds(5),
        max_attempts: 3,
    };
    let failed = catalog
        .record_failure(
            &db.pool,
            fixture.work_item_id,
            &first_worker,
            WorkErrorCode::new("catalog.synthetic_restart"),
            "simulated process termination before import",
            FailureClass::Retryable,
            &policy,
            now,
        )
        .await
        .expect("record retryable catalog failure");
    assert_eq!(failed.state, IngestionState::Cataloging);
    assert_eq!(failed.attempt_count, 1);
    assert!(failed.lease_owner.is_none());
    let retry_at = failed.next_attempt_at.expect("durable retry deadline");
    assert!(
        catalog
            .claim_cataloging(
                &db.pool,
                &retry_worker,
                &config,
                retry_at - Duration::milliseconds(1),
            )
            .await
            .expect("backoff blocks early reclaim")
            .is_empty()
    );
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial import")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, retry_at)
        .await
        .expect("direct reclaim at retry deadline");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.failure-retry.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("retry catalogs atomically");
    assert!(imported.inserted);
    let identical = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("identical completed retry");
    assert!(!identical.inserted);
    assert_eq!(identical.catalog_import_id, imported.catalog_import_id);
    let event_counts: (i64, i64) = sqlx::query_as(
        "SELECT
            count(*) FILTER (WHERE event_type='failed'),
            count(*) FILTER (WHERE event_type='retry_scheduled')
         FROM release_work_item_events WHERE work_item_id=$1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("query durable retry evidence");
    assert_eq!(event_counts, (1, 1));
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn trusted_release_evidence_merge_and_split_preserve_tracks_and_audit() {
    let db = TestDb::create("catalog_release_merge_split").await;
    let evidence = serde_json::json!({
        "recording": "bb1b1c1d-0000-7000-8000-000000000001",
        "release": "bb1b1c1d-0000-7000-8000-000000000002",
        "release_group": "bb1b1c1d-0000-7000-8000-000000000003",
        "artist": "bb1b1c1d-0000-7000-8000-000000000004",
        "release_artist": "bb1b1c1d-0000-7000-8000-000000000005"
    });
    let first = seed_published_release_with_musicbrainz_ids(
        &db,
        20,
        CatalogFixturePresentation {
            release_title: "Merge Source",
            ..CatalogFixturePresentation::default()
        },
        evidence.clone(),
    )
    .await;
    let second = seed_published_release_with_musicbrainz_ids(
        &db,
        21,
        CatalogFixturePresentation {
            release_title: "Merge Target",
            ..CatalogFixturePresentation::default()
        },
        evidence,
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let claimed = catalog
        .claim_cataloging(
            &db.pool,
            &worker,
            &test_config(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("claim Release fixtures");
    assert_eq!(claimed.len(), 2);
    for fixture in [&first, &second] {
        let now = time::OffsetDateTime::now_utc();
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("release-merge-test-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import Release fixture");
    }

    let candidates = catalog
        .strong_release_merge_candidates(&db.pool, 64)
        .await
        .expect("scan trusted Release evidence");
    let candidate = candidates
        .into_iter()
        .find(|candidate| {
            candidate.source_release_id == first.release_id.into_uuid()
                && candidate.target_release_id == second.release_id.into_uuid()
        })
        .expect("duplicate Releases form a strong evidence candidate");
    let source_track: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read source Track before merge");
    let target_track_count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(second.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks before merge");

    let merge_event = catalog
        .merge_release(
            &db.pool,
            candidate.source_release_id,
            candidate.target_release_id,
            "catalog-test",
            "strong provider evidence test merge",
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("merge Release with durable event");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read merged source status"),
        "merged"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(second.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks after merge"),
        target_track_count_before + 1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM release_merge_events WHERE id=$1")
            .bind(merge_event)
            .fetch_one(&db.pool)
            .await
            .expect("read merge event"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_projection_outbox
             WHERE entity_type='track' AND entity_id=$1 AND entity_revision > 1
               AND processed_at IS NULL",
        )
        .bind(source_track)
        .fetch_one(&db.pool)
        .await
        .expect("read Track projection event after merge"),
        1
    );

    catalog
        .split_release_merge(
            &db.pool,
            merge_event,
            "catalog-test",
            "reversible merge test split",
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read restored source status"),
        "active"
    );
    assert_eq!(
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT release_id FROM tracks WHERE id=$1")
            .bind(source_track)
            .fetch_one(&db.pool)
            .await
            .expect("read restored Track owner"),
        first.release_id.into_uuid()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_split_events WHERE merge_event_id=$1",
        )
        .bind(merge_event)
        .fetch_one(&db.pool)
        .await
        .expect("read split event"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_audit_events
             WHERE entity_id = ANY($1) AND event_type IN ('release_merged','release_merged_into','release_split')",
        )
        .bind(vec![first.release_id.into_uuid(), second.release_id.into_uuid()])
        .fetch_one(&db.pool)
        .await
        .expect("read merge and split audit events"),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT track_count FROM release_catalog_aggregates WHERE release_id=$1",
        )
        .bind(first.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read restored source aggregate"),
        1
    );
    db.drop().await;
}
