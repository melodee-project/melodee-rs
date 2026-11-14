//! request-correlation propagation (SPEC §22, `IMPLMENTATION_OUTSTANDING`
//! §7.6).
//!
//! The admin error-envelope contract has always documented `correlationId` as
//! "sourced from the `x-request-id` header when present, otherwise generated";
//! this module implements that sourcing once, at the HTTP boundary, and makes
//! the id available to every error constructor on the request task without
//! threading it through handler signatures:
//!
//! - [`middleware`] sources the id (header value when it parses as a UUID, a
//!   fresh v7 id otherwise), records it on the request tracing span, stores it
//!   as a request extension, and scopes a task-local around the rest of the
//!   request. The header is untrusted input: a malformed value never rejects
//!   the request (public-adapter error shapes are frozen) — it simply falls
//!   back to a generated id.
//! - [`envelope_id`] is the single constructor every error envelope uses:
//!   inside the middleware scope it returns the request's id; off-task
//!   (workers, tests, CLI) it generates a fresh id. Captured replay traffic
//!   sends no `x-request-id`, so captured behavior is byte-identical.
//! - [`current`] lets fire-and-record dispatchers (Last.fm, notifications)
//!   capture the in-flight request id before `tokio::spawn`, where the
//!   task-local no longer reaches.
//!
//! No response header is added: compatibility replay fails on any extra
//! response header, and the envelope proves the sourced id on its own.

use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use uuid::Uuid;

pub use melodee_application::correlation::{current, envelope_id};

/// Request extension carrying the sourced correlation id, for handlers that
/// have `Parts` access and want the id directly.
#[derive(Debug, Clone, Copy)]
pub struct CorrelationId(pub Uuid);

/// Sources the correlation id from `x-request-id` when present and parseable
/// as a UUID; otherwise generates a fresh v7 id.
#[must_use]
pub fn source_from_headers(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| Uuid::parse_str(raw.trim()).ok())
        .unwrap_or_else(Uuid::now_v7)
}

/// Axum middleware establishing the request correlation id. Must run inside
/// the `TraceLayer` span (added before it, so the layer wraps it) for the
/// span recording to land.
pub async fn middleware(mut request: Request, next: Next) -> Response {
    let id = source_from_headers(request.headers());
    tracing::Span::current().record("correlation_id", tracing::field::display(id));
    request.extensions_mut().insert(CorrelationId(id));
    melodee_application::correlation::scope(id, next.run(request)).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{HeaderValue, StatusCode},
        response::Json,
        routing::get,
    };
    use serde::Serialize;
    use tower::ServiceExt;

    #[test]
    fn sources_valid_header_and_falls_back_for_absent_or_malformed() {
        let mut headers = HeaderMap::new();
        let known = Uuid::now_v7();
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&known.to_string()).expect("ascii"),
        );
        assert_eq!(source_from_headers(&headers), known);

        assert_ne!(
            source_from_headers(&HeaderMap::new()),
            source_from_headers(&HeaderMap::new())
        );

        let mut malformed = HeaderMap::new();
        malformed.insert("x-request-id", HeaderValue::from_static("not-a-uuid"));
        let generated = source_from_headers(&malformed);
        assert_ne!(generated.to_string(), "not-a-uuid");
    }

    #[derive(Serialize)]
    struct TestEnvelope {
        correlation_id: Uuid,
    }

    async fn handler() -> (StatusCode, Json<TestEnvelope>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(TestEnvelope {
                correlation_id: envelope_id(),
            }),
        )
    }

    #[tokio::test]
    async fn middleware_scopes_the_envelope_id_and_extension() {
        let app = Router::new()
            .route("/probe", get(handler))
            .layer(axum::middleware::from_fn(middleware));

        let known = Uuid::now_v7();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header("x-request-id", known.to_string())
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["correlation_id"], known.to_string());

        // Without the header the envelope id is generated, never the nil or
        // a stale value from a previous request on the same task set.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let generated = body["correlation_id"].as_str().expect("uuid string");
        assert!(Uuid::parse_str(generated).is_ok());
        assert_ne!(generated, known.to_string());
    }

    #[tokio::test]
    async fn envelope_id_off_task_generates_fresh_ids() {
        assert!(current().is_none());
        assert_ne!(envelope_id(), envelope_id());
    }
}
