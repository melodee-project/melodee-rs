//! Hard deletion of catalog Artists, Releases, and Tracks (ADR-0211).
//!
//! The selected rows, their dependent evidence, the originating processing
//! work item, and orphaned media asset rows are removed. Audit rows retain the
//! fact that an operator confirmed the deletion.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

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

#[tokio::test]
async fn track_hard_delete_rejects_active_admin_media_reservation() {
    let db = TestDb::create("catalog_hard_track_reservation").await;
    let store = CatalogStore;
    let release = seed_release(&db, &store, 299, "Reserved Release", "Reserved Artist").await;
    let track_public: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .expect("read track");
    let reservation = melodee_persistence::WorkItemStore
        .begin_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
        )
        .await
        .expect("reserve release scope");
    assert!(
        store
            .hard_delete_tracks(
                &db.pool,
                &[track_public],
                "operator",
                "test",
                OffsetDateTime::now_utc()
            )
            .await
            .is_err()
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE public_api_id=$1",
            track_public
        )
        .await,
        1
    );
    melodee_persistence::WorkItemStore
        .finish_admin_media_reservation(&db.pool, &reservation)
        .await
        .expect("release synthetic intent");
    store
        .hard_delete_tracks(
            &db.pool,
            &[track_public],
            "operator",
            "test",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("delete after guard release");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tracks WHERE public_api_id=$1",
            track_public
        )
        .await,
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn maintenance_media_closure_is_paged_deduplicated_and_acknowledged_exactly() {
    let db = TestDb::create("maintenance_media_pages").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let selected = seed_release(&db, &catalog, 510, "Selected", "Selected Artist").await;
    let other = seed_release(&db, &catalog, 511, "Other", "Other Artist").await;
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES($1,$2,'synthetic-generation','staged','verified_copy',repeat('a',64),clock_timestamp(),clock_timestamp())")
        .bind(selected.work_item_id).bind(format!("releases/{}/source",selected.work_item_id)).execute(&db.pool).await.unwrap();

    let (original,original_asset):(Uuid,Uuid)=sqlx::query_as("SELECT id,playable_media_asset_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1").bind(selected.internal_id).fetch_one(&db.pool).await.unwrap();
    let other_asset: Uuid = sqlx::query_scalar(
        "SELECT playable_media_asset_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(other.internal_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at) VALUES($1,$2,'alternate',clock_timestamp())").bind(original).bind(other_asset).execute(&mut *tx).await.unwrap();
    // Same bytes may be linked more than once within the selected scope.
    sqlx::query("INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at) VALUES($1,$2,'alternate',clock_timestamp())")
        .bind(original).bind(original_asset).execute(&mut *tx).await.unwrap();
    for ordinal in 2_i32..=21 {
        let asset = Uuid::now_v7();
        let track = Uuid::now_v7();
        sqlx::query("INSERT INTO media_assets(id,canonical_relative_path,byte_hash,byte_size,mime_type,container,codec,sample_rate_hz,channels,bitrate_bps,duration_ms,created_at) SELECT $1,$2,byte_hash,byte_size,mime_type,container,codec,sample_rate_hz,channels,bitrate_bps,duration_ms,created_at FROM media_assets WHERE id=$3").bind(asset).bind(format!("synthetic/{asset}.mp3")).bind(original_asset).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO tracks(id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,normalized_title,sort_title,created_at,updated_at) SELECT $1,$2,release_id,audio_identity_id,$3,$4,'Synthetic','synthetic','Synthetic',created_at,updated_at FROM tracks WHERE id=$5").bind(track).bind(Uuid::now_v7()).bind(asset).bind(ordinal).bind(original).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at) VALUES($1,$2,'playable',clock_timestamp())").bind(track).bind(asset).execute(&mut *tx).await.unwrap();
    }
    let exclusive_image = format!("{}-5", "a".repeat(64));
    let shared_image = format!("{}-5", "b".repeat(64));
    for key in [&exclusive_image, &shared_image] {
        sqlx::query("INSERT INTO asset_files(asset_key,size_bytes) VALUES($1,5)")
            .bind(key)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    for (release, key) in [
        (selected.internal_id, &exclusive_image),
        (other.internal_id, &shared_image),
    ] {
        sqlx::query("INSERT INTO catalog_entity_images(entity_type,release_id,image_mime_type,asset_key,uploaded_by,created_at,updated_at) VALUES('release',$1,'image/jpeg',$2,'test',clock_timestamp(),clock_timestamp())")
            .bind(release).bind(key).execute(&mut *tx).await.unwrap();
    }
    for (provider, key) in [("deezer", &exclusive_image), ("discogs", &shared_image)] {
        sqlx::query("INSERT INTO catalog_provider_images(id,entity_type,entity_id,provider,source_url,image_mime_type,asset_key,confidence,observed_at) VALUES($1,'release',$2,$3,'https://example.invalid/image','image/jpeg',$4,80,clock_timestamp())")
            .bind(Uuid::now_v7()).bind(selected.internal_id).bind(provider).bind(key).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[selected.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[selected.public_id],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    assert!(
        store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    let mut pages = 0;
    while !store
        .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
        .await
        .unwrap()
    {
        pages += 1;
        assert!(pages < 10);
    }
    assert!(pages >= 2);
    let (entries, snapshots): (i64, i64) =
        sqlx::query_as("SELECT entries,snapshots FROM admin_media_snapshot_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(entries >= 21);
    assert_eq!(snapshots, 1);
    let media_entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_media_snapshot_entries WHERE reservation_id=$1",
    )
    .bind(reservation.id())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(media_entries, 21);
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.unwrap();
    let current = store
        .reclaim_admin_media_reservation(
            &db.pool,
            reservation.id(),
            "operator",
            time::Duration::minutes(1),
        )
        .await
        .unwrap();
    assert!(
        store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    let reservation = current;
    let mut acked = 0;
    loop {
        let page = store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 16);
        assert!(page.iter().all(|entry| entry.asset_id != other_asset));
        let ids: Vec<Uuid> = page.iter().map(|entry| entry.asset_id).collect();
        let ack = Uuid::now_v7();
        let mut wrong = ids.clone();
        wrong.pop();
        assert!(
            store
                .acknowledge_maintenance_media_page(&db.pool, &fence, &reservation, ack, &wrong)
                .await
                .is_err()
        );
        store
            .acknowledge_maintenance_media_page(&db.pool, &fence, &reservation, ack, &ids)
            .await
            .unwrap();
        store
            .acknowledge_maintenance_media_page(&db.pool, &fence, &reservation, ack, &ids)
            .await
            .unwrap();
        acked += ids.len();
    }
    assert_eq!(acked, 21);
    assert!(
        store
            .audit_maintenance_deletion_coverage_page(&db.pool, &fence, &reservation)
            .await
            .is_err(),
        "assertion-only acknowledgements are not verified deletion evidence"
    );
    let coverage_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_media_coverage_audits WHERE reservation_id=$1",
    )
    .bind(reservation.id())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(coverage_rows, 0, "failed audit must roll back its cursor");
    assert!(sqlx::query("INSERT INTO admin_media_snapshot_entries(reservation_id,asset_id,relative_path,byte_hash,byte_size) VALUES($1,$2,'unexpected',$3,1)")
        .bind(reservation.id()).bind(Uuid::now_v7()).bind("a".repeat(64)).execute(&db.pool).await.is_err(),
        "frozen closure cannot acquire an unaudited member");
    assert!(
        store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .is_err(),
        "acknowledged filesystem work must remain durable"
    );

    let objects = store
        .pending_maintenance_object_page(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    assert!(objects.len() <= 16);
    assert!(
        objects
            .iter()
            .any(|object| object.kind == "staging_directory")
    );
    assert!(
        objects
            .iter()
            .any(|object| object.kind == "canonical_directory")
    );
    assert_eq!(
        objects
            .iter()
            .filter(|object| object.kind == "artwork" && object.object_key == exclusive_image)
            .count(),
        1
    );
    assert!(
        !objects
            .iter()
            .any(|object| object.kind == "artwork" && object.object_key == shared_image)
    );
    let keys: Vec<_> = objects
        .iter()
        .map(|object| melodee_persistence::AdminMediaObjectKey {
            kind: object.kind.clone(),
            object_id: object.object_id,
        })
        .collect();
    let object_ack = Uuid::now_v7();
    store
        .acknowledge_maintenance_object_page(&db.pool, &fence, &reservation, object_ack, &keys)
        .await
        .unwrap();
    store
        .acknowledge_maintenance_object_page(&db.pool, &fence, &reservation, object_ack, &keys)
        .await
        .unwrap();
    assert!(
        store
            .pending_maintenance_object_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM media_assets WHERE id=$1",
            other_asset
        )
        .await,
        1
    );
    assert!(
        store
            .finish_admin_media_reservation(&db.pool, &reservation)
            .await
            .is_err(),
        "snapshot metadata must survive until safe completion"
    );
    db.drop().await;
}

#[tokio::test]
async fn maintenance_track_closure_excludes_unselected_shared_assets_and_directories() {
    let db = TestDb::create("maintenance_track_closure").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &catalog, 520, "Track Scope", "Track Artist").await;
    let selected: (Uuid, Uuid) = sqlx::query_as(
        "SELECT id,public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    seed_unobserved_track(&db, release.internal_id).await;
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Track,
            &[selected.1],
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Track,
            &[selected.1],
        )
        .await
        .unwrap();
    assert!(
        store
            .bind_maintenance_catalog_targets(
                &db.pool,
                &fence,
                &reservation,
                melodee_persistence::AdminCatalogDeleteKind::Release,
                &[release.public_id]
            )
            .await
            .is_err()
    );
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    for _ in 0..10 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            break;
        }
    }
    assert!(
        store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .pending_maintenance_object_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
            .is_empty()
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM admin_media_snapshots WHERE reservation_id=$1")
            .bind(reservation.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(state, "frozen");
    assert!(
        store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
    );
    store
        .finish_admin_media_reservation(&db.pool, &reservation)
        .await
        .unwrap();

    db.drop().await;
}

#[tokio::test]
async fn maintenance_artist_closure_rejects_changed_ownership_before_freeze() {
    let db = TestDb::create("maintenance_artist_scope_change").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let selected = seed_release(&db, &catalog, 521, "Artist Scope", "Owner Artist").await;
    let other = seed_release(&db, &catalog, 522, "Other Scope", "Other Artist").await;
    let (artist,public):(Uuid,Uuid)=sqlx::query_as("SELECT a.id,a.public_api_id FROM artists a JOIN release_artists r ON r.artist_id=a.id WHERE r.release_id=$1 AND r.role='primary' LIMIT 1").bind(selected.internal_id).fetch_one(&db.pool).await.unwrap();
    let other_artist: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 AND role='primary' LIMIT 1",
    )
    .bind(other.internal_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let image = format!("{}-5", "c".repeat(64));
    sqlx::query("INSERT INTO asset_files(asset_key,size_bytes) VALUES($1,5)")
        .bind(&image)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO catalog_entity_images(entity_type,artist_id,image_mime_type,asset_key,uploaded_by,created_at,updated_at) VALUES('artist',$1,'image/jpeg',$2,'test',clock_timestamp(),clock_timestamp())").bind(artist).bind(&image).execute(&db.pool).await.unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[selected.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Artist,
            &[public],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    // Simulate an out-of-band writer violating maintenance after target binding.
    sqlx::query("INSERT INTO release_artists(id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) VALUES($1,$2,$3,'primary',2,'other','',clock_timestamp())").bind(Uuid::now_v7()).bind(selected.internal_id).bind(other_artist).execute(&db.pool).await.unwrap();
    let mut rejected = false;
    for _ in 0..20 {
        match store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
        {
            Ok(true) => panic!("changed ownership must not freeze"),
            Ok(false) => {}
            Err(_) => {
                rejected = true;
                break;
            }
        }
    }
    assert!(rejected);
    let captured:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_media_snapshot_objects WHERE reservation_id=$1 AND kind='artwork' AND object_key=$2)").bind(reservation.id()).bind(&image).fetch_one(&db.pool).await.unwrap();
    assert!(captured);

    assert!(
        store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM admin_media_snapshots WHERE reservation_id=$1")
            .bind(reservation.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(state, "capturing");
    db.drop().await;
}

#[tokio::test]
async fn maintenance_snapshot_discard_resumes_in_bounded_pages_and_releases_roots() {
    let db = TestDb::create("maintenance_discard_roots").await;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &CatalogStore, 529, "Discard Scope", "Discard Artist").await;
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let mut reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[release.public_id],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    let mut roots: Vec<melodee_persistence::AdminMediaRootExpectation> =
        ["work", "stage", "canonical", "asset", "lock"]
            .iter()
            .enumerate()
            .map(|(i, role)| melodee_persistence::AdminMediaRootExpectation {
                role: (*role).to_owned(),
                source_library_id: None,
                path_format: "unix-hex-v1".to_owned(),
                path_hex: format!("2f{:02x}", i + 1),
                generation: format!("unix:1:{}", i + 1),
                mount_id: "0000000000000001".to_owned(),
                filesystem_magic: "000000000000ef53".to_owned(),
            })
            .collect();
    let mut sources: Vec<Option<Uuid>> = sqlx::query_scalar("SELECT DISTINCT source_library_id FROM admin_media_reservation_items WHERE reservation_id=$1").bind(reservation.id()).fetch_all(&db.pool).await.unwrap();
    sources.extend((0..20).map(|_| Some(Uuid::now_v7())));
    roots.extend(sources.into_iter().enumerate().map(|(i, source)| {
        melodee_persistence::AdminMediaRootExpectation {
            role: "source".to_owned(),
            source_library_id: source,
            path_format: "unix-hex-v1".to_owned(),
            path_hex: format!("2f{:02x}", i + 32),
            generation: format!("unix:2:{}", i + 1),
            mount_id: "0000000000000002".to_owned(),
            filesystem_magic: "000000000000ef53".to_owned(),
        }
    }));
    store
        .bind_maintenance_media_roots(&db.pool, &fence, &reservation, Uuid::now_v7(), &roots)
        .await
        .unwrap();
    for _ in 0..20 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            break;
        }
    }
    assert!(
        sqlx::query("DELETE FROM admin_media_snapshot_entries WHERE reservation_id=$1")
            .bind(reservation.id())
            .execute(&db.pool)
            .await
            .is_err(),
        "frozen evidence cannot disappear outside fenced discard"
    );
    assert!(
        !store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM admin_media_snapshots WHERE reservation_id=$1")
            .bind(reservation.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(state, "discarding");
    assert!(
        store
            .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    assert!(
        store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    assert!(
        store
            .pending_maintenance_media_page(&db.pool, &fence, &reservation)
            .await
            .is_err()
    );
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(reservation.id()).execute(&db.pool).await.unwrap();
    assert!(
        store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .is_err(),
        "expired epoch must not advance cleanup"
    );
    reservation = store
        .reclaim_admin_media_reservation(
            &db.pool,
            reservation.id(),
            "operator",
            time::Duration::minutes(1),
        )
        .await
        .unwrap();
    let mut complete = false;
    for _ in 0..20 {
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_media_root_entries e JOIN admin_media_root_sets s ON s.revision=e.revision WHERE s.reservation_id=$1").bind(reservation.id()).fetch_one(&db.pool).await.unwrap();
        complete = store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap();
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_media_root_entries e JOIN admin_media_root_sets s ON s.revision=e.revision WHERE s.reservation_id=$1").bind(reservation.id()).fetch_one(&db.pool).await.unwrap();
        assert!((0..=16).contains(&(before - after)));
        if complete {
            break;
        }
    }
    assert!(complete);
    assert!(
        store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
    );
    let budget: (i64, i64, i64) = sqlx::query_as(
        "SELECT snapshots,entries,bytes FROM admin_media_snapshot_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(budget, (0, 0, 0));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admin_media_root_sets WHERE reservation_id=$1")
            .bind(reservation.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    db.drop().await;
}

// Synthetic database records exercise bookkeeping only: they do not claim
// that any filesystem object was verified or deleted.
#[tokio::test]
async fn maintenance_completed_proof_cleanup_is_bounded_and_replayable() {
    let db = TestDb::create("maintenance_completion_cleanup").await;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &CatalogStore, 528, "Cleanup Scope", "Cleanup Artist").await;
    let track_public: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(release.internal_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Track,
            &[track_public],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    let mut roots: Vec<melodee_persistence::AdminMediaRootExpectation> =
        ["work", "stage", "canonical", "asset", "lock"]
            .iter()
            .enumerate()
            .map(|(i, role)| melodee_persistence::AdminMediaRootExpectation {
                role: (*role).to_owned(),
                source_library_id: None,
                path_format: "unix-hex-v1".to_owned(),
                path_hex: format!("2f{:02x}", i + 1),
                generation: format!("unix:1:{}", i + 1),
                mount_id: "0000000000000001".to_owned(),
                filesystem_magic: "000000000000ef53".to_owned(),
            })
            .collect();
    let sources:Vec<Option<Uuid>>=sqlx::query_scalar("SELECT DISTINCT source_library_id FROM admin_media_reservation_items WHERE reservation_id=$1").bind(reservation.id()).fetch_all(&db.pool).await.unwrap();
    roots.extend(sources.into_iter().enumerate().map(|(i, id)| {
        melodee_persistence::AdminMediaRootExpectation {
            role: "source".to_owned(),
            source_library_id: id,
            path_format: "unix-hex-v1".to_owned(),
            path_hex: format!("2f{:02x}", i + 32),
            generation: format!("unix:2:{}", i + 1),
            mount_id: "0000000000000002".to_owned(),
            filesystem_magic: "000000000000ef53".to_owned(),
        }
    }));
    let root_revision = Uuid::now_v7();
    store
        .bind_maintenance_media_roots(&db.pool, &fence, &reservation, root_revision, &roots)
        .await
        .unwrap();
    for _ in 0..20 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            break;
        }
    }
    for kind in [
        melodee_persistence::AdminDeletionPageKind::Media,
        melodee_persistence::AdminDeletionPageKind::Object,
    ] {
        loop {
            let id = Uuid::now_v7();
            let Some(intent) = store
                .prepare_maintenance_deletion_page(&db.pool, &fence, &reservation, kind, id)
                .await
                .unwrap()
            else {
                break;
            };
            // Deliberately synthetic: production has no API that can make a
            // verified intent from a caller's assertion.
            let mut quarantine_ids = Vec::new();
            if kind == melodee_persistence::AdminDeletionPageKind::Media {
                for ordinal in 1..=intent.entries().len() {
                    let qid = Uuid::now_v7();
                    let request = melodee_persistence::AdminQuarantineRequest {
                        ordinal: u16::try_from(ordinal).unwrap(),
                        id: qid,
                        observation: melodee_persistence::AdminQuarantineObservation {
                            quarantine_parent_relative_hex: "71756172616e74696e652fff",
                            source_generation: "unix:1:11",
                            source_parent_generation: "unix:1:12",
                            quarantine_parent_generation: "unix:1:13",
                            mount_id: "0000000000000001",
                        },
                    };
                    store
                        .prepare_admin_quarantine_intent(
                            &db.pool,
                            &fence,
                            &reservation,
                            &intent,
                            &request,
                        )
                        .await
                        .unwrap();
                    quarantine_ids.push(qid);
                }
            }
            sqlx::query("UPDATE admin_media_page_intents SET state='verified' WHERE id=$1")
                .bind(id)
                .execute(&db.pool)
                .await
                .unwrap();
            for entry in intent.entries() {
                if kind == melodee_persistence::AdminDeletionPageKind::Media {
                    sqlx::query("UPDATE admin_media_snapshot_entries SET acknowledgement=$3 WHERE reservation_id=$1 AND asset_id=$2").bind(reservation.id()).bind(entry.object_id).bind(id).execute(&db.pool).await.unwrap();
                } else {
                    sqlx::query("UPDATE admin_media_snapshot_objects SET acknowledgement=$4 WHERE reservation_id=$1 AND kind=$2 AND object_id=$3").bind(reservation.id()).bind(&entry.kind).bind(entry.object_id).bind(id).execute(&db.pool).await.unwrap();
                }
            }
            if !quarantine_ids.is_empty() {
                assert!(
                    store
                        .audit_maintenance_deletion_coverage_page(&db.pool, &fence, &reservation)
                        .await
                        .is_err(),
                    "a verified page without verified individual quarantine records cannot pass coverage"
                );
                for qid in quarantine_ids {
                    sqlx::query(
                        "UPDATE admin_media_quarantine_intents SET state='moved' WHERE id=$1",
                    )
                    .bind(qid)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                    sqlx::query(
                        "UPDATE admin_media_quarantine_intents SET state='verified' WHERE id=$1",
                    )
                    .bind(qid)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                }
            }
        }
    }
    let mut audited = false;
    for _ in 0..20 {
        if store
            .audit_maintenance_deletion_coverage_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            audited = true;
            break;
        }
    }
    assert!(audited);
    let revision: Uuid =
        sqlx::query_scalar("SELECT revision FROM admin_media_snapshots WHERE reservation_id=$1")
            .bind(reservation.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let completion = Uuid::now_v7();
    sqlx::query("INSERT INTO admin_media_completion_receipts(reservation_id,completion_id,snapshot_revision,root_set_revision,kind,actor,reason,cleanup_state) VALUES($1,$2,$3,$4,'track','test','synthetic only','pending')").bind(reservation.id()).bind(completion).bind(revision).bind(root_revision).execute(&db.pool).await.unwrap();
    assert!(
        store
            .cleanup_completed_admin_media_page(&db.pool, &fence, &reservation)
            .await
            .is_err(),
        "live scope cannot be cleaned"
    );
    sqlx::query("DELETE FROM admin_media_reservation_items WHERE reservation_id=$1")
        .bind(reservation.id())
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM admin_media_reservation_releases WHERE reservation_id=$1")
        .bind(reservation.id())
        .execute(&db.pool)
        .await
        .unwrap();
    let mut done = false;
    for _ in 0..40 {
        if store
            .cleanup_completed_admin_media_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            done = true;
            break;
        }
    }
    assert!(done);
    assert!(
        store
            .cleanup_completed_admin_media_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
    );
    let state: String = sqlx::query_scalar(
        "SELECT cleanup_state FROM admin_media_completion_receipts WHERE reservation_id=$1",
    )
    .bind(reservation.id())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(state, "done");
    assert!(
        sqlx::query("DELETE FROM admin_media_completion_receipts WHERE reservation_id=$1")
            .bind(reservation.id())
            .execute(&db.pool)
            .await
            .is_err()
    );
    let snapshot_budget: (i64, i64, i64) = sqlx::query_as(
        "SELECT snapshots,entries,bytes FROM admin_media_snapshot_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let intent_budget: (i64, i64, i64) = sqlx::query_as(
        "SELECT intents,entries,bytes FROM admin_media_page_intent_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(snapshot_budget, (0, 0, 0));
    assert_eq!(intent_budget, (0, 0, 0));
    let quarantine_budget: (i64, i64) =
        sqlx::query_as("SELECT entries,bytes FROM admin_media_quarantine_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(quarantine_budget, (0, 0));
    let root = melodee_persistence::AdminArchiveRoot {
        path_format: "unix-hex-v1".to_owned(),
        path_hex: "2f61726368697665".to_owned(),
        generation: "unix:5:6".to_owned(),
        mount_id: "0000000000000005".to_owned(),
        filesystem_magic: "000000000000ef53".to_owned(),
    };
    let archive = store
        .prepare_admin_completion_archive(&db.pool, &fence, &reservation, &root)
        .await
        .unwrap();
    assert_eq!(
        store
            .revalidate_admin_completion_archive(&db.pool, &fence, &reservation, &archive)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        archive.filename(),
        format!("admin-tombstone-{}.v1", completion.simple())
    );
    assert!(
        archive
            .bytes()
            .starts_with(b"melodee-admin-deletion-tombstone-v1\0")
    );
    assert!(archive.bytes().len() <= 8192);
    let replay = store
        .prepare_admin_completion_archive(&db.pool, &fence, &reservation, &root)
        .await
        .unwrap();
    assert_eq!(archive.bytes(), replay.bytes());
    assert_eq!(archive.digest(), replay.digest());
    assert_eq!(
        archive.lock_name(),
        format!("archive-{}.lock", completion.simple())
    );
    store
        .acknowledge_admin_archive_lock(&db.pool, &fence, &reservation, &archive, "unix:5:7")
        .await
        .unwrap();
    assert_eq!(
        store
            .revalidate_admin_completion_archive(&db.pool, &fence, &reservation, &archive)
            .await
            .unwrap()
            .as_deref(),
        Some("unix:5:7")
    );
    store
        .acknowledge_admin_archive_lock(&db.pool, &fence, &reservation, &archive, "unix:5:7")
        .await
        .unwrap();
    assert!(
        store
            .acknowledge_admin_archive_lock(&db.pool, &fence, &reservation, &archive, "unix:5:8")
            .await
            .is_err()
    );
    let mut replacement = root.clone();
    replacement.generation = "unix:5:999".to_owned();
    assert!(
        store
            .prepare_admin_completion_archive(&db.pool, &fence, &reservation, &replacement)
            .await
            .is_err()
    );
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(reservation.id()).execute(&db.pool).await.unwrap();
    assert!(
        store
            .prepare_admin_completion_archive(&db.pool, &fence, &reservation, &root)
            .await
            .is_err()
    );
    assert!(
        store
            .revalidate_admin_completion_archive(&db.pool, &fence, &reservation, &archive)
            .await
            .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn maintenance_page_intent_is_exact_immutable_and_never_adopted_by_new_epoch() {
    let db = TestDb::create("maintenance_page_intent").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &catalog, 530, "Intent Scope", "Intent Artist").await;
    let other = seed_release(
        &db,
        &catalog,
        531,
        "Other Intent Scope",
        "Other Intent Artist",
    )
    .await;
    let outside_track: Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1")
            .bind(other.internal_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let old = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &old,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[release.public_id],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &old)
        .await
        .unwrap();
    let root_revision = Uuid::now_v7();
    let mut roots: Vec<melodee_persistence::AdminMediaRootExpectation> =
        ["work", "stage", "canonical", "asset", "lock"]
            .iter()
            .enumerate()
            .map(
                |(index, role)| melodee_persistence::AdminMediaRootExpectation {
                    role: (*role).to_owned(),
                    source_library_id: None,
                    path_format: "unix-hex-v1".to_owned(),
                    path_hex: format!("2f{:02x}", index + 1),
                    generation: format!("unix:1:{}", index + 1),
                    mount_id: "0000000000000001".to_owned(),
                    filesystem_magic: "000000000000ef53".to_owned(),
                },
            )
            .collect();
    let source_ids: Vec<Option<Uuid>> = sqlx::query_scalar(
        "SELECT DISTINCT source_library_id FROM admin_media_reservation_items WHERE reservation_id=$1",
    )
    .bind(old.id())
    .fetch_all(&db.pool)
    .await
    .unwrap();
    roots.extend(source_ids.into_iter().enumerate().map(|(index, source)| {
        melodee_persistence::AdminMediaRootExpectation {
            role: "source".to_owned(),
            source_library_id: source,
            path_format: "unix-hex-v1".to_owned(),
            path_hex: format!("2f{:02x}", index + 32),
            generation: format!("unix:2:{}", index + 1),
            mount_id: "0000000000000002".to_owned(),
            filesystem_magic: "000000000000ef53".to_owned(),
        }
    }));
    assert!(roots.iter().any(|root| root.role == "source"));
    let missing_source: Vec<_> = roots
        .iter()
        .filter(|root| root.role != "source")
        .cloned()
        .collect();
    assert!(
        store
            .bind_maintenance_media_roots(&db.pool, &fence, &old, root_revision, &missing_source)
            .await
            .is_err(),
        "every reserved source must have an exact configured root binding"
    );
    store
        .bind_maintenance_media_roots(&db.pool, &fence, &old, root_revision, &roots)
        .await
        .unwrap();
    store
        .bind_maintenance_media_roots(&db.pool, &fence, &old, root_revision, &roots)
        .await
        .unwrap();
    let mut replacement = roots.clone();
    replacement[2].generation = "unix:1:999".to_owned();
    assert!(
        store
            .bind_maintenance_media_roots(&db.pool, &fence, &old, root_revision, &replacement)
            .await
            .is_err(),
        "a same-content clone or replaced root must not replay as the original binding"
    );
    let mut frozen = false;
    for _ in 0..20 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &old)
            .await
            .unwrap()
        {
            frozen = true;
            break;
        }
    }
    assert!(frozen);
    let nonce = Uuid::now_v7();
    let intent = store
        .prepare_maintenance_deletion_page(
            &db.pool,
            &fence,
            &old,
            melodee_persistence::AdminDeletionPageKind::Media,
            nonce,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.entries().len(), 1);
    assert_eq!(intent.root_set_revision(), Some(root_revision));
    let observation = melodee_persistence::AdminQuarantineObservation {
        quarantine_parent_relative_hex: "71756172616e74696e652fff",
        source_generation: "unix:1:101",
        source_parent_generation: "unix:1:102",
        quarantine_parent_generation: "unix:1:103",
        mount_id: "0000000000000001",
    };
    let quarantine_id = Uuid::now_v7();
    let request = melodee_persistence::AdminQuarantineRequest {
        ordinal: 1,
        id: quarantine_id,
        observation,
    };
    let quarantine = store
        .prepare_admin_quarantine_intent(&db.pool, &fence, &old, &intent, &request)
        .await
        .unwrap();
    assert_eq!(quarantine.id(), quarantine_id);
    assert_eq!(
        quarantine.leaf_name(),
        format!("quarantine-{}.entry", quarantine_id.simple())
    );
    let binding = store
        .read_admin_quarantine_binding(&db.pool, &fence, &old, &intent, quarantine_id)
        .await
        .unwrap();
    assert_eq!(binding.quarantine_root_role, "canonical");
    assert_eq!(
        binding.quarantine_parent_relative_hex,
        "71756172616e74696e652fff"
    );
    assert_eq!(binding.state, "intent");
    store
        .prepare_admin_quarantine_intent(&db.pool, &fence, &old, &intent, &request)
        .await
        .unwrap();
    let replacement = melodee_persistence::AdminQuarantineObservation {
        source_generation: "unix:1:999",
        ..observation
    };
    let changed_parent = melodee_persistence::AdminQuarantineObservation {
        quarantine_parent_relative_hex: "71756172616e74696e652ffe",
        ..observation
    };
    assert!(
        store
            .prepare_admin_quarantine_intent(
                &db.pool,
                &fence,
                &old,
                &intent,
                &melodee_persistence::AdminQuarantineRequest {
                    ordinal: 1,
                    id: quarantine_id,
                    observation: changed_parent
                }
            )
            .await
            .is_err()
    );
    assert!(
        store
            .prepare_admin_quarantine_intent(
                &db.pool,
                &fence,
                &old,
                &intent,
                &melodee_persistence::AdminQuarantineRequest {
                    ordinal: 1,
                    id: quarantine_id,
                    observation: replacement
                }
            )
            .await
            .is_err()
    );
    let charged: (i64, i64) =
        sqlx::query_as("SELECT entries,bytes FROM admin_media_quarantine_budget WHERE singleton")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(charged, (1, 310));
    assert!(
        store
            .prepare_admin_quarantine_intent(
                &db.pool,
                &fence,
                &old,
                &intent,
                &melodee_persistence::AdminQuarantineRequest {
                    ordinal: 1,
                    id: Uuid::now_v7(),
                    observation
                }
            )
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO admin_media_page_intent_entries(intent_id,ordinal,kind,object_id,object_key) VALUES($1,2,'media',$2,'unexpected')")
        .bind(nonce).bind(Uuid::now_v7()).execute(&db.pool).await.is_err(),
        "a completed intent page cannot gain a new member");
    assert_eq!(
        store
            .read_maintenance_intent_roots(&db.pool, &fence, &old, &intent)
            .await
            .unwrap(),
        roots
    );
    let revision: Uuid =
        sqlx::query_scalar("SELECT revision FROM admin_media_snapshots WHERE reservation_id=$1")
            .bind(old.id())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(intent.snapshot_revision(), revision);
    assert!(
        sqlx::query(
            "UPDATE admin_media_snapshots SET revision=gen_random_uuid() WHERE reservation_id=$1"
        )
        .bind(old.id())
        .execute(&db.pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("UPDATE admin_media_snapshots SET state='capturing' WHERE reservation_id=$1")
            .bind(old.id())
            .execute(&db.pool)
            .await
            .is_err()
    );

    let e = &intent.entries()[0];
    let expected: (String, String, i64) = sqlx::query_as(
        "SELECT canonical_relative_path,byte_hash,byte_size FROM media_assets WHERE id=$1",
    )
    .bind(e.object_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        (&e.object_key, e.byte_hash.as_deref(), e.byte_size),
        (&expected.0, Some(expected.1.as_str()), Some(expected.2))
    );
    let replay = store
        .prepare_maintenance_deletion_page(
            &db.pool,
            &fence,
            &old,
            melodee_persistence::AdminDeletionPageKind::Media,
            nonce,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.entries(), intent.entries());
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
        .await
        .unwrap();
    // An out-of-band new reference outside the reserved Release invalidates
    // the formerly exclusive asset before any filesystem work can proceed.
    sqlx::query("INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at) VALUES($1,$2,'alternate',clock_timestamp())").bind(outside_track).bind(e.object_id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM track_media_assets WHERE track_id=$1 AND media_asset_id=$2 AND purpose='alternate'").bind(outside_track).bind(e.object_id).execute(&db.pool).await.unwrap();
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
        .await
        .unwrap();

    // An out-of-band metadata mutation during supposed maintenance is not
    // allowed to reuse the previous intent, even if the byte size is unchanged.
    sqlx::query("UPDATE media_assets SET byte_hash=repeat('f',64) WHERE id=$1")
        .bind(e.object_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
            .await
            .is_err()
    );
    sqlx::query("UPDATE media_assets SET byte_hash=$2 WHERE id=$1")
        .bind(e.object_id)
        .bind(&expected.1)
        .execute(&db.pool)
        .await
        .unwrap();
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
        .await
        .unwrap();

    assert!(
        store
            .prepare_maintenance_deletion_page(
                &db.pool,
                &fence,
                &old,
                melodee_persistence::AdminDeletionPageKind::Object,
                Uuid::now_v7()
            )
            .await
            .is_err()
    );
    assert!(
        store
            .acknowledge_maintenance_media_page(
                &db.pool,
                &fence,
                &old,
                Uuid::now_v7(),
                &[e.object_id]
            )
            .await
            .is_err()
    );
    assert!(
        store
            .discard_maintenance_media_snapshot_page(&db.pool, &fence, &old)
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE admin_media_snapshot_entries SET relative_path='different' WHERE reservation_id=$1").bind(old.id()).execute(&db.pool).await.is_err());
    assert!(
        sqlx::query(
            "UPDATE admin_media_page_intent_entries SET object_key='different' WHERE intent_id=$1"
        )
        .bind(nonce)
        .execute(&db.pool)
        .await
        .is_err()
    );
    store
        .quarantine_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
        .await
        .unwrap();
    assert!(
        store
            .revalidate_maintenance_deletion_page(&db.pool, &fence, &old, &intent)
            .await
            .is_err()
    );
    sqlx::query("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(old.id()).execute(&db.pool).await.unwrap();
    let current = store
        .reclaim_admin_media_reservation(&db.pool, old.id(), "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    assert!(
        store
            .read_admin_quarantine_binding(&db.pool, &fence, &current, &intent, quarantine_id)
            .await
            .is_err(),
        "a new epoch cannot adopt a prior move binding"
    );
    let recovery = store
        .inspect_admin_quarantine_recovery(&db.pool, &fence, &current, quarantine_id)
        .await
        .unwrap();
    assert_eq!(recovery.page_id, intent.id());
    assert_eq!(
        recovery.binding.quarantine_parent_relative_hex,
        "71756172616e74696e652fff"
    );
    assert_eq!(recovery.binding.state, "intent");
    assert_eq!(recovery.entry, intent.entries()[0]);
    assert_eq!(recovery.roots, roots);
    assert!(
        store
            .inspect_admin_quarantine_recovery(&db.pool, &fence, &old, quarantine_id)
            .await
            .is_err()
    );
    assert!(
        store
            .prepare_admin_quarantine_intent(&db.pool, &fence, &current, &intent, &request)
            .await
            .is_err(),
        "read-only recovery cannot adopt the old epoch"
    );
    assert!(
        store
            .prepare_maintenance_deletion_page(
                &db.pool,
                &fence,
                &current,
                melodee_persistence::AdminDeletionPageKind::Media,
                nonce
            )
            .await
            .is_err()
    );
    assert!(
        store
            .prepare_maintenance_deletion_page(
                &db.pool,
                &fence,
                &current,
                melodee_persistence::AdminDeletionPageKind::Media,
                Uuid::now_v7()
            )
            .await
            .is_err()
    );
    let other_guard = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[other.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE work_item_lifecycle_maintenance SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE singleton").execute(&db.pool).await.unwrap();
    let fresh_fence = store
        .reclaim_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    let attestation = melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "recovery test",
        backup_reference: "test-backup",
        stop_all_writers: true,
        inspection_complete: true,
    };
    assert!(
        store
            .confirm_lifecycle_quiescence(&db.pool, &fresh_fence, &attestation)
            .await
            .is_err()
    );
    assert!(
        store
            .confirm_lifecycle_recovery_quiescence(&db.pool, &fresh_fence, &current, &attestation)
            .await
            .is_err(),
        "a different reservation must remain a blocker"
    );
    store
        .finish_admin_media_reservation(&db.pool, &other_guard)
        .await
        .unwrap();
    assert!(
        store
            .confirm_lifecycle_recovery_quiescence(&db.pool, &fresh_fence, &old, &attestation)
            .await
            .is_err(),
        "a stale reservation epoch cannot attest recovery"
    );
    let incomplete = melodee_persistence::LifecycleQuiescenceAttestation {
        reason: "recovery test",
        backup_reference: "test-backup",
        stop_all_writers: false,
        inspection_complete: true,
    };
    assert!(
        store
            .confirm_lifecycle_recovery_quiescence(&db.pool, &fresh_fence, &current, &incomplete)
            .await
            .is_err()
    );
    store
        .confirm_lifecycle_recovery_quiescence(&db.pool, &fresh_fence, &current, &attestation)
        .await
        .unwrap();
    store
        .confirm_lifecycle_recovery_quiescence(&db.pool, &fresh_fence, &current, &attestation)
        .await
        .unwrap();
    let recovered = store
        .inspect_admin_quarantine_recovery(&db.pool, &fresh_fence, &current, quarantine_id)
        .await
        .unwrap();
    assert_eq!(
        recovered.binding.quarantine_parent_relative_hex,
        "71756172616e74696e652fff"
    );
    assert!(
        store
            .prepare_admin_quarantine_intent(&db.pool, &fresh_fence, &current, &intent, &request)
            .await
            .is_err(),
        "attestation and lookup do not authorize adoption"
    );
    // Recovery quiescence binds the fence to exactly one reservation and never
    // claims generic quiescence, so no unrelated maintenance path may proceed.
    let fenced: (bool, Option<Uuid>) = sqlx::query_as(
        "SELECT quiesced,recovery_reservation_id FROM work_item_lifecycle_maintenance WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(fenced, (false, Some(current.id())));
    assert!(
        store
            .next_lifecycle_provision_item(&db.pool, &fresh_fence)
            .await
            .is_err(),
        "recovery attestation must not authorize unrelated maintenance"
    );
    assert!(
        store
            .prepare_maintenance_deletion_page(
                &db.pool,
                &fresh_fence,
                &current,
                melodee_persistence::AdminDeletionPageKind::Media,
                Uuid::now_v7()
            )
            .await
            .is_err(),
        "recovery attestation must not authorize new page creation"
    );
    assert!(
        store
            .acknowledge_maintenance_media_page(
                &db.pool,
                &fresh_fence,
                &current,
                Uuid::now_v7(),
                &[e.object_id]
            )
            .await
            .is_err(),
        "recovery attestation must not authorize acknowledgement"
    );
    assert!(
        store
            .quarantine_maintenance_deletion_page(&db.pool, &fresh_fence, &current, &intent)
            .await
            .is_err(),
        "recovery attestation must not authorize page unlink disposition changes"
    );
    assert!(
        store
            .begin_maintenance_admin_media_reservation(
                &db.pool,
                &[],
                &[other.internal_id],
                "operator",
                time::Duration::minutes(1),
                &fresh_fence
            )
            .await
            .is_err(),
        "recovery attestation must not authorize a second reservation"
    );
    let (intents, entries): (i64, i64) = sqlx::query_as(
        "SELECT intents,entries FROM admin_media_page_intent_budget WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((intents, entries), (1, 1));
    db.drop().await;
}

async fn blocked_maintenance_page_intent(cancel: bool) {
    let db = TestDb::create("maintenance_intent_stall").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &catalog, 540, "Intent Stall", "Stall Artist").await;
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[release.public_id],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    let mut frozen = false;
    for _ in 0..20 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            frozen = true;
            break;
        }
    }
    assert!(frozen);
    let expiry:OffsetDateTime=sqlx::query_scalar("UPDATE admin_media_reservations SET lease_expires_at=clock_timestamp()+interval '2 seconds' WHERE id=$1 RETURNING lease_expires_at").bind(reservation.id()).fetch_one(&db.pool).await.unwrap();
    let key = (Uuid::now_v7().as_u128() & i64::MAX as u128) as i64;
    let mut blocker = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let trigger = format!(
        "CREATE FUNCTION test_page_intent_stall() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER test_page_intent_stall BEFORE INSERT ON admin_media_page_intent_entries FOR EACH ROW EXECUTE FUNCTION test_page_intent_stall()"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(trigger))
        .execute(&db.pool)
        .await
        .unwrap();
    let pool = db.pool.clone();
    let worker_fence = fence.clone();
    let worker_reservation = reservation.clone();
    let task = tokio::spawn(async move {
        melodee_persistence::WorkItemStore
            .prepare_maintenance_deletion_page(
                &pool,
                &worker_fence,
                &worker_reservation,
                melodee_persistence::AdminDeletionPageKind::Media,
                Uuid::now_v7(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5),async { loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'INSERT INTO admin_media_page_intent_entries%')").fetch_one(&db.pool).await.unwrap();
        if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.expect("intent reached insertion stall");
    if cancel {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>$1")
                    .bind(expiry)
                    .fetch_one(&db.pool)
                    .await
                    .unwrap();
                if expired {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err());
    }
    if cancel {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    let mut verify = db.pool.begin().await.unwrap();
    let (intents,entries,bytes,rows):(i64,i64,i64,i64)=sqlx::query_as("SELECT intents,entries,bytes,(SELECT count(*) FROM admin_media_page_intents) FROM admin_media_page_intent_budget WHERE singleton FOR UPDATE").fetch_one(&mut *verify).await.unwrap();
    assert_eq!((intents, entries, bytes, rows), (0, 0, 0, 0));
    verify.rollback().await.unwrap();
    drop(blocker);
    db.drop().await;
}
#[tokio::test]
async fn expired_maintenance_page_intent_rolls_back() {
    blocked_maintenance_page_intent(false).await;
}
#[tokio::test]
async fn cancelled_maintenance_page_intent_rolls_back() {
    blocked_maintenance_page_intent(true).await;
}

/// Explicit opt-in load: this measures the provisional persistence API, not a
/// filesystem operation, full import, 600K catalog, or production qualification.
#[tokio::test]
#[ignore = "explicit owned-database 10K closure load; run serially in release mode"]
async fn maintenance_release_media_closure_10000_tracks_measurement() {
    let db = TestDb::create("maintenance_closure_10k").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &catalog, 550, "10K Closure", "Closure Artist").await;
    let (original,original_asset):(Uuid,Uuid)=sqlx::query_as("SELECT id,playable_media_asset_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1").bind(release.internal_id).fetch_one(&db.pool).await.unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    for start in (2_i32..=10_000).step_by(1_000) {
        let end = (start + 999).min(10_000);
        let ordinals: Vec<i32> = (start..=end).collect();
        let tracks: Vec<Uuid> = ordinals.iter().map(|_| Uuid::now_v7()).collect();
        let publics: Vec<Uuid> = ordinals.iter().map(|_| Uuid::now_v7()).collect();
        let assets: Vec<Uuid> = ordinals.iter().map(|_| Uuid::now_v7()).collect();
        let paths: Vec<String> = assets
            .iter()
            .map(|id| format!("synthetic/{id}.mp3"))
            .collect();
        sqlx::query("INSERT INTO media_assets(id,canonical_relative_path,byte_hash,byte_size,mime_type,container,codec,sample_rate_hz,channels,bitrate_bps,duration_ms,created_at) SELECT x.id,x.path,a.byte_hash,a.byte_size,a.mime_type,a.container,a.codec,a.sample_rate_hz,a.channels,a.bitrate_bps,a.duration_ms,a.created_at FROM unnest($1::uuid[],$2::text[]) x(id,path) CROSS JOIN media_assets a WHERE a.id=$3")
            .bind(&assets).bind(&paths).bind(original_asset).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO tracks(id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,normalized_title,sort_title,created_at,updated_at) SELECT x.id,x.public_id,t.release_id,t.audio_identity_id,x.asset_id,x.ordinal,'Synthetic','synthetic','Synthetic',t.created_at,t.updated_at FROM unnest($1::uuid[],$2::uuid[],$3::uuid[],$4::integer[]) x(id,public_id,asset_id,ordinal) CROSS JOIN tracks t WHERE t.id=$5")
            .bind(&tracks).bind(&publics).bind(&assets).bind(&ordinals).bind(original).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at) SELECT x.track_id,x.asset_id,'playable',clock_timestamp() FROM unnest($1::uuid[],$2::uuid[]) x(track_id,asset_id)")
            .bind(&tracks).bind(&assets).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(release.internal_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 10_000);
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::hours(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "owned disposable benchmark",
                backup_reference: "disposable-fixture",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::hours(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[release.public_id],
        )
        .await
        .unwrap();
    let started = std::time::Instant::now();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    let mut pages = 0_usize;
    while !store
        .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
        .await
        .unwrap()
    {
        pages += 1;
        assert!(pages < 1_000);
    }
    let elapsed = started.elapsed();
    let (entries,snapshots):(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM admin_media_snapshot_entries WHERE reservation_id=$1),snapshots FROM admin_media_snapshot_budget WHERE singleton").bind(reservation.id()).fetch_one(&db.pool).await.unwrap();
    assert_eq!((entries, snapshots), (10_000, 1));
    assert!(pages >= 625, "at most 16 relationships per media page");
    let intent = store
        .prepare_maintenance_deletion_page(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminDeletionPageKind::Media,
            Uuid::now_v7(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.entries().len(), 16);
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &reservation, &intent)
        .await
        .unwrap();
    println!(
        "provisional_offline_release_closure tracks={count} media_entries={entries} capture_turns={pages} elapsed_ms={} profile={} fixture=owned-synthetic filesystem=not-exercised",
        elapsed.as_millis(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    db.drop().await;
}

#[tokio::test]
async fn maintenance_object_intent_revalidates_artwork_and_directory_bindings() {
    let db = TestDb::create("maintenance_object_intent").await;
    let catalog = CatalogStore;
    let store = melodee_persistence::WorkItemStore;
    let release = seed_release(&db, &catalog, 560, "Object Scope", "Object Artist").await;
    let staging = format!("releases/{}/source", release.work_item_id);
    sqlx::query("INSERT INTO release_work_item_staging_handoffs(work_item_id,staging_relative_path,source_generation,status,method,tree_hash,intended_at,staged_at) VALUES($1,$2,'generation','staged','verified_copy',repeat('a',64),clock_timestamp(),clock_timestamp())").bind(release.work_item_id).bind(&staging).execute(&db.pool).await.unwrap();
    let image = format!("{}-5", "d".repeat(64));
    let replacement = format!("{}-5", "e".repeat(64));
    for key in [&image, &replacement] {
        sqlx::query("INSERT INTO asset_files(asset_key,size_bytes) VALUES($1,5)")
            .bind(key)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO catalog_entity_images(entity_type,release_id,image_mime_type,asset_key,uploaded_by,created_at,updated_at) VALUES('release',$1,'image/jpeg',$2,'test',clock_timestamp(),clock_timestamp())").bind(release.internal_id).bind(&image).execute(&db.pool).await.unwrap();
    let fence = store
        .begin_lifecycle_maintenance(&db.pool, "operator", time::Duration::minutes(1))
        .await
        .unwrap();
    store
        .confirm_lifecycle_quiescence(
            &db.pool,
            &fence,
            &melodee_persistence::LifecycleQuiescenceAttestation {
                reason: "test",
                backup_reference: "test-backup",
                stop_all_writers: true,
                inspection_complete: true,
            },
        )
        .await
        .unwrap();
    let reservation = store
        .begin_maintenance_admin_media_reservation(
            &db.pool,
            &[],
            &[release.internal_id],
            "operator",
            time::Duration::minutes(1),
            &fence,
        )
        .await
        .unwrap();
    store
        .bind_maintenance_catalog_targets(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminCatalogDeleteKind::Release,
            &[release.public_id],
        )
        .await
        .unwrap();
    store
        .begin_maintenance_media_snapshot(&db.pool, &fence, &reservation)
        .await
        .unwrap();
    let mut frozen = false;
    for _ in 0..20 {
        if store
            .capture_maintenance_media_snapshot_page(&db.pool, &fence, &reservation)
            .await
            .unwrap()
        {
            frozen = true;
            break;
        }
    }
    assert!(frozen);
    let intent = store
        .prepare_maintenance_deletion_page(
            &db.pool,
            &fence,
            &reservation,
            melodee_persistence::AdminDeletionPageKind::Object,
            Uuid::now_v7(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        intent
            .entries()
            .iter()
            .any(|entry| entry.kind == "artwork" && entry.object_key == image)
    );
    assert!(
        intent
            .entries()
            .iter()
            .any(|entry| entry.kind == "staging_directory" && entry.object_key == staging)
    );
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &reservation, &intent)
        .await
        .unwrap();
    sqlx::query("UPDATE catalog_entity_images SET asset_key=$2 WHERE release_id=$1")
        .bind(release.internal_id)
        .bind(&replacement)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        store
            .revalidate_maintenance_deletion_page(&db.pool, &fence, &reservation, &intent)
            .await
            .is_err()
    );
    sqlx::query("UPDATE catalog_entity_images SET asset_key=$2 WHERE release_id=$1")
        .bind(release.internal_id)
        .bind(&image)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path='replaced/path' WHERE work_item_id=$1").bind(release.work_item_id).execute(&db.pool).await.unwrap();
    assert!(
        store
            .revalidate_maintenance_deletion_page(&db.pool, &fence, &reservation, &intent)
            .await
            .is_err()
    );
    sqlx::query("UPDATE release_work_item_staging_handoffs SET staging_relative_path=$2 WHERE work_item_id=$1").bind(release.work_item_id).bind(&staging).execute(&db.pool).await.unwrap();
    store
        .revalidate_maintenance_deletion_page(&db.pool, &fence, &reservation, &intent)
        .await
        .unwrap();
    // Synthetic page state and cursor isolate the directory branch. A page
    // acknowledgement is not proof that a directory subtree was removed.
    sqlx::query("UPDATE admin_media_page_intents SET state='verified' WHERE id=$1")
        .bind(intent.id())
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE admin_media_snapshot_objects SET acknowledgement=$2 WHERE reservation_id=$1",
    )
    .bind(reservation.id())
    .bind(intent.id())
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO admin_media_coverage_audits(reservation_id,snapshot_revision,phase,last_kind,last_object) VALUES($1,$2,'objects','artwork',$3)")
        .bind(reservation.id()).bind(intent.snapshot_revision()).bind(Uuid::from_u128(u128::MAX)).execute(&db.pool).await.unwrap();
    assert!(
        store
            .audit_maintenance_deletion_coverage_page(&db.pool, &fence, &reservation)
            .await
            .is_err(),
        "an assertion-only directory acknowledgement cannot establish subtree proof"
    );
    db.drop().await;
}
