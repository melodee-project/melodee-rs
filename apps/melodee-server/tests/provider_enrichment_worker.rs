//! Reachability tests for the production enrichment composition.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use melodee_persistence::{
    EnrichmentEntityType, EnrichmentPurpose, ProviderEnrichmentStore, SettingsStore,
};
use melodee_server::enrichment_worker::{EnrichmentWorkerConfig, run_enrichment_once};
use melodee_server::providers::ProviderRuntime;
use time::OffsetDateTime;
use uuid::Uuid;

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
use common::TestDb;

#[tokio::test]
async fn disabled_provider_task_is_claimed_and_completed_without_http() {
    let db = TestDb::create("disabled").await;
    let task_store = ProviderEnrichmentStore;
    let entity_id = Uuid::now_v7();
    task_store
        .schedule(
            &db.pool,
            EnrichmentEntityType::Artist,
            entity_id,
            "discogs",
            EnrichmentPurpose::ArtistMetadata,
            "worker-test-discogs",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("schedule disabled provider task");

    let stats = run_enrichment_once(
        &db.pool,
        &EnrichmentWorkerConfig {
            batch_size: 1,
            ..EnrichmentWorkerConfig::default()
        },
        "worker-test",
    )
    .await
    .expect("run production enrichment pass");
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.disabled, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM provider_enrichment_tasks WHERE request_key='worker-test-discogs'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read task status"),
        "succeeded"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT outcome FROM provider_enrichment_attempts
             WHERE task_id=(SELECT id FROM provider_enrichment_tasks WHERE request_key='worker-test-discogs')",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read disabled attempt"),
        "disabled"
    );
    db.drop().await;
}

#[derive(Clone)]
struct FixtureState {
    requests: Arc<AtomicUsize>,
}

async fn itunes_fixture(State(state): State<FixtureState>) -> axum::Json<serde_json::Value> {
    state.requests.fetch_add(1, Ordering::Relaxed);
    axum::Json(serde_json::json!({
        "results": [{
            "artistId": 42,
            "artistName": "Fixture Provider Artist",
            "artworkUrl100": "https://fixture.invalid/image.png"
        }]
    }))
}

#[tokio::test]
async fn configured_provider_uses_http_fixture_then_fresh_cache() {
    let db = TestDb::create("http").await;
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/search", get(itunes_fixture))
        .with_state(FixtureState {
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind provider fixture");
    let address = listener.local_addr().expect("provider fixture address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve provider fixture");
    });
    SettingsStore::new()
        .set(
            &db.pool,
            "searchEngine.itunes.baseUrl",
            &serde_json::json!(format!("http://{address}")),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("configure fixture base URL");
    SettingsStore::new()
        .set(
            &db.pool,
            "searchEngine.itunes.enabled",
            &serde_json::json!(true),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("enable fixture provider");

    let runtime = ProviderRuntime::new(db.pool.clone()).expect("construct provider runtime");
    let selected = BTreeSet::from(["itunes".to_owned()]);
    let first = runtime
        .lookup_artists("Fixture", 5, None, Some(&selected))
        .await
        .expect("lookup configured provider");
    assert!(!first.cache_hit);
    assert!(first.failed_providers.is_empty());
    assert_eq!(first.candidates[0].external_id, "42");
    assert_eq!(requests.load(Ordering::Relaxed), 1);

    let second = runtime
        .lookup_artists("Fixture", 5, None, Some(&selected))
        .await
        .expect("lookup fresh provider cache");
    assert!(second.cache_hit);
    assert_eq!(second.candidates[0].name, "Fixture Provider Artist");
    assert_eq!(requests.load(Ordering::Relaxed), 1);

    let artist_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, status, created_at, updated_at
         ) VALUES ($1,$1,'Fixture Provider Artist','fixture provider artist',
                   'Fixture Provider Artist','unknown','active',$2,$2)",
    )
    .bind(artist_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed Artist for production worker path");
    ProviderEnrichmentStore
        .schedule(
            &db.pool,
            EnrichmentEntityType::Artist,
            artist_id,
            "itunes",
            EnrichmentPurpose::ArtistMetadata,
            "worker-test-itunes-artist",
            now,
        )
        .await
        .expect("schedule configured Artist task");
    let worker_stats = run_enrichment_once(
        &db.pool,
        &EnrichmentWorkerConfig {
            batch_size: 1,
            ..EnrichmentWorkerConfig::default()
        },
        "worker-test-configured",
    )
    .await
    .expect("run configured production worker path");
    assert_eq!(worker_stats.claimed, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM provider_identity_candidates
             WHERE artist_id=$1 AND provider='itunes' AND external_id='42'",
        )
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("read provider candidate evidence"),
        1
    );

    server.abort();
    db.drop().await;
}

async fn rate_limited_fixture() -> impl IntoResponse {
    (StatusCode::TOO_MANY_REQUESTS, "fixture rate limit")
}

#[tokio::test]
async fn rate_limited_provider_is_retried_bounded_and_reported() {
    let db = TestDb::create("rate_limited").await;
    let app = Router::new().route("/search", get(rate_limited_fixture));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind rate-limit fixture");
    let address = listener.local_addr().expect("rate-limit fixture address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve rate-limit fixture");
    });
    let now = OffsetDateTime::now_utc();
    SettingsStore::new()
        .set(
            &db.pool,
            "searchEngine.itunes.baseUrl",
            &serde_json::json!(format!("http://{address}")),
            None,
            now,
        )
        .await
        .expect("configure rate-limit fixture base URL");
    SettingsStore::new()
        .set(
            &db.pool,
            "searchEngine.itunes.enabled",
            &serde_json::json!(true),
            None,
            now,
        )
        .await
        .expect("enable rate-limit fixture provider");

    let runtime = ProviderRuntime::new(db.pool.clone()).expect("construct provider runtime");
    let selected = BTreeSet::from(["itunes".to_owned()]);
    let result = runtime
        .lookup_artists("Rate Limited", 5, None, Some(&selected))
        .await
        .expect("return partial provider result");
    assert!(result.candidates.is_empty());
    assert_eq!(result.failed_providers.len(), 1);
    assert_eq!(result.failed_providers[0].category, "rate_limited");

    server.abort();
    db.drop().await;
}
