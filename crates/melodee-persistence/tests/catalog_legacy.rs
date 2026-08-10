//! Frozen public-ID migration and set-based reconciliation proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::{BTreeSet, HashMap};

use catalog_common::legacy::{ARTIST_PUBLIC_IDS, RELEASE_PUBLIC_IDS, TRACK_PUBLIC_IDS};
use common::{T0, TestDb};
use melodee_domain::{
    ArtistId, ArtistRelationship, ArtistRelationshipId, ArtistRelationshipKind, Confidence,
};
use melodee_persistence::{
    CatalogStore, CatalogStoreError, LegacyCatalogEntityType, LegacyCatalogIdMapping,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenCatalog {
    artists: Vec<FrozenArtist>,
    artist_relations: Vec<FrozenArtistRelation>,
}

#[derive(Debug, Deserialize)]
struct FrozenArtist {
    slug: String,
    albums: Vec<FrozenRelease>,
}

#[derive(Debug, Deserialize)]
struct FrozenRelease {
    slug: String,
    songs: Vec<FrozenTrack>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenTrack {
    slug: String,
    song_number: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenArtistRelation {
    artist: String,
    related_artist: String,
    #[serde(rename = "type")]
    relation_type: i32,
}

fn parse_public_id(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("frozen oracle public UUID is valid")
}

#[tokio::test]
async fn all_frozen_artist_release_and_track_public_ids_roundtrip_set_wise() {
    let db = TestDb::create("catalog_legacy_public_ids").await;
    let frozen: FrozenCatalog = serde_json::from_str(include_str!(
        "../../../compatibility/fixtures/catalog-v1.json"
    ))
    .expect("parse committed frozen compatibility catalog");
    assert_eq!(frozen.artists.len(), ARTIST_PUBLIC_IDS.len());
    let migration_run_id = Uuid::now_v7();
    let store = CatalogStore;
    let mut mappings = Vec::with_capacity(
        ARTIST_PUBLIC_IDS.len() + RELEASE_PUBLIC_IDS.len() + TRACK_PUBLIC_IDS.len(),
    );
    let mut artist_ids = HashMap::new();

    for (index, ((slug, public_id), frozen_artist)) in
        ARTIST_PUBLIC_IDS.iter().zip(&frozen.artists).enumerate()
    {
        assert_eq!(*slug, frozen_artist.slug);
        let internal_id = Uuid::now_v7();
        let public_id = parse_public_id(public_id);
        sqlx::query(
            "INSERT INTO artists (
                id,public_api_id,canonical_name,normalized_name,sort_name,
                artist_type,status,created_at,updated_at
             ) VALUES ($1,$2,$3,upper(replace($3,'-','')),$3,'unknown','verified',$4,$4)",
        )
        .bind(internal_id)
        .bind(public_id)
        .bind(slug)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("seed legacy Artist target");
        artist_ids.insert((*slug).to_owned(), internal_id);
        mappings.push(LegacyCatalogIdMapping {
            migration_run_id,
            entity_type: LegacyCatalogEntityType::Artist,
            legacy_numeric_id: i64::try_from(index + 1).expect("Artist fixture index fits i64"),
            internal_id,
            public_api_id: public_id,
            source_snapshot: "phase0-catalog-v1".to_owned(),
            imported_at: T0,
        });
    }

    for relation in &frozen.artist_relations {
        assert_eq!(relation.relation_type, 2, "known frozen relation type");
        store
            .add_artist_relationship(
                &db.pool,
                &ArtistRelationship {
                    id: ArtistRelationshipId::new(),
                    subject_artist_id: ArtistId::from_uuid(artist_ids[&relation.artist]),
                    object_artist_id: ArtistId::from_uuid(artist_ids[&relation.related_artist]),
                    kind: ArtistRelationshipKind::new(format!(
                        "legacy_type_{}",
                        relation.relation_type
                    ))
                    .expect("valid legacy relationship kind"),
                    active_dates: None,
                    confidence: Confidence::new(10_000).expect("valid confidence"),
                    evidence_source: "phase0-catalog-v1".to_owned(),
                },
                T0,
            )
            .await
            .expect("preserve frozen Artist relationship");
    }

    let frozen_releases = frozen
        .artists
        .iter()
        .flat_map(|artist| artist.albums.iter())
        .collect::<Vec<_>>();
    assert_eq!(frozen_releases.len(), RELEASE_PUBLIC_IDS.len());
    let mut release_ids = HashMap::new();
    for (index, ((slug, public_id), frozen_release)) in
        RELEASE_PUBLIC_IDS.iter().zip(&frozen_releases).enumerate()
    {
        assert_eq!(*slug, frozen_release.slug);
        let internal_id = Uuid::now_v7();
        let public_id = parse_public_id(public_id);
        sqlx::query(
            "INSERT INTO releases (
                id,public_api_id,title,normalized_title,sort_title,display_artist,
                created_at,updated_at
             ) VALUES ($1,$2,$3,upper(replace($3,'-','')),$3,'Legacy fixture',$4,$4)",
        )
        .bind(internal_id)
        .bind(public_id)
        .bind(slug)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("seed legacy Release target");
        release_ids.insert((*slug).to_owned(), internal_id);
        mappings.push(LegacyCatalogIdMapping {
            migration_run_id,
            entity_type: LegacyCatalogEntityType::Release,
            legacy_numeric_id: i64::try_from(index + 1).expect("Release fixture index fits i64"),
            internal_id,
            public_api_id: public_id,
            source_snapshot: "phase0-catalog-v1".to_owned(),
            imported_at: T0,
        });
    }

    let frozen_tracks = frozen_releases
        .iter()
        .flat_map(|release| {
            release
                .songs
                .iter()
                .map(move |track| (release.slug.as_str(), track))
        })
        .collect::<Vec<_>>();
    assert_eq!(frozen_tracks.len(), TRACK_PUBLIC_IDS.len());
    let mut expected_release_tracks = BTreeSet::new();
    for (index, ((slug, public_id), (release_slug, frozen_track))) in
        TRACK_PUBLIC_IDS.iter().zip(&frozen_tracks).enumerate()
    {
        assert_eq!(*slug, frozen_track.slug);
        let internal_id = Uuid::now_v7();
        let public_id = parse_public_id(public_id);
        let audio_identity_id = Uuid::now_v7();
        let release_id = release_ids[*release_slug];
        sqlx::query(
            "INSERT INTO audio_identities (id,status,created_at,updated_at)
             VALUES ($1,'provisional',$2,$2)",
        )
        .bind(audio_identity_id)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("seed legacy AudioIdentity dependency");
        let media_asset_id = Uuid::now_v7();
        let mut track_tx = db
            .pool
            .begin()
            .await
            .expect("begin complete legacy Track fixture");
        sqlx::query(
            "INSERT INTO tracks (
                id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,
                normalized_title,sort_title,display_artist,created_at,updated_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,upper(replace($7,'-','')),$7,
                       'Legacy fixture',$8,$8)",
        )
        .bind(internal_id)
        .bind(public_id)
        .bind(release_id)
        .bind(audio_identity_id)
        .bind(media_asset_id)
        .bind(frozen_track.song_number)
        .bind(slug)
        .bind(T0)
        .execute(&mut *track_tx)
        .await
        .expect("seed legacy Track target");
        sqlx::query(
            "INSERT INTO media_assets (
                id,canonical_relative_path,byte_hash,byte_size,mime_type,created_at
             ) VALUES ($1,$2,$3,42,'audio/mpeg',$4)",
        )
        .bind(media_asset_id)
        .bind(format!("legacy-fixture/{internal_id}.mp3"))
        .bind("b".repeat(64))
        .bind(T0)
        .execute(&mut *track_tx)
        .await
        .expect("seed legacy playable MediaAsset");
        sqlx::query(
            "INSERT INTO track_media_assets (
                track_id,media_asset_id,purpose,ordinal,created_at
             ) VALUES ($1,$2,'playable',1,$3)",
        )
        .bind(internal_id)
        .bind(media_asset_id)
        .bind(T0)
        .execute(&mut *track_tx)
        .await
        .expect("attach legacy playable MediaAsset");
        track_tx
            .commit()
            .await
            .expect("commit complete legacy Track fixture");
        expected_release_tracks.insert((release_id, internal_id));
        mappings.push(LegacyCatalogIdMapping {
            migration_run_id,
            entity_type: LegacyCatalogEntityType::Track,
            legacy_numeric_id: i64::try_from(index + 1).expect("Track fixture index fits i64"),
            internal_id,
            public_api_id: public_id,
            source_snapshot: "phase0-catalog-v1".to_owned(),
            imported_at: T0,
        });
    }

    store
        .record_legacy_catalog_id_mappings(&db.pool, &mappings)
        .await
        .expect("record all frozen mappings in one bounded call");
    store
        .record_legacy_catalog_id_mappings(&db.pool, &mappings)
        .await
        .expect("identical set-based retry is idempotent");

    let stored: Vec<(String, i64, Uuid, Uuid)> = sqlx::query_as(
        "SELECT entity_type,legacy_numeric_id,internal_id,public_api_id
         FROM legacy_catalog_id_mappings
         WHERE migration_run_id=$1 ORDER BY entity_type,legacy_numeric_id",
    )
    .bind(migration_run_id)
    .fetch_all(&db.pool)
    .await
    .expect("roundtrip frozen mappings");
    assert_eq!(stored.len(), 39);
    assert_eq!(
        stored.iter().map(|row| row.3).collect::<BTreeSet<_>>(),
        ARTIST_PUBLIC_IDS
            .iter()
            .chain(RELEASE_PUBLIC_IDS.iter())
            .chain(TRACK_PUBLIC_IDS.iter())
            .map(|(_, value)| parse_public_id(value))
            .collect()
    );
    assert_eq!(
        stored.iter().map(|row| row.2).collect::<BTreeSet<_>>(),
        mappings.iter().map(|mapping| mapping.internal_id).collect()
    );
    let stored_release_tracks: BTreeSet<(Uuid, Uuid)> =
        sqlx::query_as("SELECT release_id,id FROM tracks")
            .fetch_all(&db.pool)
            .await
            .expect("query reconciled Release-to-Track ownership")
            .into_iter()
            .collect();
    assert_eq!(stored_release_tracks, expected_release_tracks);
    let stored_relations: BTreeSet<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT source_artist_id,target_artist_id FROM artist_relationships
         WHERE evidence_source='phase0-catalog-v1'",
    )
    .fetch_all(&db.pool)
    .await
    .expect("query reconciled Artist relationships")
    .into_iter()
    .collect();
    assert_eq!(
        stored_relations,
        frozen
            .artist_relations
            .iter()
            .map(|relation| {
                (
                    artist_ids[&relation.artist],
                    artist_ids[&relation.related_artist],
                )
            })
            .collect()
    );

    let mut conflicting = mappings[0].clone();
    conflicting.internal_id = mappings[1].internal_id;
    conflicting.public_api_id = mappings[1].public_api_id;
    assert!(matches!(
        store
            .record_legacy_catalog_id_mappings(&db.pool, &[conflicting])
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let final_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM legacy_catalog_id_mappings WHERE migration_run_id=$1",
    )
    .bind(migration_run_id)
    .fetch_one(&db.pool)
    .await
    .expect("conflict leaves mapping set intact");
    assert_eq!(final_count, 39);

    db.drop().await;
}
