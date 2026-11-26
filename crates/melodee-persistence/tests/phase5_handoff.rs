//! acceptance at the immutable boundary.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use melodee_domain::IngestionState;
use melodee_persistence::{PolicyStore, PolicyStoreError, WorkItemStore, WorkerId};
use phase5_common::seed_cataloged_release;

#[tokio::test]
async fn cataloged_policy_input_exists_without_readable_source_or_canonical_media() {
    let db = TestDb::create("phase5_catalog_handoff").await;
    let fixture = seed_cataloged_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Greatest Hits",
            release_artist: "Phase 5 Fixture Artist",
            track_title: "Known Song",
            track_artist: "Phase 5 Fixture Artist",
            release_date: "2005-06-07",
        },
    )
    .await;

    let durable_facts: (String, String, String, String) = sqlx::query_as(
        "SELECT w.state, m.manifest_hash, p.sealed_manifest_hash, ma.byte_hash
           FROM release_work_items w
           JOIN release_work_item_manifests m ON m.work_item_id = w.id
           JOIN release_publication_receipts p ON p.work_item_id = w.id
           JOIN catalog_imports ci ON ci.work_item_id = w.id
           JOIN tracks t ON t.release_id = ci.release_id
           JOIN media_assets ma ON ma.id = t.playable_media_asset_id
          WHERE w.id = $1",
    )
    .bind(fixture.work_item_id())
    .fetch_one(&db.pool)
    .await
    .expect("read authoritative policy facts");

    assert_eq!(durable_facts.0, IngestionState::Cataloged.as_str());
    assert_eq!(durable_facts.1, fixture.published.semantic_manifest_hash);
    assert_eq!(durable_facts.2, fixture.published.sealed_manifest_hash);
    assert_eq!(durable_facts.3, fixture.published.normalized_byte_hash);
    fixture.assert_media_paths_absent();

    let phase5_rows: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM release_classifications),
            (SELECT count(*) FROM track_novelty),
            (SELECT count(*) FROM release_novelty_evaluations),
            (SELECT count(*) FROM policy_decisions)",
    )
    .fetch_one(&db.pool)
    .await
    .expect("count pre-policy rows");
    assert_eq!(phase5_rows, (0, 0, 0, 0));

    let snapshot = PolicyStore
        .load_snapshot(&db.pool, fixture.release_id())
        .await
        .expect("load read-only policy snapshot")
        .expect("cataloged Release has a policy snapshot");
    assert_eq!(snapshot.tracks.len(), 1);
    assert_eq!(
        snapshot.tracks[0].byte_hash.as_deref(),
        Some(fixture.published.normalized_byte_hash.as_str())
    );
    // ADR-0213/ADR-0186: the sealed fingerprint lives in evidence, and the
    // durable comparison fact is its BLAKE3 digest.
    assert_eq!(snapshot.tracks[0].audio_fingerprint, None);
    assert_eq!(
        snapshot.tracks[0].audio_fingerprint_digest.as_deref(),
        Some(melodee_domain::acoustic_fingerprint_digest("fixture-fingerprint-0001").as_str())
    );
    assert!(
        !snapshot.tracks[0].has_confirmed_audio_identity,
        "a fresh local fingerprint is evidence, not a manual/provider confirmation"
    );
    assert!(snapshot.catalog_exact_byte_hashes.is_empty());
    assert!(snapshot.catalog_known_audio_identities.is_empty());
    assert!(snapshot.catalog_alternate_audio_identities.is_empty());
    let after_snapshot = WorkItemStore
        .get(&db.pool, fixture.work_item_id())
        .await
        .expect("read item after snapshot");
    assert_eq!(after_snapshot.state, IngestionState::Cataloged);
    assert!(after_snapshot.lease_owner.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_decisions")
            .fetch_one(&db.pool)
            .await
            .expect("dry snapshot leaves decisions empty"),
        0
    );

    db.drop().await;
}

#[tokio::test]
async fn policy_context_rejects_a_spoofed_source_binding_without_writes() {
    let db = TestDb::create("phase5_context_binding").await;
    let fixture = seed_cataloged_release(&db, 2, CatalogFixturePresentation::default()).await;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let mut claims = PolicyStore
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim policy context fixture");
    assert_eq!(claims.len(), 1);
    let mut spoofed = claims.pop().expect("one claim");
    spoofed.source_fingerprint = "f".repeat(64);
    assert!(matches!(
        PolicyStore.load_context(&db.pool, &spoofed).await,
        Err(PolicyStoreError::FingerprintMismatch)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT
                (SELECT count(*) FROM release_classifications)
              + (SELECT count(*) FROM track_novelty)
              + (SELECT count(*) FROM release_novelty_evaluations)
              + (SELECT count(*) FROM policy_decisions)",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count rejected policy writes"),
        0
    );
    fixture.assert_media_paths_absent();

    db.drop().await;
}
