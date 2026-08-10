//! Date-aware direct/inferred participation and credit-presentation proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::BTreeSet;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{T0, TestDb, test_config};
use melodee_domain::{
    Artist, ArtistId, ArtistPublicId, ArtistStatus, ArtistType, Confidence, Contribution,
    ContributionId, ContributionRole, ContributionTarget, DateRange, GroupMembership,
    GroupMembershipId, MembershipRole, NewArtist, ParticipationKind, TrackId,
};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkerId};
use sqlx::PgPool;
use time::{Date, Month};
use uuid::Uuid;

async fn insert_artist(pool: &PgPool, id: Uuid, name: &str, artist_type: &str) {
    let artist = Artist::new(NewArtist {
        id: ArtistId::from_uuid(id),
        public_api_id: ArtistPublicId::from_uuid(id),
        canonical_name: name.to_owned(),
        normalized_name: name.to_uppercase().replace(' ', ""),
        sort_name: name.to_owned(),
        artist_type: match artist_type {
            "person" => ArtistType::Person,
            "group" => ArtistType::Group,
            _ => ArtistType::Unknown,
        },
        disambiguation: None,
        active_dates: None,
        status: ArtistStatus::Verified,
        created_at: T0,
    })
    .expect("valid participation Artist");
    CatalogStore
        .create_artist(pool, &artist)
        .await
        .expect("insert participation Artist");
}

async fn insert_track(
    pool: &PgPool,
    title: &str,
    release_date: Option<&str>,
    display_artist: &str,
) -> Uuid {
    let release_id = Uuid::now_v7();
    let track_id = Uuid::now_v7();
    let audio_identity_id = Uuid::now_v7();
    let media_asset_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO releases (
            id, public_api_id, title, normalized_title, sort_title,
            display_artist, release_date, release_date_text, created_at, updated_at
         ) VALUES ($1,$1,$2,upper(replace($2,' ','')),$2,$3,$4::date,$4,$5,$5)",
    )
    .bind(release_id)
    .bind(format!("{title} Release"))
    .bind(display_artist)
    .bind(release_date)
    .bind(T0)
    .execute(pool)
    .await
    .expect("insert participation Release");
    sqlx::query(
        "INSERT INTO audio_identities (id,status,created_at,updated_at)
         VALUES ($1,'provisional',$2,$2)",
    )
    .bind(audio_identity_id)
    .bind(T0)
    .execute(pool)
    .await
    .expect("insert AudioIdentity");
    let mut track_tx = pool
        .begin()
        .await
        .expect("begin complete participation Track fixture");
    sqlx::query(
        "INSERT INTO tracks (
            id, public_api_id, release_id, audio_identity_id, playable_media_asset_id, ordinal,
            title, normalized_title, sort_title, display_artist, duration_ms,
            created_at, updated_at
         ) VALUES ($1,$1,$2,$3,$4,1,$5,upper(replace($5,' ','')),$5,$6,180000,$7,$7)",
    )
    .bind(track_id)
    .bind(release_id)
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .bind(title)
    .bind(display_artist)
    .bind(T0)
    .execute(&mut *track_tx)
    .await
    .expect("insert participation Track");
    sqlx::query(
        "INSERT INTO media_assets (
            id, canonical_relative_path, byte_hash, byte_size, mime_type, created_at
         ) VALUES ($1,$2,$3,42,'audio/mpeg',$4)",
    )
    .bind(media_asset_id)
    .bind(format!("releases/fixture/{track_id}.mp3"))
    .bind("a".repeat(64))
    .bind(T0)
    .execute(&mut *track_tx)
    .await
    .expect("insert participation MediaAsset");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,created_at)
         VALUES ($1,$2,'playable',$3)",
    )
    .bind(track_id)
    .bind(media_asset_id)
    .bind(T0)
    .execute(&mut *track_tx)
    .await
    .expect("link playable MediaAsset");
    track_tx
        .commit()
        .await
        .expect("commit complete participation Track fixture");
    track_id
}

#[tokio::test]
async fn colin_hay_query_distinguishes_direct_inferred_excluded_and_all_credits() {
    let db = TestDb::create("catalog_colin_hay_participation").await;
    let colin = Uuid::now_v7();
    let men_at_work = Uuid::now_v7();
    let somebody = Uuid::now_v7();
    insert_artist(&db.pool, colin, "Colin Hay", "person").await;
    insert_artist(&db.pool, men_at_work, "Men at Work", "group").await;
    insert_artist(&db.pool, somebody, "Somebody", "person").await;

    let solo = insert_track(&db.pool, "Solo", Some("2020-01-01"), "Colin Hay").await;
    let group_inside =
        insert_track(&db.pool, "Group Inside", Some("1983-01-01"), "Men at Work").await;
    let group_outside =
        insert_track(&db.pool, "Group Outside", Some("1990-01-01"), "Men at Work").await;
    let group_excluded = insert_track(
        &db.pool,
        "Group Excluded",
        Some("1984-01-01"),
        "Men at Work",
    )
    .await;
    let group_unknown_date =
        insert_track(&db.pool, "Group Unknown Date", None, "Men at Work").await;
    let guest = insert_track(
        &db.pool,
        "Guest",
        Some("2010-01-01"),
        "Somebody feat. Colin Hay",
    )
    .await;
    let composer_fixture = seed_published_release(
        &db,
        71,
        CatalogFixturePresentation {
            release_title: "Release-level Composer Credit",
            release_artist: "Somebody",
            track_title: "Composer",
            track_artist: "Somebody",
            release_date: "2011-01-01",
        },
    )
    .await;
    let catalog_now = time::OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    let claimed = CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), catalog_now)
        .await
        .expect("claim composer fixture");
    assert_eq!(claimed.len(), 1);
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            composer_fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("participation-fixture-v1", "participation-test"),
            catalog_now,
        )
        .await
        .expect("import composer fixture");
    let composer: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(composer_fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("query imported composer Track");

    for (track_id, artist_id, display_value) in [
        (solo, colin, "Colin Hay"),
        (group_outside, men_at_work, "Men at Work"),
        (group_excluded, men_at_work, "Men at Work"),
        (group_unknown_date, men_at_work, "Men at Work"),
    ] {
        sqlx::query(
            "INSERT INTO track_artists (
                id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at
             ) VALUES ($1,$2,$3,'primary',1,$4,'',$5)",
        )
        .bind(Uuid::now_v7())
        .bind(track_id)
        .bind(artist_id)
        .bind(display_value)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("persist direct Track billing");
    }
    let group_inside_release: (Uuid,) =
        sqlx::query_as("SELECT release_id FROM tracks WHERE id = $1")
            .bind(group_inside)
            .fetch_one(&db.pool)
            .await
            .expect("query Release-billed Track owner");
    sqlx::query(
        "INSERT INTO release_artists (
            id,release_id,artist_id,role,presentation_order,display_value,join_phrase,created_at
         ) VALUES ($1,$2,$3,'primary',1,'Men at Work','',$4)",
    )
    .bind(Uuid::now_v7())
    .bind(group_inside_release.0)
    .bind(men_at_work)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("persist Release-level group billing");
    sqlx::query(
        "INSERT INTO track_artists (
            id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at
         ) VALUES
            ($1,$2,$3,'primary',1,'Somebody',' feat. ',$6),
            ($4,$2,$5,'featured',2,'Colin Hay','',$6)",
    )
    .bind(Uuid::now_v7())
    .bind(guest)
    .bind(somebody)
    .bind(Uuid::now_v7())
    .bind(colin)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("persist structured guest billing");

    let membership_id = GroupMembershipId::new();
    CatalogStore
        .add_group_membership(
            &db.pool,
            &GroupMembership {
                id: membership_id,
                member_artist_id: ArtistId::from_uuid(colin),
                group_artist_id: ArtistId::from_uuid(men_at_work),
                active_dates: DateRange::new(
                    Some(Date::from_calendar_date(1978, Month::January, 1).expect("valid date")),
                    Some(Date::from_calendar_date(1986, Month::December, 31).expect("valid date")),
                )
                .expect("valid inclusive membership range"),
                role: MembershipRole::new("member".to_owned()).expect("valid role"),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                evidence_source: "fixture".to_owned(),
                excluded_tracks: [TrackId::from_uuid(group_excluded)].into_iter().collect(),
            },
            T0,
        )
        .await
        .expect("persist bounded membership and exclusion");
    CatalogStore
        .add_contribution(
            &db.pool,
            &Contribution {
                id: ContributionId::new(),
                target: ContributionTarget::Release(composer_fixture.release_id),
                artist_id: ArtistId::from_uuid(colin),
                role: ContributionRole::Composer,
                credit_text: Some("Colin Hay".to_owned()),
                evidence_source: "fixture".to_owned(),
            },
            catalog_now,
        )
        .await
        .expect("persist Release-level contribution through CatalogStore");

    let sentinel_artist = Uuid::now_v7();
    insert_artist(&db.pool, sentinel_artist, "Unrelated Artist", "person").await;
    let sentinel_track = insert_track(
        &db.pool,
        "Unrelated Projection",
        Some("2000-01-01"),
        "Unrelated Artist",
    )
    .await;
    let sentinel_source = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artist_track_associations (
            artist_id,track_id,association_type,participation_kind,source_key,
            source_artist_id,confidence,is_performance,created_at
         ) VALUES ($1,$2,'primary_artist','direct',$3,NULL,10000,true,$4)",
    )
    .bind(sentinel_artist)
    .bind(sentinel_track)
    .bind(sentinel_source)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("seed unrelated projection sentinel");
    CatalogStore
        .rebuild_artist_track_associations(
            &db.pool,
            &[
                solo,
                group_inside,
                group_outside,
                group_excluded,
                group_unknown_date,
                guest,
                composer,
            ]
            .into_iter()
            .map(TrackId::from_uuid)
            .collect::<Vec<_>>(),
            T0,
        )
        .await
        .expect("set-wise rebuild affected Track participation");
    let sentinel_after: (Uuid, Uuid, Uuid, time::OffsetDateTime) = sqlx::query_as(
        "SELECT artist_id,track_id,source_key,created_at
         FROM artist_track_associations WHERE track_id=$1",
    )
    .bind(sentinel_track)
    .fetch_one(&db.pool)
    .await
    .expect("unrelated projection sentinel survives scoped rebuild");
    assert_eq!(
        sentinel_after,
        (sentinel_artist, sentinel_track, sentinel_source, T0),
        "scoped rebuild must not churn unrelated catalog projection rows"
    );

    let default = CatalogStore
        .list_artist_participation(&db.pool, ArtistId::from_uuid(colin), false, 100)
        .await
        .expect("default performance participation");
    assert_eq!(default.len(), 3);
    let default_tracks = default
        .iter()
        .map(|row| row.track_id.into_uuid())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        default_tracks,
        [solo, group_inside, guest].into_iter().collect()
    );
    assert!(!default_tracks.contains(&group_outside));
    assert!(!default_tracks.contains(&group_excluded));
    assert!(!default_tracks.contains(&group_unknown_date));
    assert!(!default_tracks.contains(&composer));
    let inferred = default
        .iter()
        .find(|row| row.track_id.into_uuid() == group_inside)
        .expect("date-valid group inference");
    assert_eq!(inferred.role, ContributionRole::BandMember);
    assert_eq!(
        inferred.participation,
        ParticipationKind::InferredGroupParticipation {
            source_group_id: ArtistId::from_uuid(men_at_work),
            membership_id,
        }
    );

    let all_credits = CatalogStore
        .list_artist_participation(&db.pool, ArtistId::from_uuid(colin), true, 100)
        .await
        .expect("all-credit participation");
    assert_eq!(all_credits.len(), 4);
    assert!(all_credits.iter().any(|row| {
        row.track_id.into_uuid() == composer && row.role == ContributionRole::Composer
    }));
    let guest_billing: Vec<(String, String)> = sqlx::query_as(
        "SELECT display_value,join_phrase FROM track_artists WHERE track_id=$1 ORDER BY presentation_order",
    )
    .bind(guest)
    .fetch_all(&db.pool)
    .await
    .expect("query structured guest billing");
    assert_eq!(
        guest_billing,
        vec![
            ("Somebody".to_owned(), " feat. ".to_owned()),
            ("Colin Hay".to_owned(), String::new()),
        ]
    );
    let display: (String,) = sqlx::query_as("SELECT display_artist FROM tracks WHERE id=$1")
        .bind(guest)
        .fetch_one(&db.pool)
        .await
        .expect("query exact DisplayArtist");
    assert_eq!(display.0, "Somebody feat. Colin Hay");

    let mut explain_tx = db.pool.begin().await.expect("begin EXPLAIN transaction");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *explain_tx)
        .await
        .expect("make index selection deterministic for plan evidence");
    let explain: Vec<String> = sqlx::query_scalar(
        "EXPLAIN (COSTS OFF)
         SELECT artist_id,track_id,association_type,participation_kind,source_artist_id,source_key
         FROM artist_track_associations
         WHERE artist_id=$1 AND is_performance
         ORDER BY track_id,association_type,source_key LIMIT 100",
    )
    .bind(colin)
    .fetch_all(&mut *explain_tx)
    .await
    .expect("EXPLAIN default participation query");
    assert!(
        explain
            .join("\n")
            .contains("artist_track_associations_default_idx"),
        "default participation query remains backed by its partial index: {explain:?}"
    );
    explain_tx.rollback().await.expect("rollback EXPLAIN only");
    db.drop().await;
}
