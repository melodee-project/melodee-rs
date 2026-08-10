//! Runtime liveness and basic PostgreSQL readiness endpoints.

use std::{env, time::Duration};

use axum::{Json, http::StatusCode};
use serde::Serialize;

/// The readiness probe is deliberately short and bounded so an unavailable
/// database cannot indefinitely consume HTTP tasks.
pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(2);
/// Canonical hierarchical server bind key.
pub const ENV_SERVER_BIND: &str = "MELODEE__SERVER__BIND";
const LEGACY_ENV_SERVER_BIND: &str = "MELODEE_BIND";
const DEFAULT_SERVER_BIND: &str = "127.0.0.1:8080";

/// Resolves the listener address, preferring the canonical hierarchical key
/// while retaining the original bootstrap key as a compatibility fallback.
#[must_use]
pub fn configured_bind() -> String {
    select_bind(
        env::var(ENV_SERVER_BIND).ok(),
        env::var(LEGACY_ENV_SERVER_BIND).ok(),
    )
}

fn select_bind(canonical: Option<String>, legacy: Option<String>) -> String {
    canonical
        .or(legacy)
        .unwrap_or_else(|| DEFAULT_SERVER_BIND.to_owned())
}

/// Runtime health/version metadata (SPEC section 27). It intentionally omits
/// implementation-phase labels, which belong in implementation evidence and
/// become stale in runtime responses.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    service: &'static str,
    version: &'static str,
    status: &'static str,
}

impl SystemInfo {
    const fn new(status: &'static str) -> Self {
        Self {
            service: "melodee-server",
            version: env!("CARGO_PKG_VERSION"),
            status,
        }
    }
}

/// State for the bounded PostgreSQL readiness probe.
#[derive(Clone)]
pub struct ReadinessState {
    pool: sqlx::PgPool,
    timeout: Duration,
}

impl ReadinessState {
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            timeout: DEFAULT_READINESS_TIMEOUT,
        }
    }

    /// Overrides the timeout for focused tests and tightly constrained
    /// embedding environments.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Liveness only: a successful response proves the HTTP process can serve a
/// request. It makes no dependency or worker-readiness claim.
pub async fn healthz() -> Json<SystemInfo> {
    Json(SystemInfo::new("ok"))
}

/// Basic service readiness: PostgreSQL must answer a bounded `SELECT 1`.
/// Media tools, storage roots, disk space, and individual workers are outside
/// this intentionally narrow contract.
pub async fn readyz(
    axum::extract::State(state): axum::extract::State<ReadinessState>,
) -> (StatusCode, Json<SystemInfo>) {
    let probe = sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool);
    match tokio::time::timeout(state.timeout, probe).await {
        Ok(Ok(1)) => (StatusCode::OK, Json(SystemInfo::new("ready"))),
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SystemInfo::new("not_ready")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bind_key_wins_and_legacy_key_remains_a_fallback() {
        assert_eq!(
            select_bind(
                Some("127.0.0.1:9000".to_owned()),
                Some("127.0.0.1:8000".to_owned())
            ),
            "127.0.0.1:9000"
        );
        assert_eq!(
            select_bind(None, Some("127.0.0.1:8000".to_owned())),
            "127.0.0.1:8000"
        );
        assert_eq!(select_bind(None, None), DEFAULT_SERVER_BIND);
    }
}
