//! Read-only, in-process admin statistics route comparison on the owned fixture.
#![allow(clippy::expect_used)]

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use melodee_auth::{NativeAccessClaims, SecretString, issue_access_token};
use melodee_server::{admin_access::AdminAccessState, admin_system::AdminSystemState};
use sqlx::Connection;
use std::{sync::Arc, time::Instant};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "performance-fixture-jwt-key-at-least-32-bytes";

fn normalize_stats(value: &mut serde_json::Value) -> String {
    let generated = value
        .as_object_mut()
        .expect("stats object")
        .shift_remove("generatedAt")
        .expect("generatedAt");
    OffsetDateTime::parse(generated.as_str().expect("generatedAt string"), &Rfc3339)
        .expect("valid generatedAt");
    serde_json::to_string(value).expect("normalized JSON")
}

#[test]
fn normalization_preserves_relative_object_order() {
    fn normalize(text: &str) -> String {
        normalize_stats(&mut serde_json::from_str(text).expect("valid JSON"))
    }
    let initial = normalize(r#"{"generatedAt":"2026-01-01T00:00:00Z","a":{"x":1,"y":2},"b":3}"#);
    let moved = normalize(r#"{"a":{"x":1,"y":2},"generatedAt":"2026-01-01T00:00:01Z","b":3}"#);
    let reversed = normalize(r#"{"b":3,"generatedAt":"2026-01-01T00:00:00Z","a":{"x":1,"y":2}}"#);
    let nested = normalize(r#"{"generatedAt":"2026-01-01T00:00:00Z","a":{"y":2,"x":1},"b":3}"#);
    assert_eq!(initial, moved);
    assert_ne!(initial, reversed);
    assert_ne!(initial, nested);
}

async fn fixture_sessions_stable(
    pool: &sqlx::PgPool,
    attempts: usize,
    interval: std::time::Duration,
) -> bool {
    let mut stable = 0;
    for _ in 0..attempts {
        let sessions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()",
        )
        .fetch_one(pool)
        .await
        .expect("fixture sessions");
        stable = if sessions == 8 { stable + 1 } else { 0 };
        if stable == 3 {
            return true;
        }
        tokio::time::sleep(interval).await;
    }
    false
}

#[tokio::test]
#[ignore = "requires the invocation-owned representative fixture and release build"]
async fn measure_read_only_admin_stats_route() {
    let url = std::env::var("MELODEE__DATABASE__URL").expect("fixture URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("fixture pool");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database name");
    assert!(database.strip_prefix("melodee_template_").is_some_and(
        |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    ));
    assert_eq!(
        std::env::var("MELODEE_PERF_OWNED_DATABASE").expect("owned database"),
        database
    );
    // The route reports every database session. Prewarm and retain a full,
    // fixed-size pool so lazy connection creation cannot change that value
    // between samples. Unrelated sessions still make the check fail closed.
    let mut connections = Vec::with_capacity(8);
    for _ in 0..8 {
        connections.push(pool.acquire().await.expect("prewarm fixture pool"));
    }
    drop(connections);
    // Exercise the fail-closed gate against a real ninth session before
    // timing. It must never accept an extra connection as a stable pool.
    let extra = sqlx::PgConnection::connect(&url)
        .await
        .expect("open ninth fixture session");
    assert!(
        !fixture_sessions_stable(&pool, 3, std::time::Duration::ZERO).await,
        "ninth fixture session was accepted"
    );
    extra.close().await.expect("close ninth fixture session");
    assert!(
        fixture_sessions_stable(&pool, 30, std::time::Duration::from_secs(1)).await,
        "fixture database sessions did not stabilize"
    );
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artists WHERE status <> 'merged'), (SELECT count(*) FROM releases), (SELECT count(*) FROM tracks)")
        .fetch_one(&pool).await.expect("fixture counts");
    let admin: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM native_users WHERE normalized_username='BENCHMARK-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("synthetic administrator");
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let token = issue_access_token(
        &SecretString::new(KEY.into()),
        &NativeAccessClaims {
            aud: "MelodeeClient".into(),
            email: "benchmark-1@example.test".into(),
            exp: now + 3600,
            sid: admin,
            iat: now,
            iss: "MelodeeApi".into(),
            nbf: now,
            role: vec![],
            unique_name: "benchmark-1".into(),
            theme: String::new(),
        },
    )
    .expect("fixture token");
    let state = AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool: pool.clone(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(KEY.into(), 90).expect("auth"),
        ),
    };
    let router =
        melodee_server::admin_stats::router().with_state(AdminSystemState::new(state, None));
    let mut expected: Option<String> = None;
    for iteration in 0..9 {
        let request = Request::builder()
            .uri("/api/admin/v1/stats")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request");
        let started = Instant::now();
        let response = router.clone().oneshot(request).await.expect("route");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("bounded response");
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(status, StatusCode::OK);
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        // Preserve relative object order when removing the time-varying field.
        let normalized = normalize_stats(&mut value);
        let library = value.get("library").expect("library panel").clone();
        assert_eq!(library["artists"], counts.0);
        assert_eq!(library["releases"], counts.1);
        assert_eq!(library["tracks"], counts.2);
        // Compare serialized bytes because Value's map equality ignores order.
        if let Some(previous) = &expected {
            assert_eq!(
                previous, &normalized,
                "normalized stats result changed between samples"
            );
        } else {
            expected = Some(normalized);
        }
        println!(
            "admin_stats_ab_iteration={iteration} elapsed_ms={elapsed_ms:.3} response_bytes={}",
            bytes.len()
        );
    }
    println!(
        "admin_stats_ab_response={}",
        expected.expect("sampled normalized response")
    );
    println!(
        "admin_stats_ab_fixture artists={} releases={} tracks={}",
        counts.0, counts.1, counts.2
    );
}
