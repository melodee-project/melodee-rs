//! Hard deletion of catalog Artists, Releases, and Tracks (ADR-0211).
//!
//! The selected rows, their dependent evidence, the originating processing
//! work item, and orphaned media asset rows are removed. Audit rows retain the
//! fact that an operator confirmed the deletion.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, PublishedCatalogFixture, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

struct SeededRelease {
    work_item_id: Uuid,
    internal_id: Uuid,
    public_id: Uuid,
}

async fn seed_release(
    db: &TestDb,
    store: &CatalogStore,
    ordinal: u32,
    title: &str,
    artist: &str,
) -> SeededRelease {
    let fixture: PublishedCatalogFixture = seed_published_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            release_artist: artist,
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let worker = WorkerId::new();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
        .await
        .expect("claim Release fixture");
    assert_eq!(claimed.len(), 1);
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("catalog-hard-delete-v1", "catalog-test"),
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("import Release fixture");
    let public_id: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read seeded Release public id");
    SeededRelease {
        work_item_id: fixture.work_item_id,
        internal_id: fixture.release_id.into_uuid(),
        public_id,
    }
}

async fn count(db: &TestDb, sql: &'static str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("count rows")
}

#[tokio::test]
async fn release_hard_delete_removes_catalog_rows_work_item_and_media_assets() {
    let db = TestDb::create("catalog_hard_release").await;
    let store = CatalogStore;
    let release = seed_release(
        &db,
        &store,
        100,
        "Hard Delete Release",
        "Hard Delete Artist",
    )
    .await;

    let track_internal: Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 LIMIT 1")
            .bind(release.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("read Track internal id");
    let preview = store
        .preview_hard_delete_releases(&db.pool, &[release.public_id])
        .await
        .expect("preview Release hard deletion");
    assert_eq!(preview.impact.releases, 1);
    assert_eq!(preview.impact.tracks, 1);
    assert_eq!(preview.impact.media_files, 1);

    let outcome = store
        .hard_delete_releases(
            &db.pool,
            &[release.public_id],
            "catalog-admin",
            "operator confirmed deletion",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("hard-delete Release");
    assert_eq!(outcome.deleted_releases, vec![release.public_id]);

    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            release.internal_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE release_id=$1",
            release.internal_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM catalog_imports WHERE release_id=$1",
            release.internal_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM release_work_items WHERE id=$1",
            release.work_item_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM observed_artists WHERE release_id=$1",
            release.internal_id
        )
        .await,
        0
    );
    let projection_deletes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_projection_outbox
          WHERE operation='delete'
            AND ((entity_type='release' AND entity_id=$1)
                 OR (entity_type='track' AND entity_id=$2))",
    )
    .bind(release.internal_id)
    .bind(track_internal)
    .fetch_one(&db.pool)
    .await
    .expect("count projection deletes");
    assert_eq!(projection_deletes, 2);

    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_audit_events
          WHERE entity_type='release' AND entity_id=$1 AND event_type='release_hard_deleted'",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read deletion audit");
    assert_eq!(audit, 1);
}

#[tokio::test]
async fn track_hard_delete_keeps_release_and_recomputes_its_aggregate() {
    let db = TestDb::create("catalog_hard_track").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 200, "Keep This Release", "Keep This Artist").await;
    let track_public: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read Track public id");

    let outcome = store
        .hard_delete_tracks(
            &db.pool,
            &[track_public],
            "catalog-editor",
            "operator confirmed deletion",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("hard-delete Track");
    assert_eq!(outcome.deleted_tracks, vec![track_public]);

    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            release.internal_id
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE release_id=$1",
            release.internal_id
        )
        .await,
        0
    );
    let track_count: i64 = sqlx::query_scalar(
        "SELECT track_count FROM release_catalog_aggregates WHERE release_id=$1",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read Release aggregate");
    assert_eq!(track_count, 0);
}

#[tokio::test]
async fn artist_hard_delete_removes_the_artist_and_releases_it_owns() {
    let db = TestDb::create("catalog_hard_artist").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 300, "Owned Release", "Owned Artist").await;
    let artist_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM artists WHERE canonical_name='Owned Artist'")
            .fetch_one(&db.pool)
            .await
            .expect("read Artist public id");
    let artist_internal: Uuid = sqlx::query_scalar("SELECT id FROM artists WHERE public_api_id=$1")
        .bind(artist_public)
        .fetch_one(&db.pool)
        .await
        .expect("read Artist internal id");

    let preview = store
        .preview_hard_delete_artists(&db.pool, &[artist_public])
        .await
        .expect("preview Artist hard deletion");
    assert_eq!(preview.impact.artists, 1);
    assert_eq!(preview.impact.releases, 1);

    let outcome = store
        .hard_delete_artists(
            &db.pool,
            &[artist_public],
            "catalog-editor",
            "operator confirmed deletion",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("hard-delete Artist");
    assert_eq!(outcome.deleted_artists, vec![artist_public]);
    assert_eq!(outcome.deleted_releases, vec![release.public_id]);

    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM artists WHERE id=$1",
            artist_internal
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            release.internal_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE release_id=$1",
            release.internal_id
        )
        .await,
        0
    );
}
