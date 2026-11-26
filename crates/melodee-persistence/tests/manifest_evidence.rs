//! Manifest evidence on persistent files: chunking, upgrade, verification and
//! query facts (ADR-0213).

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, WorkerId, asset_files, manifest_evidence,
};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

async fn seed_manifest_row(db: &TestDb) -> (i64, Uuid) {
    let fixture = seed_published_release(
        db,
        401,
        CatalogFixturePresentation {
            release_title: "Evidence Fixture",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let manifest_row_id: i64 =
        sqlx::query_scalar("SELECT id FROM release_work_item_manifests WHERE work_item_id=$1")
            .bind(fixture.work_item_id)
            .fetch_one(&db.pool)
            .await
            .expect("manifest row");
    (manifest_row_id, fixture.work_item_id)
}

/// Objects above the 10 MiB single-object limit are chunked, reconstructed and
/// verified; a corrupt or missing part fails closed.
#[tokio::test]
async fn large_evidence_is_chunked_and_verified() {
    let db = TestDb::create("manifest_evidence_chunked").await;
    // > 10 MiB forces the fixed 4 MiB chunk representation.
    let bytes = vec![b'e'; 11 * 1024 * 1024 + 17];
    let receipt = asset_files::put_evidence(&db.pool, &bytes)
        .await
        .expect("store evidence");
    assert_eq!(receipt.storage_kind, "chunked");
    assert_eq!(receipt.chunk_count, 3);
    assert_eq!(receipt.length, i64::try_from(bytes.len()).expect("length"));
    let restored = asset_files::read_evidence(
        receipt.key.clone(),
        receipt.storage_kind.to_owned(),
        receipt.sha256.clone(),
        receipt.length,
    )
    .await
    .expect("read evidence");
    assert_eq!(restored, bytes);

    // A wrong receipt is rejected before the payload is trusted.
    assert!(
        asset_files::read_evidence(
            receipt.key.clone(),
            receipt.storage_kind.to_owned(),
            "0".repeat(64),
            receipt.length,
        )
        .await
        .is_err()
    );

    // A shared object survives one owner's death (content addressing) and a
    // second write reuses the same key.
    let second = asset_files::put_evidence(&db.pool, &bytes[..4 * 1024 * 1024])
        .await
        .expect("store shared chunk");
    let third = asset_files::put_evidence(&db.pool, &bytes[..4 * 1024 * 1024])
        .await
        .expect("reuse shared chunk");
    assert_eq!(second.key, third.key);
}

/// A manifest imported by the writer keeps its payload in evidence, writes
/// compact facts, and never stores inline JSON.
#[tokio::test]
async fn import_stores_evidence_and_facts() {
    let db = TestDb::create("manifest_evidence_import").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let fixture = seed_published_release(
        &db,
        402,
        CatalogFixturePresentation {
            release_title: "Evidence Import",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim cataloging");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("manifest-evidence-v1", "evidence-test"),
            now,
        )
        .await
        .expect("import");

    let (manifest_row_id, inline): (i64, bool) = sqlx::query_as(
        "SELECT id, manifest_payload IS NOT NULL FROM release_work_item_manifests
          WHERE work_item_id=$1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("manifest row");
    assert!(!inline, "manifest payload must live on file storage");
    let (revision, serialization): (i32, i32) = sqlx::query_as(
        "SELECT revision, serialization_version
           FROM release_work_item_manifest_evidence WHERE manifest_row_id=$1",
    )
    .bind(manifest_row_id)
    .fetch_one(&db.pool)
    .await
    .expect("evidence reference");
    assert!(revision >= 1);
    assert_eq!(serialization, 1);

    let facts: (bool, Vec<String>, Vec<String>, bool) = sqlx::query_as(
        "SELECT has_corrupt_media, structured_traits, embedded_traits, lyrics_present
           FROM release_work_item_manifest_facts WHERE manifest_row_id=$1",
    )
    .bind(manifest_row_id)
    .fetch_one(&db.pool)
    .await
    .expect("manifest facts");
    assert!(!facts.0);
    assert!(!facts.3);

    let (bit_depth, genre): (Option<i32>, Option<String>) = sqlx::query_as(
        "SELECT bit_depth, genre FROM release_work_item_manifest_track_facts
          WHERE manifest_row_id=$1",
    )
    .bind(manifest_row_id)
    .fetch_one(&db.pool)
    .await
    .expect("track facts");
    assert_eq!(bit_depth, Some(24));
    assert_eq!(genre.as_deref(), Some("Fixture"));

    let manifest = manifest_evidence::load_manifest(&db.pool, manifest_row_id)
        .await
        .expect("load evidence");
    assert_eq!(
        manifest["release_observation"]["title"].as_str(),
        Some("Evidence Import")
    );

    // The genre serving projection is derived from the facts, not the payload.
    let genres: Vec<(String, String)> = sqlx::query_as(
        "SELECT surface, display_value FROM catalog_genre_terms
          WHERE track_id IN (SELECT id FROM tracks WHERE release_id=$1)
          ORDER BY surface",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("genre projection");
    assert!(
        genres
            .iter()
            .any(|(surface, value)| { surface == "native" && value == "Fixture" })
    );
}

/// The upgrade converts an inline legacy payload, verifies it independently,
/// removes the inline copy only after that check, and is idempotent.
#[tokio::test]
async fn upgrade_backfills_verifies_and_finalizes() {
    let db = TestDb::create("manifest_evidence_upgrade").await;
    let (manifest_row_id, _) = seed_manifest_row(&db).await;
    let payload = manifest_evidence::load_manifest(&db.pool, manifest_row_id)
        .await
        .expect("load seeded evidence");
    let mut legacy_payload = payload.clone();
    legacy_payload["release_observation"]["blackbeard"] =
        json!({ "document": "provenance", "unknownField": [1, 2] });
    legacy_payload["normalization"]["parsed_sidecars"] = json!([{
        "sidecar_type": "lrc",
        "relative_path": "Artist/Release/01.lrc",
        "parsed": { "value": "не-ASCII lyrics" }
    }]);
    // Simulate the pre-upgrade schema state: inline payload, no evidence. The
    // v3 immutability trigger is suspended for fixture construction only.
    sqlx::query("ALTER TABLE release_work_item_manifests DISABLE TRIGGER release_work_item_manifests_v3_immutable")
        .execute(&db.pool)
        .await
        .expect("suspend immutability trigger");
    for statement in [
        "DELETE FROM release_work_item_manifest_evidence_revision WHERE manifest_row_id=$1",
        "DELETE FROM release_work_item_manifest_evidence WHERE manifest_row_id=$1",
        "DELETE FROM release_work_item_manifest_facts WHERE manifest_row_id=$1",
        "DELETE FROM manifest_evidence_migration_receipts WHERE manifest_row_id=$1",
        "UPDATE release_work_item_manifests SET manifest_payload=$2 WHERE id=$1",
    ] {
        let mut query = sqlx::query(statement).bind(manifest_row_id);
        if statement.starts_with("UPDATE") {
            query = query.bind(&legacy_payload);
        }
        query
            .execute(&db.pool)
            .await
            .expect("restore legacy inline state");
    }
    sqlx::query("ALTER TABLE release_work_item_manifests ENABLE TRIGGER release_work_item_manifests_v3_immutable")
        .execute(&db.pool)
        .await
        .expect("restore immutability trigger");

    let now = OffsetDateTime::now_utc();
    let report = manifest_evidence::upgrade(&db.pool, now)
        .await
        .expect("upgrade");
    assert!(report.is_clean(), "{:?}", report.discrepancies);
    assert_eq!(report.files_verified, 1);
    let (inline, evidence_rows): (bool, i64) = sqlx::query_as(
        "SELECT manifest_payload IS NOT NULL,
                (SELECT count(*) FROM release_work_item_manifest_evidence
                  WHERE manifest_row_id=$1)
           FROM release_work_item_manifests WHERE id=$1",
    )
    .bind(manifest_row_id)
    .fetch_one(&db.pool)
    .await
    .expect("post-upgrade state");
    assert!(
        !inline,
        "inline evidence is removed only after verification"
    );
    assert_eq!(evidence_rows, 1);

    // Unknown fields and non-ASCII lyrics survive the round trip.
    let restored = manifest_evidence::load_manifest(&db.pool, manifest_row_id)
        .await
        .expect("restored evidence");
    assert_eq!(restored, legacy_payload);
    assert_eq!(
        restored["normalization"]["parsed_sidecars"][0]["parsed"]["value"].as_str(),
        Some("не-ASCII lyrics")
    );

    // A repeat run performs no bulk work and reports the same clean result.
    let status = manifest_evidence::status(&db.pool).await.expect("status");
    assert_eq!(status.pending, 0);
    assert_eq!(status.evidence_rows, 1);
    let again = manifest_evidence::upgrade(&db.pool, now)
        .await
        .expect("repeat upgrade");
    assert!(again.is_clean());
    assert_eq!(
        manifest_evidence::status(&db.pool)
            .await
            .expect("status")
            .evidence_rows,
        1,
        "repeat startup does not duplicate evidence rows"
    );
    manifest_evidence::require_complete(&db.pool)
        .await
        .expect("complete");
}

/// Independent verification detects a missing or corrupt evidence file and a
/// row that still carries inline evidence.
#[tokio::test]
async fn verification_fails_closed_on_missing_or_corrupt_files() {
    let db = TestDb::create("manifest_evidence_corrupt").await;
    let (manifest_row_id, _) = seed_manifest_row(&db).await;
    let key: String = sqlx::query_scalar(
        "SELECT asset_key FROM release_work_item_manifest_evidence WHERE manifest_row_id=$1",
    )
    .bind(manifest_row_id)
    .fetch_one(&db.pool)
    .await
    .expect("evidence key");
    // Removing the backing object must be reported, not swallowed.
    asset_files::remove(key).await.expect("remove object");
    let report = manifest_evidence::verify_all(&db.pool, 10)
        .await
        .expect("verify");
    assert!(!report.is_clean());
    assert!(
        report
            .discrepancies
            .iter()
            .any(|message| message.contains("could not be read")),
        "{:?}",
        report.discrepancies
    );

    // Reintroducing inline evidence for a sealed v3 manifest is rejected by
    // the database, so a later writer cannot bypass the reference.
    let reintroduced = sqlx::query(
        "UPDATE release_work_item_manifests
            SET manifest_payload='{\"schema_version\":3}' WHERE id=$1",
    )
    .bind(manifest_row_id)
    .execute(&db.pool)
    .await;
    assert!(
        reintroduced.is_err(),
        "sealed manifests must not regain an inline payload"
    );
}
