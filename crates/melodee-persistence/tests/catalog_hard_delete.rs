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

async fn seed_unobserved_track(db: &TestDb, release_id: Uuid) -> Uuid {
    let original: Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1")
            .bind(release_id)
            .fetch_one(&db.pool)
            .await
            .expect("source Track");
    let track = Uuid::now_v7();
    let mut setup = db.pool.begin().await.expect("new Track transaction");
    sqlx::query(
        "INSERT INTO tracks (id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
                            ordinal,title,normalized_title,sort_title,created_at,updated_at)
         SELECT $1,$2,release_id,audio_identity_id,playable_media_asset_id,2,
                'Unobserved','unobserved','Unobserved',created_at,updated_at FROM tracks WHERE id=$3",
    ).bind(track).bind(Uuid::now_v7()).bind(original).execute(&mut *setup).await.expect("insert unobserved Track");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,created_at)
         SELECT id,playable_media_asset_id,'playable',created_at FROM tracks WHERE id=$1",
    )
    .bind(track)
    .execute(&mut *setup)
    .await
    .expect("link playable asset");
    setup.commit().await.expect("commit unobserved Track");
    track
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
async fn track_hard_delete_waits_for_release_merge_and_revalidates_binding() {
    let db = TestDb::create("hard_track_merge_order").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 211, "Merge Source", "Source Artist").await;
    let target = seed_release(&db, &store, 212, "Merge Target", "Target Artist").await;
    let track_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
            .bind(source.internal_id)
            .fetch_one(&db.pool)
            .await
            .expect("source Track");
    let mut holder = db.pool.begin().await.expect("holder transaction");
    sqlx::query("SELECT id FROM releases WHERE id=$1 FOR UPDATE")
        .bind(source.internal_id)
        .execute(&mut *holder)
        .await
        .expect("hold source Release");
    let merge_pool = db.pool.clone();
    let source_id = source.internal_id;
    let target_id = target.internal_id;
    let merge = tokio::spawn(async move {
        CatalogStore
            .merge_release(
                &merge_pool,
                source_id,
                target_id,
                "test",
                "lock order",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    // Enqueue the merge first so it wins the parent Release lock; the hard
    // delete must not hold the Track while queued behind it.
    wait_for_release_lock_waiters(&db, 1).await;
    let delete_pool = db.pool.clone();
    let deletion = tokio::spawn(async move {
        CatalogStore
            .hard_delete_tracks(
                &delete_pool,
                &[track_public],
                "test",
                "lock order",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_release_lock_waiters(&db, 2).await;
    let mut probe = db.pool.begin().await.expect("probe transaction");
    sqlx::query("SELECT id FROM tracks WHERE public_api_id=$1 FOR UPDATE NOWAIT")
        .bind(track_public)
        .execute(&mut *probe)
        .await
        .expect("waiting delete must not lock Track before Release");
    probe.rollback().await.expect("release probe");
    holder.commit().await.expect("release parent");
    let (merge, deletion) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        (
            merge.await.expect("merge task"),
            deletion.await.expect("delete task"),
        )
    })
    .await
    .expect("operations finish without deadlock");
    merge.expect("Release merge succeeds");
    assert!(matches!(
        deletion,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    let current_release: Uuid =
        sqlx::query_scalar("SELECT release_id FROM tracks WHERE public_api_id=$1")
            .bind(track_public)
            .fetch_one(&db.pool)
            .await
            .expect("Track survives");
    assert_eq!(current_release, target.internal_id);
    db.drop().await;
}

async fn wait_for_release_lock_waiters(db: &TestDb, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                  AND wait_event_type='Lock' AND query LIKE '%FROM releases%'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("count Release waiters");
            if waiters >= minimum {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Release lock waiters");
}

#[tokio::test]
async fn artist_hard_delete_and_catalog_import_serialize_on_name_before_rows() {
    let db = TestDb::create("hard_artist_import_name_order").await;
    let store = CatalogStore;
    let existing = seed_release(&db, &store, 225, "Existing", "Same Name").await;
    let (artist_public, normalized_name): (Uuid, String) = sqlx::query_as(
        "SELECT artist.public_api_id,artist.normalized_name FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(existing.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("existing Artist");
    let incoming = seed_published_release(
        &db,
        226,
        CatalogFixturePresentation {
            release_title: "Incoming",
            release_artist: "Same Name",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let worker = WorkerId::new();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), OffsetDateTime::now_utc())
        .await
        .expect("claim incoming Release");
    assert_eq!(claimed.len(), 1);

    let mut holder = db.pool.begin().await.expect("advisory holder");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&normalized_name)
        .execute(&mut *holder)
        .await
        .expect("hold Artist name");
    let delete_pool = db.pool.clone();
    let deletion = tokio::spawn(async move {
        CatalogStore
            .hard_delete_artists(
                &delete_pool,
                &[artist_public],
                "test",
                "name lock race",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_name_lock_waiters(&db, 1).await;
    let import_pool = db.pool.clone();
    let item = incoming.work_item_id;
    let import = tokio::spawn(async move {
        CatalogStore
            .import_manifest_v3(
                &import_pool,
                item,
                &worker,
                &CatalogImportRequest::new("hard-delete-name-race", "test"),
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_name_lock_waiters(&db, 2).await;
    holder.commit().await.expect("release Artist name");
    let (deletion, import) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        (
            deletion.await.expect("delete task"),
            import.await.expect("import task"),
        )
    })
    .await
    .expect("name-conflicting operations complete without deadlock");
    deletion.expect("Artist deletion succeeds");
    import.expect("incoming catalog import succeeds");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM artists WHERE public_api_id=$1",
            artist_public
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM catalog_imports WHERE work_item_id=$1",
            item
        )
        .await,
        1
    );
    db.drop().await;
}

async fn wait_for_name_lock_waiters(db: &TestDb, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                  AND wait_event_type='Lock' AND query LIKE 'SELECT lock_artist_identity_names%'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("count Artist name waiters");
            if waiters >= minimum {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Artist name waiters");
}

#[tokio::test]
async fn artist_hard_delete_rejects_name_change_while_waiting_on_advisory_lock() {
    let db = TestDb::create("hard_artist_name_revalidation").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 227, "Existing", "Original Name").await;
    let (artist, artist_public, normalized_name): (Uuid, Uuid, String) = sqlx::query_as(
        "SELECT artist.id,artist.public_api_id,artist.normalized_name FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("existing Artist");
    let mut holder = db.pool.begin().await.expect("advisory holder");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&normalized_name)
        .execute(&mut *holder)
        .await
        .expect("hold Artist name");
    let pool = db.pool.clone();
    let deletion = tokio::spawn(async move {
        CatalogStore
            .hard_delete_artists(
                &pool,
                &[artist_public],
                "test",
                "name changed",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_name_lock_waiters(&db, 1).await;
    // Model an older/raw writer bypassing the advisory protocol while the
    // deletion waits: stale name locks must never authorize deletion.
    sqlx::query("UPDATE artists SET normalized_name='changed while waiting' WHERE id=$1")
        .bind(artist)
        .execute(&db.pool)
        .await
        .expect("change Artist name");
    holder.commit().await.expect("release Artist name");
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), deletion)
        .await
        .expect("deletion completes")
        .expect("delete task");
    assert!(matches!(
        result,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    assert_eq!(
        count(&db, "SELECT count(*) FROM artists WHERE id=$1", artist).await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            release.internal_id
        )
        .await,
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn artist_hard_delete_handles_multiple_bounded_name_pages() {
    let db = TestDb::create("hard_artist_name_pages").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 228, "Existing", "Redirect Target").await;
    let (artist, artist_public): (Uuid, Uuid) = sqlx::query_as(
        "SELECT artist.id,artist.public_api_id FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("redirect target");
    sqlx::query(
        "INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,
                              artist_type,status,merged_into_artist_id,created_at,updated_at)
         SELECT uuidv7(),uuidv7(),'Redirect '||g,'redirect '||g,'Redirect '||g,
                'unknown','merged',$1,now(),now() FROM generate_series(1,300) AS g",
    )
    .bind(artist)
    .execute(&db.pool)
    .await
    .expect("create redirect closure");
    store
        .hard_delete_artists(
            &db.pool,
            &[artist_public],
            "test",
            "bounded name pages",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("delete target Artist");
    let survivors: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artists WHERE canonical_name LIKE 'Redirect %'
           AND status='active' AND merged_into_artist_id IS NULL",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count surviving redirect sources");
    assert_eq!(survivors, 300);
    db.drop().await;
}

/// A raw Track move can commit after deletion captures the Release membership
/// but before it acquires the parent lock. Both Release and Artist deletion
/// must reject the stale membership before deleting dependent observations.
async fn moved_track_rejects_stale_release_deletion(delete_artist: bool) {
    let db = TestDb::create("hard_release_moved_track").await;
    let store = CatalogStore;
    let source = seed_release(&db, &store, 231, "Race Source", "Race Owner").await;
    let target = seed_release(&db, &store, 232, "Race Target", "Target Owner").await;
    let original: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(source.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("source Track");
    // The fixture Track has an observation with a composite Track/Release FK.
    // A separate Track without observations can legally be moved by raw SQL.
    let track = Uuid::now_v7();
    let mut setup = db.pool.begin().await.expect("new Track transaction");
    sqlx::query(
        "INSERT INTO tracks (id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
                            ordinal,title,normalized_title,sort_title,created_at,updated_at)
         SELECT $1,$2,release_id,audio_identity_id,playable_media_asset_id,2,
                'Unobserved','unobserved','Unobserved',created_at,updated_at FROM tracks WHERE id=$3",
    ).bind(track).bind(Uuid::now_v7()).bind(original).execute(&mut *setup).await.expect("insert unobserved Track");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,created_at)
         SELECT id,playable_media_asset_id,'playable',created_at FROM tracks WHERE id=$1",
    )
    .bind(track)
    .execute(&mut *setup)
    .await
    .expect("link playable asset");
    setup.commit().await.expect("commit unobserved Track");
    let artist_public: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(source.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("source owner");
    let mut holder = db.pool.begin().await.expect("holder transaction");
    sqlx::query("SELECT id FROM releases WHERE id=$1 FOR UPDATE")
        .bind(source.internal_id)
        .execute(&mut *holder)
        .await
        .expect("hold source Release");
    let pool = db.pool.clone();
    let source_public = source.public_id;
    let deletion = tokio::spawn(async move {
        if delete_artist {
            CatalogStore
                .hard_delete_artists(
                    &pool,
                    &[artist_public],
                    "test",
                    "membership race",
                    OffsetDateTime::now_utc(),
                )
                .await
        } else {
            CatalogStore
                .hard_delete_releases(
                    &pool,
                    &[source_public],
                    "test",
                    "membership race",
                    OffsetDateTime::now_utc(),
                )
                .await
        }
    });
    wait_for_release_lock_waiters(&db, 1).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::query("UPDATE tracks SET release_id=$2,ordinal=2 WHERE id=$1")
            .bind(track)
            .bind(target.internal_id)
            .execute(&db.pool),
    )
    .await
    .expect("raw move does not wait for old Release")
    .expect("move Track");
    holder.commit().await.expect("release source parent");
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), deletion)
        .await
        .expect("deletion finishes without deadlock")
        .expect("delete task");
    assert!(matches!(
        result,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    let current_release: Uuid = sqlx::query_scalar("SELECT release_id FROM tracks WHERE id=$1")
        .bind(track)
        .fetch_one(&db.pool)
        .await
        .expect("Track remains");
    assert_eq!(current_release, target.internal_id);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            source.internal_id
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM release_work_items WHERE id=$1",
            source.work_item_id
        )
        .await,
        1
    );
    assert!(
        count(
            &db,
            "SELECT count(*) FROM observed_artists WHERE release_id=$1",
            source.internal_id
        )
        .await
            > 0
    );
    db.drop().await;
}

#[tokio::test]
async fn release_delete_rejects_track_moved_during_parent_wait() {
    moved_track_rejects_stale_release_deletion(false).await;
}

#[tokio::test]
async fn artist_owned_release_delete_rejects_track_moved_during_parent_wait() {
    moved_track_rejects_stale_release_deletion(true).await;
}

#[tokio::test]
async fn release_delete_rechecks_incoming_track_after_parent_lock_upgrade() {
    let db = TestDb::create("hard_release_incoming_track").await;
    let store = CatalogStore;
    let selected = seed_release(&db, &store, 241, "Selected", "Selected Owner").await;
    let other = seed_release(&db, &store, 242, "Other", "Other Owner").await;
    let incoming = seed_unobserved_track(&db, other.internal_id).await;
    // Retain FK-compatible KEY SHARE through the deletion's first membership
    // check so the concurrent move can commit just before its parent upgrade.
    let mut mover = db.pool.begin().await.expect("move transaction");
    sqlx::query("SELECT id FROM releases WHERE id=$1 FOR KEY SHARE")
        .bind(selected.internal_id)
        .execute(&mut *mover)
        .await
        .expect("hold FK-compatible parent");
    let pool = db.pool.clone();
    let selected_public = selected.public_id;
    let deletion = tokio::spawn(async move {
        CatalogStore
            .hard_delete_releases(
                &pool,
                &[selected_public],
                "test",
                "incoming Track race",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_release_lock_waiters(&db, 1).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::query("UPDATE tracks SET release_id=$2,ordinal=2 WHERE id=$1")
            .bind(incoming)
            .bind(selected.internal_id)
            .execute(&mut *mover),
    )
    .await
    .expect("move does not deadlock")
    .expect("move incoming Track");
    mover.commit().await.expect("commit incoming move");
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), deletion)
        .await
        .expect("deletion finishes")
        .expect("delete task");
    assert!(matches!(
        result,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE release_id=$1",
            selected.internal_id
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM release_work_items WHERE id=$1",
            selected.work_item_id
        )
        .await,
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn release_delete_allows_move_between_selected_parents_then_rejects_snapshot() {
    let db = TestDb::create("hard_release_selected_move").await;
    let store = CatalogStore;
    let first = seed_release(&db, &store, 251, "First", "First Owner").await;
    let second = seed_release(&db, &store, 252, "Second", "Second Owner").await;
    let moved = seed_unobserved_track(&db, first.internal_id).await;
    let mut mover = db.pool.begin().await.expect("move transaction");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(moved)
        .execute(&mut *mover)
        .await
        .expect("hold moved Track");
    let pool = db.pool.clone();
    let public_ids = [first.public_id, second.public_id];
    let deletion = tokio::spawn(async move {
        CatalogStore
            .hard_delete_releases(
                &pool,
                &public_ids,
                "test",
                "selected-parent race",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND query LIKE '%melodee_hard_delete_track_scope%'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("count Track waiters");
            if waiters > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("deletion waits for captured Track");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::query("UPDATE tracks SET release_id=$2,ordinal=2 WHERE id=$1")
            .bind(moved)
            .bind(second.internal_id)
            .execute(&mut *mover),
    )
    .await
    .expect("FK check remains compatible with initial parent lock")
    .expect("move Track");
    mover.commit().await.expect("commit move");
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), deletion)
        .await
        .expect("deletion finishes")
        .expect("delete task");
    assert!(matches!(
        result,
        Err(melodee_persistence::CatalogStoreError::MutationConflict)
    ));
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE release_id=$1",
            second.internal_id
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            first.internal_id
        )
        .await,
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn opposing_artist_deletions_lock_shared_release_and_track_closure() {
    let db = TestDb::create("hard_artist_shared_order").await;
    let store = CatalogStore;
    let first = seed_release(&db, &store, 221, "First Owned", "First Owner").await;
    let second = seed_release(&db, &store, 222, "Second Owned", "Second Owner").await;
    let (artist_a, public_a): (Uuid, Uuid) = sqlx::query_as(
        "SELECT artist.id,artist.public_api_id FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(first.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("first owner");
    let (artist_b, public_b): (Uuid, Uuid) = sqlx::query_as(
        "SELECT artist.id,artist.public_api_id FROM release_artists credit
         JOIN artists artist ON artist.id=credit.artist_id
         WHERE credit.release_id=$1 AND credit.role='primary'",
    )
    .bind(second.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("second owner");
    sqlx::query(
        "UPDATE track_artists credit SET artist_id=CASE
            WHEN track.release_id=$1 THEN $4 ELSE $3 END
         FROM tracks track WHERE track.id=credit.track_id
           AND track.release_id=ANY($2) AND credit.role='primary'",
    )
    .bind(first.internal_id)
    .bind([first.internal_id, second.internal_id].as_slice())
    .bind(artist_a)
    .bind(artist_b)
    .execute(&db.pool)
    .await
    .expect("cross-credit owned Tracks");

    let mut holder = db.pool.begin().await.expect("holder transaction");
    sqlx::query("SELECT id FROM releases WHERE id=$1 FOR UPDATE")
        .bind(first.internal_id)
        .execute(&mut *holder)
        .await
        .expect("hold shared Release");
    let pool_a = db.pool.clone();
    let pool_b = db.pool.clone();
    let delete_a = tokio::spawn(async move {
        CatalogStore
            .hard_delete_artists(
                &pool_a,
                &[public_a],
                "test",
                "shared closure",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    let delete_b = tokio::spawn(async move {
        CatalogStore
            .hard_delete_artists(
                &pool_b,
                &[public_b],
                "test",
                "shared closure",
                OffsetDateTime::now_utc(),
            )
            .await
    });
    wait_for_release_lock_waiters(&db, 2).await;
    holder.commit().await.expect("release shared parent");
    let (delete_a, delete_b) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        (
            delete_a.await.expect("first task"),
            delete_b.await.expect("second task"),
        )
    })
    .await
    .expect("deletions complete without deadlock");
    // A concurrent operation may invalidate the other's captured closure.
    // Rejecting the stale snapshot is safe; retry must converge.
    for (result, public_id) in [(delete_a, public_a), (delete_b, public_b)] {
        if let Err(error) = result {
            assert!(matches!(
                error,
                melodee_persistence::CatalogStoreError::MutationConflict
            ));
            CatalogStore
                .hard_delete_artists(
                    &db.pool,
                    &[public_id],
                    "test",
                    "retry stale closure",
                    OffsetDateTime::now_utc(),
                )
                .await
                .expect("retry deletion");
        }
    }
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            first.internal_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM releases WHERE id=$1",
            second.internal_id
        )
        .await,
        0
    );
    db.drop().await;
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
    let durable_name_seeds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM catalog_identity_reconciliation_seed
         WHERE normalized_name='owned artist' AND artist_id IS NULL AND release_id IS NULL",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read deletion-safe identity seed");
    assert!(
        durable_name_seeds > 0,
        "deleted Artist names must remain queued"
    );

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

#[tokio::test]
async fn hard_deletes_preserve_scoped_identity_seeds_and_roll_back_on_failure() {
    for (kind, db_name, ordinal) in [
        ("artist", "delete_identity_seed_artist", 410),
        ("release", "delete_identity_seed_release", 420),
        ("track", "delete_identity_seed_track", 430),
    ] {
        let db = TestDb::create(db_name).await;
        let store = CatalogStore;
        let target = seed_release(&db, &store, ordinal, "Deletion Target", "Observed Artist").await;
        let target_artist: (Uuid, Uuid) = sqlx::query_as(
            "SELECT a.id, a.public_api_id FROM artists a
             JOIN release_artists c ON c.artist_id=a.id WHERE c.release_id=$1 LIMIT 1",
        )
        .bind(target.internal_id)
        .fetch_one(&db.pool)
        .await
        .expect("read target Artist");
        if kind == "artist" {
            sqlx::query("UPDATE artists SET disambiguation='fixture separation' WHERE id=$1")
                .bind(target_artist.0)
                .execute(&db.pool)
                .await
                .expect("separate survivor Artist");
        }
        let survivor = seed_release(
            &db,
            &store,
            ordinal + 1,
            "Surviving Release",
            "Observed Artist",
        )
        .await;
        let survivor_artist: Uuid =
            sqlx::query_scalar("SELECT artist_id FROM release_artists WHERE release_id=$1 LIMIT 1")
                .bind(survivor.internal_id)
                .fetch_one(&db.pool)
                .await
                .expect("read survivor Artist");
        sqlx::query("UPDATE artists SET disambiguation=NULL WHERE id=ANY($1)")
            .bind([target_artist.0, survivor_artist])
            .execute(&db.pool)
            .await
            .expect("clear fixture separation");
        let candidate = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name,
                                 artist_type, status, revision, created_at, updated_at)
             SELECT $1, $2, canonical_name, normalized_name, sort_name, artist_type, status, 1, now(), now()
             FROM artists WHERE id=$3",
        )
        .bind(candidate)
        .bind(Uuid::now_v7())
        .bind(survivor_artist)
        .execute(&db.pool)
        .await
        .expect("add surviving same-name candidate");
        if kind == "artist" {
            // A deleted Artist's Observation may have decisions referring to
            // another Artist whose name is different from the deleted one.
            let other = Uuid::now_v7();
            sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,revision,created_at,updated_at) VALUES ($1,$2,'Other Candidate','other candidate','Other Candidate','unknown','active',1,now(),now())")
                .bind(other).bind(Uuid::now_v7()).execute(&db.pool).await.expect("add other candidate");
            let prior: Uuid = sqlx::query_scalar("INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,predecessor_decision_id,payload,created_at) SELECT uuidv7(),d.observed_artist_id,$2,'proposed','automated',5000,'test',false,d.id,'{}'::jsonb,now() FROM artist_match_decisions d JOIN artist_match_current c ON c.decision_id=d.id JOIN observed_artists o ON o.id=d.observed_artist_id WHERE o.release_id=$1 LIMIT 1 RETURNING id")
                .bind(survivor.internal_id).bind(target_artist.0).fetch_one(&db.pool).await.expect("add deleted-Artist decision");
            sqlx::query("INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,predecessor_decision_id,payload,created_at) SELECT uuidv7(),d.observed_artist_id,$2,'proposed','automated',5000,'test',false,d.id,'{}'::jsonb,now() FROM artist_match_decisions d WHERE d.id=$1")
                .bind(prior).bind(other).execute(&db.pool).await.expect("add candidate decision");
        }
        let track_public: Uuid =
            sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1 LIMIT 1")
                .bind(target.internal_id)
                .fetch_one(&db.pool)
                .await
                .expect("read target Track");
        sqlx::query("DELETE FROM catalog_identity_reconciliation_seed")
            .execute(&db.pool)
            .await
            .expect("clear earlier seeds");
        sqlx::query("CREATE FUNCTION reject_fixture_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected delete failure'; END $$")
            .execute(&db.pool).await.expect("create failure injection");
        let (create_trigger, drop_trigger) = match kind {
            "artist" => (
                "CREATE TRIGGER reject_fixture_delete BEFORE DELETE ON artists FOR EACH ROW EXECUTE FUNCTION reject_fixture_delete()",
                "DROP TRIGGER reject_fixture_delete ON artists",
            ),
            "release" => (
                "CREATE TRIGGER reject_fixture_delete BEFORE DELETE ON releases FOR EACH ROW EXECUTE FUNCTION reject_fixture_delete()",
                "DROP TRIGGER reject_fixture_delete ON releases",
            ),
            _ => (
                "CREATE TRIGGER reject_fixture_delete BEFORE DELETE ON tracks FOR EACH ROW EXECUTE FUNCTION reject_fixture_delete()",
                "DROP TRIGGER reject_fixture_delete ON tracks",
            ),
        };
        sqlx::query(create_trigger)
            .execute(&db.pool)
            .await
            .expect("inject failure");
        let delete = |store: CatalogStore, pool: sqlx::PgPool| async move {
            match kind {
                "artist" => {
                    store
                        .hard_delete_artists(
                            &pool,
                            &[target_artist.1],
                            "test",
                            "test",
                            OffsetDateTime::now_utc(),
                        )
                        .await
                }
                "release" => {
                    store
                        .hard_delete_releases(
                            &pool,
                            &[target.public_id],
                            "test",
                            "test",
                            OffsetDateTime::now_utc(),
                        )
                        .await
                }
                _ => {
                    store
                        .hard_delete_tracks(
                            &pool,
                            &[track_public],
                            "test",
                            "test",
                            OffsetDateTime::now_utc(),
                        )
                        .await
                }
            }
        };
        assert!(
            delete(CatalogStore, db.pool.clone()).await.is_err(),
            "injected failure must abort {kind} deletion"
        );
        let seed_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
                .fetch_one(&db.pool)
                .await
                .expect("count rolled-back seeds");
        assert_eq!(
            seed_count, 0,
            "{kind} deletion failure must roll back its seeds"
        );
        sqlx::query(drop_trigger)
            .execute(&db.pool)
            .await
            .expect("remove failure injection");
        delete(CatalogStore, db.pool.clone())
            .await
            .expect("delete after retry");
        if kind == "artist" {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed WHERE normalized_name='other candidate'")
                .fetch_one(&db.pool).await.expect("read surviving candidate seed");
            assert!(
                count > 0,
                "deleted Observation decisions must seed surviving Artists"
            );
        }
        let seed_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM catalog_identity_reconciliation_seed WHERE normalized_name='observed artist' AND artist_id IS NULL AND release_id IS NULL",
        )
        .fetch_all(&db.pool)
        .await
        .expect("read surviving name-only seeds");
        assert!(
            !seed_ids.is_empty(),
            "{kind} deletion must persist FK-independent identity seeds"
        );
        let proposals = store
            .seeded_name_only_artist_match_proposal_candidates(&db.pool, &seed_ids, 64)
            .await
            .expect("find affected name candidates");
        assert!(
            proposals
                .iter()
                .any(|row| row.candidate_artist_id == candidate),
            "{kind} deletion seed must drive scoped discovery"
        );
        db.drop().await;
    }
}
