//! ADR-0189/0194: import reuse must preserve ambiguity and durable evidence.
#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
#[path = "common/historical_catalog.rs"]
mod historical_catalog;

use catalog_common::{CatalogFixturePresentation, seed_published_release_with_musicbrainz_ids};
use common::{TestDb, test_config};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkerId};
use time::OffsetDateTime;
use uuid::Uuid;

async fn import(db: &TestDb, ordinal: u32, ids: serde_json::Value) -> (Uuid, Uuid) {
    let title = format!("Release {ordinal}");
    let fixture = seed_published_release_with_musicbrainz_ids(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: &title,
            release_artist: "Shared Name",
            track_title: "Track",
            track_artist: "Shared Name",
            release_date: "2024-01-01",
        },
        ids,
    )
    .await;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    CatalogStore
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim");
    let request = CatalogImportRequest::new("reuse-regression-v1", "catalog-test");
    CatalogStore
        .import_manifest_v3(&db.pool, fixture.work_item_id, &worker, &request, now)
        .await
        .expect("import");
    // A retry must not bump revisions or manufacture an additional overlay/outbox.
    CatalogStore
        .import_manifest_v3(&db.pool, fixture.work_item_id, &worker, &request, now)
        .await
        .expect("retry");
    let release_id = fixture.release_id.into_uuid();
    let artist_id = sqlx::query_scalar("SELECT artist_id FROM release_artists WHERE release_id=$1")
        .bind(release_id)
        .fetch_one(&db.pool)
        .await
        .expect("Artist");
    (release_id, artist_id)
}

#[tokio::test]
async fn reuse_counts_disambiguated_homonyms_before_eligibility() {
    let db = TestDb::create("reuse_homonyms").await;
    let (_, first) = import(&db, 201, serde_json::json!({})).await;
    sqlx::query("UPDATE artists SET disambiguation='Distinct person' WHERE id=$1")
        .bind(first)
        .execute(&db.pool)
        .await
        .expect("disambiguate");
    let (_, second) = import(&db, 202, serde_json::json!({})).await;
    let (_, third) = import(&db, 203, serde_json::json!({})).await;
    assert_ne!(first, second);
    assert_ne!(
        second, third,
        "the undisambiguated homonym must not hide the other Artist"
    );
    db.drop().await;
}

#[tokio::test]
async fn embedded_id_requires_existing_evidence_and_multiple_ids_are_one_artist() {
    let db = TestDb::create("reuse_identifiers").await;
    let (_, original) = import(&db, 204, serde_json::json!({})).await;
    let (_, identified) = import(
        &db,
        205,
        serde_json::json!({"artist":"11111111-1111-1111-1111-111111111111"}),
    )
    .await;
    assert_ne!(
        original, identified,
        "missing existing identifier is not matching evidence"
    );
    // Retire the empty-evidence homonym; give the sole live Artist a second valid ID.
    sqlx::query("UPDATE artists SET status='deprecated' WHERE id=$1")
        .bind(original)
        .execute(&db.pool)
        .await
        .expect("retire");
    sqlx::query("INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at) VALUES (uuidv7(),$1,'artist','musicbrainz','22222222-2222-2222-2222-222222222222',false,9000,'test',false,now())")
        .bind(identified).execute(&db.pool).await.expect("second identifier");
    let (_, reused) = import(
        &db,
        206,
        serde_json::json!({"artist":"22222222-2222-2222-2222-222222222222"}),
    )
    .await;
    assert_eq!(
        identified, reused,
        "multiple evidence rows do not create multiple Artist candidates"
    );
    let (_, conflicting) = import(
        &db,
        207,
        serde_json::json!({"artist":"33333333-3333-3333-3333-333333333333"}),
    )
    .await;
    assert_ne!(identified, conflicting);
    db.drop().await;
}

#[tokio::test]
async fn reused_artist_overlay_decisions_and_projection_revisions_agree() {
    let db = TestDb::create("reuse_evidence").await;
    let (_, artist) = import(&db, 208, serde_json::json!({})).await;
    let (release, reused) = import(&db, 209, serde_json::json!({})).await;
    assert_eq!(artist, reused);
    let mismatches: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_semantic_overlays overlay CROSS JOIN LATERAL jsonb_array_elements(overlay.payload->'references'->'artist_resolutions') reference LEFT JOIN artist_match_decisions decision ON decision.id::text=reference->>'decision_id' WHERE overlay.release_id=$1 AND reference->>'artist_id' IS DISTINCT FROM decision.artist_id::text")
        .bind(release).fetch_one(&db.pool).await.expect("overlay correspondence");
    assert_eq!(mismatches, 0);
    let reasons: Vec<String> = sqlx::query_scalar("SELECT decision.reason FROM artist_match_decisions decision JOIN observed_artists observed ON observed.id=decision.observed_artist_id WHERE observed.release_id=$1")
        .bind(release).fetch_all(&db.pool).await.expect("reuse reasons");
    assert!(
        reasons
            .iter()
            .all(|reason| reason.contains("reused the unambiguous existing Artist"))
    );
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM artists WHERE id=$1")
        .bind(artist)
        .fetch_one(&db.pool)
        .await
        .expect("Artist revision");
    assert_eq!(
        revision, 2,
        "exactly one bump for the second import, none for retries"
    );
    let outbox: Vec<i64> = sqlx::query_scalar("SELECT entity_revision FROM catalog_projection_outbox WHERE entity_type='artist' AND entity_id=$1 ORDER BY entity_revision")
        .bind(artist).fetch_all(&db.pool).await.expect("Artist outbox");
    assert_eq!(outbox, [1, 2]);
    db.drop().await;
}

#[tokio::test]
async fn migration_repairs_projection_and_appends_overlay_without_rewriting_history() {
    use melodee_domain::{
        ArtistId, CatalogImportId, CatalogSemanticOverlay, CatalogSemanticOverlayId,
        NewCatalogSemanticOverlay, OverlayReferences, ReleaseId,
    };
    use melodee_persistence::{MIGRATOR, migrate};
    use sqlx::migrate::Migrator;
    use std::borrow::Cow;

    let db = TestDb::create_unmigrated("reuse_upgrade").await;
    Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .migrations
                .iter()
                // The 0045 projection repair is the test subject. Every other
                // migration is additive and independent, and the current
                // claim/policy engine behind the historical fixture driver
                // references the 0071 guard columns, so the whole chain minus
                // the subject must be present before seeding.
                .filter(|migration| migration.version != 45)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
    .run(&db.pool)
    .await
    .expect("pre-repair schema");
    let source = TestDb::create("reuse_fixture_source").await;
    let fixture = historical_catalog::import(&source, &db, 210, "Release 210").await;
    source.drop().await;
    let release = fixture.release_id.into_uuid();
    let artist: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM release_artists WHERE release_id=$1")
            .bind(release)
            .fetch_one(&db.pool)
            .await
            .expect("historical credit");
    let payload: serde_json::Value = sqlx::query_scalar("SELECT overlay.payload FROM catalog_semantic_overlays overlay JOIN releases release ON release.current_overlay_id=overlay.id WHERE release.id=$1")
        .bind(release).fetch_one(&db.pool).await.expect("original overlay");
    let mut references: OverlayReferences =
        serde_json::from_value(payload["references"].clone()).expect("references");
    for resolution in &mut references.artist_resolutions {
        resolution.artist_id = ArtistId::new();
    }
    let broken = CatalogSemanticOverlay::new(NewCatalogSemanticOverlay {
        id: CatalogSemanticOverlayId::new(),
        release_id: ReleaseId::from_uuid(release),
        source_manifest_id: serde_json::from_value(payload["source_manifest_id"].clone())
            .expect("manifest"),
        source_semantic_manifest_hash: payload["source_semantic_manifest_hash"]
            .as_str()
            .expect("source hash")
            .to_owned(),
        publication_plan_hash: payload["publication_plan_hash"]
            .as_str()
            .expect("plan hash")
            .to_owned(),
        catalog_import_id: CatalogImportId::from_uuid(
            serde_json::from_value(payload["catalog_import_id"].clone()).expect("import"),
        ),
        resolver_version: "old-importer".to_owned(),
        predecessor_id: Some(serde_json::from_value(payload["id"].clone()).expect("predecessor")),
        references,
        created_at: OffsetDateTime::now_utc(),
        created_by: "historical-fixture".to_owned(),
    })
    .expect("historical overlay shape");
    let broken_payload = serde_json::to_value(&broken).expect("serialize overlay");
    sqlx::query("INSERT INTO catalog_semantic_overlays (id,release_id,catalog_import_id,predecessor_overlay_id,schema_name,schema_version,source_manifest_id,source_semantic_manifest_hash,publication_plan_hash,resolver_version,payload,overlay_hash,created_at) VALUES ($1,$2,$3,$4,'catalog.semantic',1,$5,$6,$7,$8,$9,$10,$11)")
        .bind(broken.id().into_uuid()).bind(release).bind(broken.catalog_import_id().into_uuid())
        .bind(broken.predecessor_id().map(|id| id.into_uuid())).bind(broken.source_manifest_id())
        .bind(broken.source_semantic_manifest_hash()).bind(broken.publication_plan_hash()).bind(broken.resolver_version())
        .bind(&broken_payload).bind(broken.overlay_hash().expect("overlay hash")).bind(broken.created_at())
        .execute(&db.pool).await.expect("seed old defective evidence");
    sqlx::query("UPDATE releases SET current_overlay_id=$2 WHERE id=$1")
        .bind(release)
        .bind(broken.id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("historical pointer");
    // The derived-state repair runs at the end of `migrate()`, so the injected
    // failure trigger must exist before the migration that seeds the repair
    // queue. A failed repair retains the queue row while applied migration
    // history stays intact and the retry happens with the next startup.
    sqlx::raw_sql(sqlx::AssertSqlSafe("CREATE FUNCTION reject_repair_for_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected repair failure'; END $$; CREATE TRIGGER reject_repair_for_test BEFORE INSERT ON catalog_semantic_overlays FOR EACH ROW EXECUTE FUNCTION reject_repair_for_test();"))
        .execute(&db.pool).await.expect("inject repair failure");
    migrate(&db.pool)
        .await
        .expect_err("repair failure prevents startup success");
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_overlay_repair_queue")
        .fetch_one(&db.pool)
        .await
        .expect("pending queue");
    assert_eq!(remaining, 1, "failed batch retains its retry intent");
    let unchanged: Uuid = sqlx::query_scalar("SELECT current_overlay_id FROM releases WHERE id=$1")
        .bind(release)
        .fetch_one(&db.pool)
        .await
        .expect("old pointer");
    assert_eq!(unchanged, broken.id().into_uuid());
    sqlx::raw_sql(sqlx::AssertSqlSafe("DROP TRIGGER reject_repair_for_test ON catalog_semantic_overlays; DROP FUNCTION reject_repair_for_test();"))
        .execute(&db.pool).await.expect("clear failure injection");
    migrate(&db.pool).await.expect("upgrade repair");
    let current: (Uuid, serde_json::Value)=sqlx::query_as("SELECT overlay.predecessor_overlay_id, overlay.payload FROM releases release JOIN catalog_semantic_overlays overlay ON overlay.id=release.current_overlay_id WHERE release.id=$1")
        .bind(release).fetch_one(&db.pool).await.expect("current evidence");
    assert_eq!(current.0, broken.id().into_uuid());
    let repaired: OverlayReferences =
        serde_json::from_value(current.1["references"].clone()).expect("repaired references");
    assert!(
        repaired
            .artist_resolutions
            .iter()
            .all(|resolution| resolution.artist_id.into_uuid() == artist)
    );
    let retained: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM catalog_semantic_overlays WHERE id=$1")
            .bind(broken.id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("historical evidence");
    assert_eq!(retained, broken_payload);
    let projection: (i64,i64)=sqlx::query_as("SELECT artist.revision,max(outbox.entity_revision) FROM artists artist JOIN catalog_projection_outbox outbox ON outbox.entity_id=artist.id AND outbox.entity_type='artist' WHERE artist.id=$1 GROUP BY artist.id")
        .bind(artist).fetch_one(&db.pool).await.expect("upgrade projection");
    assert_eq!(projection, (2, 2));
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_semantic_overlays")
        .fetch_one(&db.pool)
        .await
        .expect("overlays");
    migrate(&db.pool).await.expect("repeat upgrade");
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_semantic_overlays")
        .fetch_one(&db.pool)
        .await
        .expect("overlays after retry");
    assert_eq!(before, after);
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_overlay_repair_queue")
        .fetch_one(&db.pool)
        .await
        .expect("queue drained");
    assert_eq!(queued, 0);
    db.drop().await;
}
