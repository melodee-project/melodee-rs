//! Explicit, ignored release-build benchmark for the actual OpenSubsonic router.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{net::SocketAddr, time::Instant};

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::Request,
};
use melodee_server::opensubsonic::{OsState, configured_router};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires the invocation-owned representative fixture and a release build"]
async fn measure_artist_indexes() {
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture database URL");
    let _ = tracing_subscriber::fmt().without_time().try_init();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET statement_timeout = '15s'")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect fixture");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database.strip_prefix("melodee_template_").is_some_and(
            |suffix| suffix.len() == 32 && suffix.chars().all(|c| c.is_ascii_hexdigit())
        ),
        "refusing a non-fixture database"
    );
    let user_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username = 'BENCHMARK-1'")
            .fetch_one(&pool)
            .await
            .unwrap();

    // The same indexed user-state predicate used by the route. These are
    // diagnostic query plans, not substitutes for the router timings below.
    let queries = [
        (
            "correlated",
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) SELECT artist.id, COALESCE((SELECT AVG(rating)::float8 FROM native_artist_user_states ratings WHERE ratings.artist_id = artist.id AND rating > 0 AND ratings.user_id <> $1), 0) FROM artists artist WHERE artist.status NOT IN ('merged', 'deprecated')",
        ),
        (
            "grouped",
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) SELECT artist.id, COALESCE(ratings.average_rating, 0) FROM artists artist LEFT JOIN (SELECT artist_id, AVG(rating)::float8 AS average_rating FROM native_artist_user_states WHERE rating > 0 AND user_id <> $1 GROUP BY artist_id) ratings ON ratings.artist_id = artist.id WHERE artist.status NOT IN ('merged', 'deprecated')",
        ),
    ];
    for (name, query) in queries {
        match sqlx::query_scalar::<_, String>(query)
            .bind(user_id)
            .fetch_all(&pool)
            .await
        {
            Ok(plan) => println!("plan {name}:\n{}", plan.join("\n")),
            Err(error) => println!("plan {name} failed: {error}"),
        }
    }

    let state = OsState {
        pool,
        server_version: "performance-fixture".into(),
        public_origin: "http://benchmark.invalid".into(),
        base_url_host: None,
        legacy_auth_bypass: false,
        music_folders: vec![],
        podcasts_enabled: false,
        downloads_enabled: false,
        jukebox_enabled: false,
        direct_play: None,
        transcoding: None,
        client_addresses: Default::default(),
        search: None,
        lastfm_api_base_url: None,
    };
    let app = configured_router(state);
    for endpoint in ["getArtists", "getIndexes"] {
        for format in ["json", "xml"] {
            for iteration in 0..6 {
                let uri = format!(
                    "/rest/{endpoint}.view?f={format}&u=benchmark-1&p=benchmark-only-password&c=benchmark&v=1.16.1"
                );
                let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
                request.extensions_mut().insert(ConnectInfo(
                    "203.0.113.7:9000".parse::<SocketAddr>().unwrap(),
                ));
                let start = Instant::now();
                let response = app.clone().oneshot(request).await.unwrap();
                assert!(response.status().is_success());
                let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
                    .await
                    .unwrap();
                let body = std::str::from_utf8(&bytes).unwrap();
                if !body.contains(if format == "json" {
                    "\"status\":\"ok\""
                } else {
                    "status=\"ok\""
                }) {
                    println!(
                        "route={endpoint} format={format} iteration={iteration} failed_after_ms={:.3}",
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                    return;
                }
                println!(
                    "route={endpoint} format={format} iteration={iteration} elapsed_ms={:.3} bytes={}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    bytes.len()
                );
            }
        }
    }
}
