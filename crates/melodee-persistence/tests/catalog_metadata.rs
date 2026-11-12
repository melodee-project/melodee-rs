//! Persistence proofs for the W0 / RELEASE-002 metadata-edit contract:
//! round-trip and partial patches, revision bumps, audit events, transaction
//! rollback on validation failure, and rejection of blank names, non-positive
//! ordinals, and negative durations.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use melodee_persistence::{
    ArtistMetadataPatch, CatalogStore, CatalogStoreError, ReleaseMetadataPatch, TrackMetadataPatch,
};
use time::{Date, Month};
use uuid::Uuid;

fn presentation<'a>(
    release_title: &'a str,
    release_artist: &'a str,
    track_title: &'a str,
) -> CatalogFixturePresentation<'a> {
    CatalogFixturePresentation {
        release_title,
        release_artist,
        track_title,
        track_artist: release_artist,
        release_date: "2007-08-09",
    }
}

/// Resolves the internal ids of the seeded Release, its billed Artist, and its Track.
async fn internal_ids(pool: &sqlx::PgPool, release_id: Uuid) -> (Uuid, Uuid, Uuid) {
    let (release_id, track_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT r.id, t.id FROM releases r
         JOIN tracks t ON t.release_id = r.id WHERE r.id = $1",
    )
    .bind(release_id)
    .fetch_one(pool)
    .await
    .expect("release/track ids");
    let artist_id: Uuid = sqlx::query_scalar(
        "SELECT a.id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id
         WHERE ra.release_id = $1",
    )
    .bind(release_id)
    .fetch_one(pool)
    .await
    .expect("artist id");
    (release_id, artist_id, track_id)
}

async fn revision(pool: &sqlx::PgPool, table: &str, id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT revision FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("revision row")
}

async fn audit_count(pool: &sqlx::PgPool, entity_type: &str, entity_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM catalog_audit_events WHERE entity_type = $1 AND entity_id = $2",
    )
    .bind(entity_type)
    .bind(entity_id)
    .fetch_one(pool)
    .await
    .expect("audit count")
}

#[tokio::test]
async fn release_metadata_round_trip_partial_and_audit() {
    let db = TestDb::create("release_metadata").await;
    let store = CatalogStore;
    let release = phase5_common::seed_cataloged_release(
        &db,
        1,
        presentation("Meta Release", "Meta Artist", "Meta Track"),
    )
    .await;
    let (release_id, _artist_id, _track_id) =
        internal_ids(&db.pool, release.release_id().into_uuid()).await;

    // Round-trip: full editable patch.
    let before = revision(&db.pool, "releases", release_id).await;
    let audit_before = audit_count(&db.pool, "release", release_id).await;
    store
        .update_release_metadata(
            &db.pool,
            release_id,
            &ReleaseMetadataPatch {
                title: Some("New Title".to_string()),
                sort_title: Some("custom sort".to_string()),
                display_artist: Some("Meta Artist".to_string()),
                release_date: Some(
                    Date::from_calendar_date(2008, Month::January, 15).expect("date"),
                ),
                release_date_text: Some("Jan 2008".to_string()),
            },
            "test-admin",
        )
        .await
        .expect("round-trip patch");

    let row: (String, String, String, Option<String>, i64) = sqlx::query_as(
        "SELECT title, sort_title, normalized_title, release_date_text, revision
         FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("release row");
    assert_eq!(row.0, "New Title");
    // Explicit sort_title wins over the computed key.
    assert_eq!(row.1, "custom sort");
    assert_eq!(row.2, "new title");
    assert_eq!(row.3.as_deref(), Some("Jan 2008"));
    assert_eq!(row.4, before + 1);
    assert_eq!(
        audit_count(&db.pool, "release", release_id).await,
        audit_before + 1
    );

    // Partial patch: only the title changes; other columns stay.
    store
        .update_release_metadata(
            &db.pool,
            release_id,
            &ReleaseMetadataPatch {
                title: Some("Final Title".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("partial patch");

    let row: (String, String, Option<String>, i64) = sqlx::query_as(
        "SELECT title, sort_title, release_date_text, revision FROM releases WHERE id = $1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("release row");
    assert_eq!(row.0, "Final Title");
    // Title change recomputes sort_title when no explicit sort is supplied.
    assert_eq!(row.1, "final title");
    assert_eq!(
        row.2.as_deref(),
        Some("Jan 2008"),
        "release_date_text untouched"
    );
    assert_eq!(row.3, before + 2);

    db.drop().await;
}

#[tokio::test]
async fn release_metadata_rejects_blank_and_unknown() {
    let db = TestDb::create("release_metadata_invalid").await;
    let store = CatalogStore;
    let release = phase5_common::seed_cataloged_release(
        &db,
        1,
        presentation("Meta Release", "Meta Artist", "Meta Track"),
    )
    .await;
    let (release_id, _, _) = internal_ids(&db.pool, release.release_id().into_uuid()).await;
    let before = revision(&db.pool, "releases", release_id).await;
    let audit_before = audit_count(&db.pool, "release", release_id).await;

    // Blank title is rejected; nothing is written and the revision is stable.
    let result = store
        .update_release_metadata(
            &db.pool,
            release_id,
            &ReleaseMetadataPatch {
                title: Some("   ".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));
    assert_eq!(revision(&db.pool, "releases", release_id).await, before);
    assert_eq!(
        audit_count(&db.pool, "release", release_id).await,
        audit_before
    );

    // Unknown release id is rejected.
    let result = store
        .update_release_metadata(
            &db.pool,
            Uuid::now_v7(),
            &ReleaseMetadataPatch {
                title: Some("Ghost".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));

    db.drop().await;
}

#[tokio::test]
async fn artist_metadata_round_trip_partial_and_rejects() {
    let db = TestDb::create("artist_metadata").await;
    let store = CatalogStore;
    let release = phase5_common::seed_cataloged_release(
        &db,
        1,
        presentation("Meta Release", "Meta Artist", "Meta Track"),
    )
    .await;
    let (_release_id, artist_id, _track_id) =
        internal_ids(&db.pool, release.release_id().into_uuid()).await;

    // Round-trip.
    let before = revision(&db.pool, "artists", artist_id).await;
    let audit_before = audit_count(&db.pool, "artist", artist_id).await;
    store
        .update_artist_metadata(
            &db.pool,
            artist_id,
            &ArtistMetadataPatch {
                canonical_name: Some("New Artist".to_string()),
                sort_name: Some("artist, new".to_string()),
                artist_type: Some("orchestra".to_string()),
                status: None,
            },
            "test-admin",
        )
        .await
        .expect("artist round-trip");

    let row: (String, String, String, i64) = sqlx::query_as(
        "SELECT canonical_name, sort_name, artist_type, revision FROM artists WHERE id = $1",
    )
    .bind(artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("artist row");
    assert_eq!(row.0, "New Artist");
    assert_eq!(row.1, "artist, new", "explicit sort wins");
    assert_eq!(row.2, "orchestra");
    assert_eq!(row.3, before + 1);
    assert_eq!(
        audit_count(&db.pool, "artist", artist_id).await,
        audit_before + 1
    );

    // Name change without explicit sort recomputes normalized + sort.
    store
        .update_artist_metadata(
            &db.pool,
            artist_id,
            &ArtistMetadataPatch {
                canonical_name: Some("The Finalists".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("artist partial");
    let row: (String, String, i64) =
        sqlx::query_as("SELECT canonical_name, sort_name, revision FROM artists WHERE id = $1")
            .bind(artist_id)
            .fetch_one(&db.pool)
            .await
            .expect("artist row");
    assert_eq!(row.0, "The Finalists");
    assert_eq!(row.1, "finalists", "article stripped");
    assert_eq!(row.2, before + 2);

    // Blank canonical name is rejected without side effects.
    let result = store
        .update_artist_metadata(
            &db.pool,
            artist_id,
            &ArtistMetadataPatch {
                canonical_name: Some("".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));
    assert_eq!(revision(&db.pool, "artists", artist_id).await, before + 2);

    db.drop().await;
}

#[tokio::test]
async fn track_metadata_round_trip_partial_and_rejects() {
    let db = TestDb::create("track_metadata").await;
    let store = CatalogStore;
    let release = phase5_common::seed_cataloged_release(
        &db,
        1,
        presentation("Meta Release", "Meta Artist", "Meta Track"),
    )
    .await;
    let (_release_id, _artist_id, track_id) =
        internal_ids(&db.pool, release.release_id().into_uuid()).await;

    // Round-trip.
    let before = revision(&db.pool, "tracks", track_id).await;
    let audit_before = audit_count(&db.pool, "track", track_id).await;
    store
        .update_track_metadata(
            &db.pool,
            track_id,
            &TrackMetadataPatch {
                title: Some("New Track".to_string()),
                display_artist: Some("Meta Artist".to_string()),
                ordinal: Some(3),
                duration_ms: Some(240_000),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("track round-trip");

    let row: (String, String, i32, i64, i64) = sqlx::query_as(
        "SELECT title, display_artist, ordinal, duration_ms, revision FROM tracks WHERE id = $1",
    )
    .bind(track_id)
    .fetch_one(&db.pool)
    .await
    .expect("track row");
    assert_eq!(row.0, "New Track");
    assert_eq!(row.1, "Meta Artist");
    assert_eq!(row.2, 3);
    assert_eq!(row.3, 240_000);
    assert_eq!(row.4, before + 1);
    assert_eq!(
        audit_count(&db.pool, "track", track_id).await,
        audit_before + 1
    );

    // Partial patch: duration only.
    store
        .update_track_metadata(
            &db.pool,
            track_id,
            &TrackMetadataPatch {
                duration_ms: Some(300_000),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("track partial");
    let (title, ordinal, duration, revision): (String, i32, i64, i64) =
        sqlx::query_as("SELECT title, ordinal, duration_ms, revision FROM tracks WHERE id = $1")
            .bind(track_id)
            .fetch_one(&db.pool)
            .await
            .expect("track row");
    assert_eq!(title, "New Track", "title untouched");
    assert_eq!(ordinal, 3, "ordinal untouched");
    assert_eq!(duration, 300_000);
    assert_eq!(revision, before + 2);

    // Non-positive ordinal rejected.
    let result = store
        .update_track_metadata(
            &db.pool,
            track_id,
            &TrackMetadataPatch {
                ordinal: Some(0),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));

    // Negative duration rejected.
    let result = store
        .update_track_metadata(
            &db.pool,
            track_id,
            &TrackMetadataPatch {
                duration_ms: Some(-1),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));

    // Unknown track id rejected.
    let result = store
        .update_track_metadata(
            &db.pool,
            Uuid::now_v7(),
            &TrackMetadataPatch {
                title: Some("Ghost".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await;
    assert!(matches!(result, Err(CatalogStoreError::InvalidRequest)));

    db.drop().await;
}

/// Import always creates provisional Artists and nothing promoted them, so the
/// operator status edit is the only auditable path to `verified`. System states
/// stay system-managed.
#[tokio::test]
async fn artist_status_is_operator_settable_but_system_states_are_not() {
    let db = TestDb::create("metadata_artist_status").await;
    let store = CatalogStore;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7,
        presentation("Status Release", "Provisional Artist", "Status Track"),
    )
    .await;
    let (_release_id, artist_id, _track_id) =
        internal_ids(&db.pool, fixture.release_id().into_uuid()).await;
    let status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id = $1")
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("imported artist status");
    assert_eq!(status, "provisional", "import never promotes an Artist");
    let audit_before = audit_count(&db.pool, "artist", artist_id).await;

    store
        .update_artist_metadata(
            &db.pool,
            artist_id,
            &ArtistMetadataPatch {
                status: Some("verified".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("promote to verified");
    let status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id = $1")
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("artist status");
    assert_eq!(status, "verified");
    assert_eq!(
        audit_count(&db.pool, "artist", artist_id).await,
        audit_before + 1,
        "the promotion is audited"
    );

    // Back to provisional is allowed (an operator can withdraw the assertion).
    store
        .update_artist_metadata(
            &db.pool,
            artist_id,
            &ArtistMetadataPatch {
                status: Some("provisional".to_string()),
                ..Default::default()
            },
            "test-admin",
        )
        .await
        .expect("return to provisional");

    // Merge and delete flows own their states; a metadata edit cannot set them.
    for rejected in ["merged", "deprecated", "VERIFIED"] {
        let result = store
            .update_artist_metadata(
                &db.pool,
                artist_id,
                &ArtistMetadataPatch {
                    status: Some(rejected.to_string()),
                    ..Default::default()
                },
                "test-admin",
            )
            .await;
        assert!(
            matches!(result, Err(CatalogStoreError::InvalidRequest)),
            "{rejected} must be rejected"
        );
    }
    let status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id = $1")
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("artist status");
    assert_eq!(status, "provisional", "rejected edits change nothing");

    db.drop().await;
}
