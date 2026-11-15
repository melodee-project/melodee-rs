//! Administrative Artist and Release deletion (ADR-0199).
//!
//! Deletion is a durable tombstone: identities, credits, Tracks, media
//! evidence, and audit history are never row-deleted. Only owned Releases are
//! retired, and only after the caller removed their storage.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, PublishedCatalogFixture, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::ArtistId;
use melodee_persistence::{
    ArtistDelete, ArtistDeleteMode, CatalogImportRequest, CatalogStore, CatalogStoreError,
    ReleaseDelete, ReleaseDeleteMode, WorkerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

struct SeededRelease {
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
            &CatalogImportRequest::new("catalog-delete-v1", "catalog-test"),
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
        internal_id: fixture.release_id.into_uuid(),
        public_id,
    }
}

async fn artist_public_id(db: &TestDb, canonical_name: &str) -> Uuid {
    sqlx::query_scalar("SELECT public_api_id FROM artists WHERE canonical_name=$1")
        .bind(canonical_name)
        .fetch_one(&db.pool)
        .await
        .expect("read Artist public id")
}

async fn internal_artist_id(db: &TestDb, public_id: Uuid) -> ArtistId {
    ArtistId::from_uuid(
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM artists WHERE public_api_id=$1")
            .bind(public_id)
            .fetch_one(&db.pool)
            .await
            .expect("read Artist internal id"),
    )
}

fn artist_delete(public_api_id: Uuid, artist_id: ArtistId, mode: ArtistDeleteMode) -> ArtistDelete {
    ArtistDelete {
        artist_id,
        public_api_id,
        mode,
        actor: "catalog-admin".to_owned(),
        reason: "administrator removed a wrong Artist identity".to_owned(),
        occurred_at: OffsetDateTime::now_utc(),
    }
}

#[tokio::test]
async fn catalog_only_artist_delete_retires_identity_and_keeps_catalog_content() {
    let db = TestDb::create("catalog_artist_delete_catalog").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 100, "Kept Release", "Kept Artist").await;
    let artist_public = artist_public_id(&db, "Kept Artist").await;
    let artist_id = internal_artist_id(&db, artist_public).await;

    let preview = store
        .preview_artist_delete(&db.pool, &[artist_public])
        .await
        .expect("preview Artist deletion");
    assert_eq!(preview.artists.len(), 1);
    assert_eq!(preview.artists[0].owned_releases, 1);
    assert_eq!(preview.impact.releases, 1);
    assert_eq!(preview.impact.tracks, 1);
    assert_eq!(preview.impact.retained_shared_releases, 0);

    let outcome = store
        .delete_artists(
            &db.pool,
            &[artist_delete(
                artist_public,
                artist_id,
                ArtistDeleteMode::CatalogOnly,
            )],
        )
        .await
        .expect("delete Artist catalog-only");
    assert_eq!(outcome.deleted_artists, vec![artist_public]);
    assert!(outcome.deleted_releases.is_empty());

    let artist_status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id=$1")
        .bind(artist_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read deleted Artist status");
    assert_eq!(artist_status, "deleted");
    let release_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(release.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read kept Release status");
    assert_eq!(release_status, "active");
    let tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(release.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("count kept Tracks");
    assert_eq!(tracks, 1);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_audit_events
          WHERE entity_type='artist' AND entity_id=$1 AND event_type='artist_deleted'",
    )
    .bind(artist_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read Artist deletion audit");
    assert_eq!(audit, 1);

    // The deleted Artist is no longer listed as a live identity.
    let listed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artists WHERE public_api_id=$1 AND status IN ('active','verified')",
    )
    .bind(artist_public)
    .fetch_one(&db.pool)
    .await
    .expect("read Artist visibility");
    assert_eq!(listed, 0);

    // A second deletion is rejected: the tombstone is terminal.
    let repeat = store
        .delete_artists(
            &db.pool,
            &[artist_delete(
                artist_public,
                artist_id,
                ArtistDeleteMode::CatalogOnly,
            )],
        )
        .await;
    assert!(matches!(repeat, Err(CatalogStoreError::MutationConflict)));

    db.drop().await;
}

#[tokio::test]
async fn artist_delete_with_files_retires_only_owned_releases() {
    let db = TestDb::create("catalog_artist_delete_owned").await;
    let store = CatalogStore;
    let owned = seed_release(&db, &store, 110, "Owned Release", "Owner Artist").await;
    let other = seed_release(&db, &store, 111, "Other Release", "Other Artist").await;
    let owner_public = artist_public_id(&db, "Owner Artist").await;
    let owner_id = internal_artist_id(&db, owner_public).await;

    let preview = store
        .preview_artist_delete(&db.pool, &[owner_public])
        .await
        .expect("preview owned Release deletion");
    assert_eq!(preview.impact.releases, 1);
    assert_eq!(preview.impact.tracks, 1);
    assert_eq!(preview.impact.media_files, 1);
    assert!(preview.impact.media_bytes > 0);

    let storage = store
        .artist_delete_release_storage(&db.pool, &[owner_public])
        .await
        .expect("resolve owned Release storage");
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].release_public_id, owned.public_id);
    assert!(!storage[0].canonical_relative_path.trim().is_empty());

    let outcome = store
        .delete_artists(
            &db.pool,
            &[artist_delete(
                owner_public,
                owner_id,
                ArtistDeleteMode::ReleasesAndFiles,
            )],
        )
        .await
        .expect("delete Artist with owned Releases");
    assert_eq!(outcome.deleted_releases, vec![owned.internal_id]);

    let owned_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(owned.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read retired Release status");
    assert_eq!(owned_status, "deleted");
    let other_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(other.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read untouched Release status");
    assert_eq!(other_status, "active");
    // Evidence is retained: Tracks and credits stay on the deleted Release.
    let retained_tracks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(owned.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count retained Tracks");
    assert_eq!(retained_tracks, 1);
    let artist_aggregate: i64 = sqlx::query_scalar(
        "SELECT release_count FROM artist_catalog_aggregates WHERE artist_id=$1",
    )
    .bind(owner_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read recomputed Artist aggregate");
    assert_eq!(artist_aggregate, 0);

    db.drop().await;
}

#[tokio::test]
async fn artist_delete_keeps_releases_shared_with_another_primary_artist() {
    let db = TestDb::create("catalog_artist_delete_shared").await;
    let store = CatalogStore;
    let shared = seed_release(&db, &store, 120, "Shared Release", "First Primary").await;
    let first_public = artist_public_id(&db, "First Primary").await;
    let first_id = internal_artist_id(&db, first_public).await;
    let second_public = artist_public_id(&db, "Observed Artist").await;

    // The fixture's second Artist exists from the Track credit; bill the same
    // Release to it as a second primary Artist.
    sqlx::query(
        "INSERT INTO release_artists (
             id, release_id, artist_id, observed_artist_id, role,
             presentation_order, display_value, join_phrase, created_at
         )
         SELECT $1, $2, artist.id, NULL, 'primary',
                (SELECT max(presentation_order) + 1 FROM release_artists WHERE release_id=$2),
                'Second Primary', ' & ', $3
           FROM artists artist WHERE artist.public_api_id=$4",
    )
    .bind(Uuid::now_v7())
    .bind(shared.internal_id)
    .bind(OffsetDateTime::now_utc())
    .bind(second_public)
    .execute(&db.pool)
    .await
    .expect("add a second primary Release artist");

    let preview = store
        .preview_artist_delete(&db.pool, &[first_public])
        .await
        .expect("preview shared Release deletion");
    assert_eq!(preview.impact.releases, 0);
    assert_eq!(preview.impact.retained_shared_releases, 1);
    assert!(
        preview
            .warnings
            .iter()
            .any(|warning| warning.contains("also billed as primary"))
    );

    let outcome = store
        .delete_artists(
            &db.pool,
            &[artist_delete(
                first_public,
                first_id,
                ArtistDeleteMode::ReleasesAndFiles,
            )],
        )
        .await
        .expect("delete Artist with shared Release");
    assert!(outcome.deleted_releases.is_empty());
    assert_eq!(outcome.retained_shared_releases, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(shared.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read shared Release status");
    assert_eq!(status, "active");

    db.drop().await;
}

#[tokio::test]
async fn bulk_release_delete_retires_selected_releases_and_keeps_evidence() {
    let db = TestDb::create("catalog_release_bulk_delete").await;
    let store = CatalogStore;
    let first = seed_release(&db, &store, 130, "Delete One", "Artist One").await;
    let second = seed_release(&db, &store, 131, "Delete Two", "Artist Two").await;
    let kept = seed_release(&db, &store, 132, "Keep Three", "Artist Three").await;

    let preview = store
        .preview_release_delete(&db.pool, &[first.public_id, second.public_id])
        .await
        .expect("preview Release deletion");
    assert_eq!(preview.impact.releases, 2);
    assert_eq!(preview.impact.tracks, 2);
    assert_eq!(preview.impact.media_files, 2);
    assert_eq!(preview.releases.len(), 2);

    let storage = store
        .release_delete_storage(&db.pool, &[first.public_id, second.public_id])
        .await
        .expect("resolve Release storage");
    assert_eq!(storage.len(), 2);

    let refs = store
        .release_delete_refs(&db.pool, &[first.public_id, second.public_id])
        .await
        .expect("resolve Release refs");
    let actor = "catalog-admin".to_owned();
    let reason = "administrator removed duplicate imports".to_owned();
    let now = OffsetDateTime::now_utc();
    let deletes = refs
        .into_iter()
        .map(|(release_id, public_api_id)| ReleaseDelete {
            release_id,
            public_api_id,
            mode: ReleaseDeleteMode::ReleasesAndFiles,
            actor: actor.clone(),
            reason: reason.clone(),
            occurred_at: now,
        })
        .collect::<Vec<_>>();
    let outcome = store
        .delete_releases(&db.pool, &deletes)
        .await
        .expect("delete selected Releases");
    assert_eq!(outcome.deleted_releases.len(), 2);

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='deleted'), count(*) FILTER (WHERE status='active')
           FROM releases WHERE id = ANY($1)",
    )
    .bind([first.internal_id, second.internal_id, kept.internal_id])
    .fetch_one(&db.pool)
    .await
    .expect("read Release statuses");
    assert_eq!(statuses, (2, 1));
    let retained_credits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_artists WHERE release_id=$1")
            .bind(first.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("count retained Release credits");
    assert!(retained_credits >= 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_audit_events
          WHERE entity_type='release' AND event_type='release_deleted'
            AND entity_id = ANY($1)",
    )
    .bind([first.internal_id, second.internal_id])
    .fetch_one(&db.pool)
    .await
    .expect("read Release deletion audits");
    assert_eq!(audits, 2);

    // A merged Release cannot be deleted from the list surface.
    let merged_event = store
        .merge_release(
            &db.pool,
            first.internal_id,
            kept.internal_id,
            "catalog-admin",
            "merge before delete attempt",
            OffsetDateTime::now_utc(),
        )
        .await;
    // The first Release is already `deleted`, so the merge is rejected.
    assert!(merged_event.is_err());
    let repeat = store
        .preview_release_delete(&db.pool, &[first.public_id])
        .await;
    assert!(matches!(repeat, Err(CatalogStoreError::MutationConflict)));

    db.drop().await;
}
