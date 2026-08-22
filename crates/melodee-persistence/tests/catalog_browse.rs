//! read-side store coverage: catalog browsing listings
//! (Artists/Releases/Tracks with filters, whitelist sorts, pagination) and
//! the bounded Artist/Release/Track detail reads.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use melodee_domain::{
    ArtistAliasId, ArtistAliasType, ArtistRelationshipId, ArtistRelationshipKind, Confidence,
    ContributionId, ContributionRole, ContributionTarget, DateRange, ExternalIdentityId,
    ExternalIdentityOwner, GroupMembershipId, MembershipRole,
};
use melodee_persistence::{
    CatalogArtistSort, CatalogReleaseSort, CatalogStore, CatalogTrackSort,
    MAX_CATALOG_BROWSE_QUERY_CHARS, contains_pattern,
};
use time::{Date, Month, OffsetDateTime};
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

/// Creates one verified Artist directly through the catalog store and returns
/// its (internal, public) identity pair.
async fn seed_artist(db: &TestDb, canonical: &str) -> (melodee_domain::ArtistId, Uuid) {
    let now = OffsetDateTime::now_utc();
    let artist = melodee_domain::Artist::new(melodee_domain::NewArtist {
        id: melodee_domain::ArtistId::new(),
        public_api_id: melodee_domain::ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: canonical.to_string(),
        normalized_name: canonical.to_lowercase(),
        sort_name: canonical.to_string(),
        artist_type: melodee_domain::ArtistType::Person,
        disambiguation: None,
        active_dates: None,
        status: melodee_domain::ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid artist");
    let public = artist.public_api_id().into_uuid();
    let id = artist.id();
    CatalogStore
        .create_artist(&db.pool, &artist)
        .await
        .expect("create artist");
    (id, public)
}

#[tokio::test]
async fn contains_pattern_escapes_like_metacharacters_and_bounds_length() {
    assert_eq!(contains_pattern(None).expect("none"), None);
    assert_eq!(contains_pattern(Some("   ")).expect("blank"), None);
    assert_eq!(
        contains_pattern(Some("100%_\\")).expect("escaped"),
        Some("%100\\%\\_\\\\%".to_string())
    );
    let too_long = "x".repeat(MAX_CATALOG_BROWSE_QUERY_CHARS + 1);
    assert!(contains_pattern(Some(&too_long)).is_err());
}

#[tokio::test]
async fn artist_listing_filters_by_name_and_alias_and_paginates() {
    let db = TestDb::create("browse_artist_listing").await;
    let alpha = phase5_common::seed_cataloged_release(
        &db,
        1,
        presentation("Alpha Release", "Alpha Artist", "Alpha Track"),
    )
    .await;
    let _beta = phase5_common::seed_cataloged_release(
        &db,
        2,
        presentation("Beta Release", "Beta Artist", "Beta Track"),
    )
    .await;

    // The import creates one provisional Artist per resolution, so each
    // Release contributes a release-scoped and a track-scoped Artist.
    let (items, total) = CatalogStore
        .list_catalog_artists(&db.pool, None, 50, 0, CatalogArtistSort::NameAsc)
        .await
        .expect("list artists");
    assert_eq!(total, 4);
    assert_eq!(items.len(), 4);
    assert!(
        items
            .iter()
            .all(|item| matches!(item.canonical_name.as_str(), "Alpha Artist" | "Beta Artist"))
    );
    assert_eq!(items[0].canonical_name, "Alpha Artist");
    assert_eq!(items[3].canonical_name, "Beta Artist");
    assert!(
        items
            .iter()
            .any(|item| item.release_count == 1 && item.status == "provisional"),
        "the release-scoped Artist carries the billed Release count"
    );

    // Substring filter on the canonical name.
    let (items, total) = CatalogStore
        .list_catalog_artists(&db.pool, Some("%alpha%"), 50, 0, CatalogArtistSort::NameAsc)
        .await
        .expect("filter artists");
    assert_eq!(total, 2);
    assert!(
        items
            .iter()
            .all(|item| item.canonical_name == "Alpha Artist")
    );

    // The same filter matches through a recorded alias.
    let alpha_public: Uuid = sqlx::query_scalar(
        "SELECT a.public_api_id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id
         WHERE ra.release_id = $1",
    )
    .bind(alpha.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("alpha artist public id");
    let alpha_artist = CatalogStore
        .artist_by_public_id(&db.pool, alpha_public)
        .await
        .expect("lookup")
        .expect("alpha artist");
    let now = OffsetDateTime::now_utc();
    CatalogStore
        .add_artist_alias(
            &db.pool,
            &melodee_domain::ArtistAlias {
                id: ArtistAliasId::new(),
                artist_id: alpha_artist.id,
                value: "The Alphas".to_string(),
                normalized_value: "the alphas".to_string(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "fixture".to_string(),
                confidence: Confidence::new(9_000).expect("confidence"),
                locked: false,
            },
            now,
        )
        .await
        .expect("add alias");
    let (items, total) = CatalogStore
        .list_catalog_artists(
            &db.pool,
            Some("%alphas%"),
            50,
            0,
            CatalogArtistSort::NameAsc,
        )
        .await
        .expect("alias filter");
    assert_eq!(total, 1);
    assert_eq!(items[0].canonical_name, "Alpha Artist");

    // Sorting and paging are honored with the filter total intact.
    let (items, _) = CatalogStore
        .list_catalog_artists(&db.pool, None, 50, 0, CatalogArtistSort::NameDesc)
        .await
        .expect("desc sort");
    assert_eq!(items[0].canonical_name, "Beta Artist");
    let (items, total) = CatalogStore
        .list_catalog_artists(&db.pool, None, 1, 1, CatalogArtistSort::NameAsc)
        .await
        .expect("paged");
    assert_eq!(total, 4);
    assert_eq!(items.len(), 1);

    // Sort whitelist parsing is strict.
    assert!(CatalogArtistSort::parse("name:asc").is_some());
    assert!(CatalogArtistSort::parse("bogus").is_none());
    assert!(CatalogArtistSort::parse("name:sideways").is_none());
    assert!(CatalogReleaseSort::parse("releaseDate:desc").is_some());
    assert!(CatalogTrackSort::parse("durationMs:asc").is_some());

    db.drop().await;
}

#[tokio::test]
async fn release_listing_filters_by_title_and_artist() {
    let db = TestDb::create("browse_release_listing").await;
    let alpha = phase5_common::seed_cataloged_release(
        &db,
        3,
        presentation("Alpha Release", "Listing Artist A", "Alpha Track"),
    )
    .await;
    let _beta = phase5_common::seed_cataloged_release(
        &db,
        4,
        presentation("Beta Release", "Listing Artist B", "Beta Track"),
    )
    .await;

    let (items, total) = CatalogStore
        .list_catalog_releases(&db.pool, None, None, 50, 0, CatalogReleaseSort::TitleAsc)
        .await
        .expect("list releases");
    assert_eq!(total, 2);
    assert_eq!(items[0].title, "Alpha Release");
    assert_eq!(items[0].track_count, 1);
    assert_eq!(
        items[0].release_date,
        Some(Date::from_calendar_date(2007, Month::August, 9).expect("date"))
    );

    let (items, total) = CatalogStore
        .list_catalog_releases(
            &db.pool,
            Some("%beta%"),
            None,
            50,
            0,
            CatalogReleaseSort::TitleAsc,
        )
        .await
        .expect("title filter");
    assert_eq!(total, 1);
    assert_eq!(items[0].title, "Beta Release");

    // The artist filter joins through the billed ReleaseArtist link.
    let artist_public: Uuid = sqlx::query_scalar(
        "SELECT a.public_api_id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id
         WHERE ra.release_id = $1",
    )
    .bind(alpha.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("artist public id");
    let (items, total) = CatalogStore
        .list_catalog_releases(
            &db.pool,
            None,
            Some(artist_public),
            50,
            0,
            CatalogReleaseSort::TitleAsc,
        )
        .await
        .expect("artist filter");
    assert_eq!(total, 1);
    assert_eq!(items[0].title, "Alpha Release");

    db.drop().await;
}

#[tokio::test]
async fn track_listing_filters_by_release_and_title() {
    let db = TestDb::create("browse_track_listing").await;
    let alpha = phase5_common::seed_cataloged_release(
        &db,
        5,
        presentation("Track Fixture A", "Track Artist A", "First Track"),
    )
    .await;
    let _beta = phase5_common::seed_cataloged_release(
        &db,
        6,
        presentation("Track Fixture B", "Track Artist B", "Second Track"),
    )
    .await;

    let (items, total) = CatalogStore
        .list_catalog_tracks(&db.pool, None, None, 50, 0, CatalogTrackSort::TitleAsc)
        .await
        .expect("list tracks");
    assert_eq!(total, 2);
    assert_eq!(items[0].title, "First Track");
    assert_eq!(items[0].duration_ms, Some(180_000));

    let release_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(alpha.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("release public id");
    let (items, total) = CatalogStore
        .list_catalog_tracks(
            &db.pool,
            None,
            Some(release_public),
            50,
            0,
            CatalogTrackSort::TitleAsc,
        )
        .await
        .expect("release filter");
    assert_eq!(total, 1);
    assert_eq!(items[0].release_title, "Track Fixture A");

    let (_items, total) = CatalogStore
        .list_catalog_tracks(
            &db.pool,
            Some("%second%"),
            None,
            50,
            0,
            CatalogTrackSort::TitleAsc,
        )
        .await
        .expect("title filter");
    assert_eq!(total, 1);

    db.drop().await;
}

#[tokio::test]
async fn artist_detail_exposes_identity_stats_and_bounded_sections() {
    let db = TestDb::create("browse_artist_detail").await;
    let release = phase5_common::seed_cataloged_release(
        &db,
        7,
        presentation("Detail Release", "Detail Fixture", "Detail Track"),
    )
    .await;
    let (member_id, member_public) = seed_artist(&db, "Detail Member").await;
    let (_group_id, group_public) = seed_artist(&db, "Detail Group").await;
    let now = OffsetDateTime::now_utc();

    CatalogStore
        .add_artist_alias(
            &db.pool,
            &melodee_domain::ArtistAlias {
                id: ArtistAliasId::new(),
                artist_id: member_id,
                value: "Member Alias".to_string(),
                normalized_value: "member alias".to_string(),
                alias_type: ArtistAliasType::FormerName,
                locale: Some("en".to_string()),
                evidence_source: "fixture".to_string(),
                confidence: Confidence::new(8_000).expect("confidence"),
                locked: true,
            },
            now,
        )
        .await
        .expect("alias");
    CatalogStore
        .add_artist_external_identity(
            &db.pool,
            &melodee_domain::ExternalIdentity {
                id: ExternalIdentityId::new(),
                owner: ExternalIdentityOwner::Artist(member_id),
                provider: "musicbrainz".to_string(),
                external_id: "mbid-fixture".to_string(),
                confidence: Confidence::new(10_000).expect("confidence"),
                evidence_source: "fixture".to_string(),
                verified_at: Some(now),
                locked: false,
                provider_guarantees_uniqueness: true,
            },
            now,
        )
        .await
        .expect("external identity");
    CatalogStore
        .add_artist_relationship(
            &db.pool,
            &melodee_domain::ArtistRelationship {
                id: ArtistRelationshipId::new(),
                subject_artist_id: member_id,
                object_artist_id: melodee_domain::ArtistId::from_uuid(
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM artists WHERE public_api_id = $1",
                    )
                    .bind(group_public)
                    .fetch_one(&db.pool)
                    .await
                    .expect("group id"),
                ),
                kind: ArtistRelationshipKind::new("collaborator".to_string()).expect("kind"),
                active_dates: None,
                confidence: Confidence::new(9_000).expect("confidence"),
                evidence_source: "fixture".to_string(),
            },
            now,
        )
        .await
        .expect("relationship");
    CatalogStore
        .add_group_membership(
            &db.pool,
            &melodee_domain::GroupMembership {
                id: GroupMembershipId::new(),
                member_artist_id: member_id,
                group_artist_id: melodee_domain::ArtistId::from_uuid(
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM artists WHERE public_api_id = $1",
                    )
                    .bind(group_public)
                    .fetch_one(&db.pool)
                    .await
                    .expect("group id"),
                ),
                active_dates: DateRange::new(
                    Some(Date::from_calendar_date(1999, Month::January, 1).expect("date")),
                    None,
                )
                .expect("range"),
                role: MembershipRole::new("guitar".to_string()).expect("role"),
                confidence: Confidence::new(10_000).expect("confidence"),
                evidence_source: "fixture".to_string(),
                excluded_tracks: Default::default(),
            },
            now,
        )
        .await
        .expect("membership");
    CatalogStore
        .add_contribution(
            &db.pool,
            &melodee_domain::Contribution {
                id: ContributionId::new(),
                target: ContributionTarget::Release(release.release_id()),
                artist_id: member_id,
                role: ContributionRole::Producer,
                credit_text: Some("Produced by Detail Member".to_string()),
                evidence_source: "fixture".to_string(),
            },
            now,
        )
        .await
        .expect("contribution");

    let detail = CatalogStore
        .artist_catalog_detail(&db.pool, member_public)
        .await
        .expect("detail read")
        .expect("artist exists");
    assert_eq!(detail.canonical_name, "Detail Member");
    assert_eq!(detail.artist_type, "person");
    assert_eq!(detail.status, "verified");
    // The Producer contribution enters the participation projection, so the
    // set-based aggregate credits the member with the Release's Track.
    assert_eq!(detail.release_count, 1);
    assert_eq!(detail.performance_track_count, 0);
    assert_eq!(detail.all_credit_track_count, 1);
    assert_eq!(detail.aliases.total, 1);
    assert_eq!(detail.aliases.items[0].alias, "Member Alias");
    assert_eq!(detail.aliases.items[0].alias_type, "former_name");
    assert!(detail.aliases.items[0].is_locked);
    assert_eq!(detail.external_ids.total, 1);
    assert_eq!(detail.external_ids.items[0].provider, "musicbrainz");
    assert_eq!(detail.relationships.total, 1);
    assert_eq!(
        detail.relationships.items[0].relationship_type,
        "collaborator"
    );
    assert_eq!(
        detail.relationships.items[0].target_public_api_id,
        group_public
    );
    assert_eq!(detail.memberships.total, 1);
    assert_eq!(
        detail.memberships.items[0].group_public_api_id,
        group_public
    );
    assert_eq!(detail.memberships.items[0].role, "guitar");
    assert_eq!(detail.contributions.total, 1);
    assert_eq!(detail.contributions.items[0].role, "producer");
    assert!(!detail.contributions.items[0].is_performance);
    assert_eq!(
        detail.contributions.items[0].release_title,
        "Detail Release"
    );
    // create_artist + alias + external identity + relationship + membership
    // each append Artist-scope audit events.
    assert!(
        detail.history.total >= 5,
        "audit history covers the recorded mutations: {}",
        detail.history.total
    );
    assert!(
        detail
            .history
            .items
            .iter()
            .any(|event| event.event_type == "artist_created")
    );

    // The import-billed provisional Artist reports set-based aggregate stats.
    let billed_public: Uuid = sqlx::query_scalar(
        "SELECT a.public_api_id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id
         WHERE ra.release_id = $1",
    )
    .bind(release.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("billed artist");
    let billed = CatalogStore
        .artist_catalog_detail(&db.pool, billed_public)
        .await
        .expect("detail read")
        .expect("artist exists");
    assert_eq!(billed.release_count, 1);
    assert_eq!(billed.releases.total, 1);
    assert_eq!(billed.releases.items[0].title, "Detail Release");
    assert_eq!(billed.releases.items[0].role, "primary");

    // Unknown public ids resolve to None (the API maps this to 404).
    let missing = CatalogStore
        .artist_catalog_detail(&db.pool, Uuid::now_v7())
        .await
        .expect("missing read");
    assert!(missing.is_none());

    db.drop().await;
}

#[tokio::test]
async fn release_detail_exposes_tracks_media_provenance_and_history() {
    let db = TestDb::create("browse_release_detail").await;
    let release = phase5_common::seed_cataloged_release(
        &db,
        8,
        presentation("Release Detail Fixture", "Release Artist", "Release Track"),
    )
    .await;
    let release_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("release public id");

    let detail = CatalogStore
        .release_catalog_detail(&db.pool, release_public)
        .await
        .expect("detail read")
        .expect("release exists");
    assert_eq!(detail.title, "Release Detail Fixture");
    assert_eq!(detail.status, "active");
    assert_eq!(detail.track_count, 1);
    assert_eq!(detail.duration_ms, 180_000);
    assert!(detail.media_bytes > 0);

    assert_eq!(detail.tracks.total, 1);
    let track = &detail.tracks.items[0];
    assert_eq!(track.title, "Release Track");
    assert_eq!(track.ordinal, 1);
    assert_eq!(track.container.as_deref(), Some("mp3"));
    assert_eq!(track.duration_ms, Some(180_000));
    assert!(track.byte_size.is_some());

    assert_eq!(detail.artists.len(), 1);
    assert_eq!(detail.artists[0].canonical_name, "Release Artist");
    assert_eq!(detail.artists[0].role, "primary");

    assert_eq!(detail.media_assets.total, 1);
    assert_eq!(detail.media_assets.items[0].mime_type, "audio/mpeg");

    let provenance = detail.provenance.expect("import provenance");
    assert_eq!(provenance.work_item_id, release.work_item_id());
    assert_eq!(provenance.manifest_schema_version, 3);

    assert!(
        detail
            .history
            .items
            .iter()
            .any(|event| event.event_type == "manifest_v3_imported")
    );

    let missing = CatalogStore
        .release_catalog_detail(&db.pool, Uuid::now_v7())
        .await
        .expect("missing read");
    assert!(missing.is_none());

    db.drop().await;
}

#[tokio::test]
async fn track_detail_exposes_release_linkage_artists_and_media() {
    let db = TestDb::create("browse_track_detail").await;
    let release = phase5_common::seed_cataloged_release(
        &db,
        9,
        presentation(
            "Track Detail Fixture",
            "Track Detail Artist",
            "Detail Song Title",
        ),
    )
    .await;
    let (track_public, release_public): (Uuid, Uuid) = sqlx::query_as(
        "SELECT t.public_api_id, r.public_api_id FROM tracks t
         JOIN releases r ON r.id = t.release_id WHERE t.release_id = $1",
    )
    .bind(release.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("track public id");

    let detail = CatalogStore
        .track_catalog_detail(&db.pool, track_public)
        .await
        .expect("detail read")
        .expect("track exists");
    assert_eq!(detail.title, "Detail Song Title");
    assert_eq!(detail.release_public_api_id, release_public);
    assert_eq!(detail.release_title, "Track Detail Fixture");
    assert_eq!(detail.duration_ms, Some(180_000));

    // The manifest track-scoped observation produced one billed Track artist.
    assert_eq!(detail.artists.len(), 1);
    assert_eq!(detail.artists[0].canonical_name, "Track Detail Artist");
    assert_eq!(detail.artists[0].role, "primary");

    // Exactly one playable asset link, carrying the normalized format facts.
    assert_eq!(detail.media_assets.len(), 1);
    let (purpose, asset) = &detail.media_assets[0];
    assert_eq!(purpose, "playable");
    assert_eq!(asset.container.as_deref(), Some("mp3"));
    assert_eq!(asset.channels, Some(2));
    assert_eq!(asset.sample_rate_hz, Some(48_000));

    let missing = CatalogStore
        .track_catalog_detail(&db.pool, Uuid::now_v7())
        .await
        .expect("missing read");
    assert!(missing.is_none());

    db.drop().await;
}
