//! Phase 4 migration and cross-release integrity proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{T0, TestDb, test_config};
use melodee_domain::{
    ArtistId, ArtistMatchEvidence, ArtistMatchEvidenceId, Confidence, EvidenceKind,
    EvidencePolarity, IngestionState, ObservedArtistId,
};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkItemStore, WorkerId};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn catalog_schema_rejects_identity_state_and_cross_release_corruption() {
    let db = TestDb::create("catalog_schema_integrity").await;

    for invalid_internal_id in [
        Uuid::nil(),
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").expect("valid UUIDv4 fixture"),
    ] {
        assert!(
            sqlx::query(
                "INSERT INTO artists (
                    id,public_api_id,canonical_name,normalized_name,sort_name,
                    artist_type,status,created_at,updated_at
                 ) VALUES ($1,$2,'Invalid internal ID','INVALIDINTERNALID','Invalid internal ID',
                           'unknown','verified',$3,$3)",
            )
            .bind(invalid_internal_id)
            .bind(Uuid::max())
            .bind(T0)
            .execute(&db.pool)
            .await
            .is_err(),
            "nil and UUIDv4 internal identities must both fail closed"
        );
    }
    let stable_artist_id = Uuid::now_v7();
    let stable_public_id =
        Uuid::parse_str("813c11e0-61cd-5204-ae0f-ce68386eacec").expect("valid frozen public UUID");
    sqlx::query(
        "INSERT INTO artists (
            id,public_api_id,canonical_name,normalized_name,sort_name,
            artist_type,status,created_at,updated_at
         ) VALUES ($1,$2,'Stable public ID','STABLEPUBLICID','Stable public ID',
                   'unknown','verified',$3,$3)",
    )
    .bind(stable_artist_id)
    .bind(stable_public_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("insert stable public identity fixture");
    assert!(
        sqlx::query("UPDATE artists SET public_api_id=$2 WHERE id=$1")
            .bind(stable_artist_id)
            .bind(Uuid::parse_str("5bf951d5-7afe-5c50-8941-145b7095902b").expect("valid UUID"))
            .execute(&db.pool)
            .await
            .is_err(),
        "public identity is immutable"
    );

    let first = seed_published_release(
        &db,
        81,
        CatalogFixturePresentation {
            release_title: "Schema One",
            release_artist: "Schema Artist One",
            track_title: "Schema Track One",
            track_artist: "Schema Artist One",
            release_date: "2001-01-01",
        },
    )
    .await;
    let second = seed_published_release(
        &db,
        82,
        CatalogFixturePresentation {
            release_title: "Schema Two",
            release_artist: "Schema Artist Two",
            track_title: "Schema Track Two",
            track_artist: "Schema Artist Two",
            release_date: "2002-02-02",
        },
    )
    .await;
    assert!(
        WorkItemStore
            .system_transition(
                &db.pool,
                first.work_item_id,
                IngestionState::Cataloging,
                Some("forbidden generic bypass"),
                OffsetDateTime::now_utc(),
            )
            .await
            .is_err(),
        "only the fenced catalog claim path may enter Cataloging"
    );

    let store = CatalogStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim schema fixtures");
    assert_eq!(claimed.len(), 2);
    for fixture in [&first, &second] {
        store
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("schema-integrity-v1", "schema-test"),
                now,
            )
            .await
            .expect("import schema fixture");
    }

    let playable_asset_counts: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT count(*),bool_and(track.playable_media_asset_id=asset.media_asset_id)
         FROM tracks track
         JOIN track_media_assets asset
           ON asset.track_id=track.id AND asset.purpose='playable'
         GROUP BY track.id ORDER BY track.id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("query importer-created playable MediaAssets");
    assert_eq!(
        playable_asset_counts,
        vec![(1, true), (1, true)],
        "the importer binds every Track to its one authoritative playable asset"
    );
    let declarative_playable_invariant: (bool, bool, bool, bool, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT attnotnull FROM pg_attribute
             WHERE attrelid='tracks'::regclass AND attname='playable_media_asset_id'),
            (SELECT attgenerated='s' FROM pg_attribute
             WHERE attrelid='tracks'::regclass AND attname='playable_purpose'),
            (SELECT condeferrable FROM pg_constraint
             WHERE conname='tracks_exactly_one_playable_asset_fk'),
            (SELECT condeferred FROM pg_constraint
             WHERE conname='tracks_exactly_one_playable_asset_fk'),
            (SELECT count(*) FROM pg_trigger
             WHERE tgrelid IN ('tracks'::regclass,'track_media_assets'::regclass)
               AND NOT tgisinternal AND tgname LIKE '%playable%'),
            (SELECT count(*) FROM pg_proc
             WHERE proname='validate_track_playable_media_asset')",
    )
    .fetch_one(&db.pool)
    .await
    .expect("inspect declarative playable-asset invariant");
    assert_eq!(
        declarative_playable_invariant,
        (true, true, true, true, 0, 0),
        "the invariant is a NOT NULL/generated/deferred-FK design, not row-trigger code"
    );
    let playable_fk_definition: String = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint
         WHERE conname='tracks_exactly_one_playable_asset_fk'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("query exact playable composite FK");
    assert!(playable_fk_definition.contains("playable_media_asset_id"));
    assert!(playable_fk_definition.contains("playable_purpose"));
    assert!(playable_fk_definition.contains("track_media_assets"));
    let first_track: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(first.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("query first Track for deferred asset invariant");
    let mut assetless_track = db
        .pool
        .begin()
        .await
        .expect("begin assetless Track attempt");
    let delete_playable =
        sqlx::query("DELETE FROM track_media_assets WHERE track_id=$1 AND purpose='playable'")
            .bind(first_track)
            .execute(&mut *assetless_track)
            .await;
    assert!(
        delete_playable.is_err(),
        "the declarative RESTRICT edge prevents removing a Track's mandatory playable asset"
    );
    assetless_track
        .rollback()
        .await
        .expect("roll back rejected playable-asset deletion");
    let playable_after_rejected_commit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM track_media_assets WHERE track_id=$1 AND purpose='playable'",
    )
    .bind(first_track)
    .fetch_one(&db.pool)
    .await
    .expect("failed commit preserves playable MediaAsset");
    assert_eq!(playable_after_rejected_commit, 1);

    let complete_policy_lease = sqlx::query(
        "UPDATE release_work_items
         SET lease_owner='policy-worker',lease_acquired_at=$2,lease_expires_at=$3
         WHERE id=$1",
    )
    .bind(first.work_item_id)
    .bind(now)
    .bind(now + Duration::seconds(60))
    .execute(&db.pool)
    .await
    .expect("Cataloged work permits a complete Phase 5 policy lease");
    assert_eq!(complete_policy_lease.rows_affected(), 1);
    assert!(
        sqlx::query("UPDATE release_work_items SET lease_expires_at=NULL WHERE id=$1",)
            .bind(first.work_item_id)
            .execute(&db.pool)
            .await
            .is_err(),
        "Cataloged work rejects a partial lease tuple"
    );
    sqlx::query(
        "UPDATE release_work_items
         SET lease_owner=NULL,lease_acquired_at=NULL,lease_expires_at=NULL
         WHERE id=$1",
    )
    .bind(first.work_item_id)
    .execute(&db.pool)
    .await
    .expect("Cataloged work also permits the wholly unleased form");

    let first_overlay: Uuid =
        sqlx::query_scalar("SELECT current_overlay_id FROM releases WHERE id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query first overlay");
    assert!(
        sqlx::query("UPDATE catalog_semantic_overlays SET actor='rewrite' WHERE id=$1")
            .bind(first_overlay)
            .execute(&db.pool)
            .await
            .is_err(),
        "semantic overlays are append-only"
    );
    assert!(
        sqlx::query("UPDATE releases SET current_overlay_id=$2 WHERE id=$1")
            .bind(second.release_id.into_uuid())
            .bind(first_overlay)
            .execute(&db.pool)
            .await
            .is_err(),
        "an overlay pointer cannot cross Release scope"
    );

    let first_observation: Uuid = sqlx::query_scalar(
        "SELECT id FROM observed_artists WHERE release_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(first.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query first observation");
    let first_candidate: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM release_artists WHERE release_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(first.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query first candidate");
    let evidence_id = ArtistMatchEvidenceId::new();
    store
        .record_artist_match_evidence(
            &db.pool,
            &ArtistMatchEvidence {
                id: evidence_id,
                observed_artist_id: ObservedArtistId::from_uuid(first_observation),
                candidate_artist_id: ArtistId::from_uuid(first_candidate),
                kind: EvidenceKind::ReleaseContext,
                polarity: EvidencePolarity::Supports,
                confidence: Confidence::new(7_500).expect("valid confidence"),
                source: "schema-fixture".to_owned(),
            },
            serde_json::json!({"fixture": true}),
            now,
        )
        .await
        .expect("append identity evidence");
    assert!(
        sqlx::query("UPDATE artist_match_evidence SET source='rewrite' WHERE id=$1")
            .bind(evidence_id.into_uuid())
            .execute(&db.pool)
            .await
            .is_err(),
        "identity evidence is append-only"
    );

    let second_track: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(second.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("query second Track");
    assert!(
        sqlx::query(
            "INSERT INTO track_artists (
                id,track_id,artist_id,observed_artist_id,role,presentation_order,
                display_value,join_phrase,created_at
             ) VALUES ($1,$2,$3,$4,'other',99,'cross-release','',$5)",
        )
        .bind(Uuid::now_v7())
        .bind(second_track)
        .bind(first_candidate)
        .bind(first_observation)
        .bind(now)
        .execute(&db.pool)
        .await
        .is_err(),
        "an observation cannot be attached to a Track from another Release"
    );

    db.drop().await;
}
