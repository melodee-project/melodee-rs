//! Lease, retry, and idempotency evidence for ADR-0162 provider work.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    EnrichmentEntityType, EnrichmentOutcome, EnrichmentPurpose, ProviderArtworkStore,
    ProviderEnrichmentStore, ProviderEnrichmentStoreError, ProviderIdentityCandidateObservation,
    ProviderIdentityCandidateStore,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[tokio::test]
async fn provider_tasks_are_idempotent_leased_and_fail_degraded() {
    let db = TestDb::create("provider_enrichment").await;
    let store = ProviderEnrichmentStore;
    let entity_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    store
        .schedule(
            &db.pool,
            EnrichmentEntityType::Artist,
            entity_id,
            "brave",
            EnrichmentPurpose::ArtistArtwork,
            "artist-fixture-request",
            now,
        )
        .await
        .expect("schedule provider task");
    store
        .schedule(
            &db.pool,
            EnrichmentEntityType::Artist,
            entity_id,
            "brave",
            EnrichmentPurpose::ArtistArtwork,
            "artist-fixture-request",
            now,
        )
        .await
        .expect("duplicate scheduling is idempotent");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_tasks")
            .fetch_one(&db.pool)
            .await
            .expect("count tasks"),
        1
    );

    let claimed = store
        .claim(&db.pool, "provider-worker-a", 8, 30, now)
        .await
        .expect("claim provider task");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempt_count, 1);
    store
        .fail_or_retry(
            &db.pool,
            &claimed[0],
            "provider-worker-a",
            "timeout",
            "deterministic fixture timeout",
            Some(now + Duration::seconds(10)),
            Some("cache-key-1"),
            now,
        )
        .await
        .expect("record retry");

    assert!(
        store
            .claim(
                &db.pool,
                "provider-worker-b",
                8,
                30,
                now + Duration::seconds(5)
            )
            .await
            .expect("early claim")
            .is_empty()
    );
    let reclaimed = store
        .claim(
            &db.pool,
            "provider-worker-b",
            8,
            30,
            now + Duration::seconds(11),
        )
        .await
        .expect("reclaim provider task");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].attempt_count, 2);
    store
        .complete(
            &db.pool,
            &reclaimed[0],
            "provider-worker-b",
            EnrichmentOutcome::CacheHit,
            Some("cache-key-2"),
            now + Duration::seconds(12),
        )
        .await
        .expect("complete provider task");

    let status: String =
        sqlx::query_scalar("SELECT status FROM provider_enrichment_tasks WHERE id=$1")
            .bind(reclaimed[0].id)
            .fetch_one(&db.pool)
            .await
            .expect("read task status");
    assert_eq!(status, "succeeded");
    let outcomes: Vec<String> = sqlx::query_scalar(
        "SELECT outcome FROM provider_enrichment_attempts
         WHERE task_id=$1 ORDER BY attempt_number",
    )
    .bind(reclaimed[0].id)
    .fetch_all(&db.pool)
    .await
    .expect("read attempt outcomes");
    assert_eq!(outcomes, ["retry", "cache_hit"]);
    db.drop().await;
}

#[tokio::test]
async fn provider_identity_candidates_are_observations_and_upserts_are_idempotent() {
    let db = TestDb::create("provider_identity_candidates").await;
    let artist_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, created_at, updated_at
         ) VALUES ($1,$1,'Candidate Artist','candidate artist','Candidate Artist',
                   'unknown','active',$2,$2)",
    )
    .bind(artist_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed Artist");
    let store = ProviderIdentityCandidateStore;
    store
        .upsert_artist_candidates(
            &db.pool,
            artist_id,
            "itunes",
            "Candidate",
            &[ProviderIdentityCandidateObservation {
                external_id: "42".to_owned(),
                candidate_rank: 1,
                confidence: 7_500,
                payload: serde_json::json!({"name":"Candidate Artist"}),
            }],
            now,
        )
        .await
        .expect("insert candidate evidence");
    store
        .upsert_artist_candidates(
            &db.pool,
            artist_id,
            "itunes",
            "Candidate",
            &[ProviderIdentityCandidateObservation {
                external_id: "42".to_owned(),
                candidate_rank: 2,
                confidence: 7_000,
                payload: serde_json::json!({"name":"Candidate Artist","updated":true}),
            }],
            now,
        )
        .await
        .expect("repeat candidate evidence is idempotent");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM provider_identity_candidates
             WHERE artist_id=$1 AND provider='itunes' AND external_id='42'",
        )
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("count candidate evidence"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT candidate_rank FROM provider_identity_candidates
             WHERE artist_id=$1 AND provider='itunes' AND external_id='42'",
        )
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("read current candidate evidence"),
        2
    );
    db.drop().await;
}

#[tokio::test]
async fn provider_artwork_round_trips_with_bounds_and_provider_precedence_data() {
    let db = TestDb::create("provider_artwork").await;
    let artist_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    let store = ProviderArtworkStore;
    let png_1x1 = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    store
        .upsert(
            &db.pool,
            EnrichmentEntityType::Artist,
            artist_id,
            "musicbrainz",
            "https://fixture.test/artist.png",
            "image/png",
            &png_1x1,
            7_500,
            now,
        )
        .await
        .expect("persist provider artwork");
    let image = store
        .image(&db.pool, EnrichmentEntityType::Artist, artist_id)
        .await
        .expect("read provider artwork")
        .expect("provider artwork row");
    assert_eq!(image.provider, "musicbrainz");
    assert_eq!(image.image_mime_type, "image/png");
    assert_eq!(image.image_bytes, png_1x1);
    assert_eq!(image.confidence, 7_500);

    assert!(matches!(
        store
            .upsert(
                &db.pool,
                EnrichmentEntityType::Artist,
                artist_id,
                "musicbrainz",
                "https://fixture.test/bad.gif",
                "image/gif",
                &png_1x1,
                1,
                now,
            )
            .await,
        Err(ProviderEnrichmentStoreError::InvalidRequest)
    ));
    db.drop().await;
}
