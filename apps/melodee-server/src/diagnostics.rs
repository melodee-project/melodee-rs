//! Runtime diagnostics primitives: a bounded in-memory ring buffer of
//! warning/error events and an HTTP completion logger.
//!
//! Operators and troubleshooting agents need a way to answer "what just went
//! wrong?" without tailing container logs or grepping a daily file. The
//! buffer keeps the most recent [`EVENT_CAPACITY`] WARN/ERROR events,
//! including their `tracing` target and correlation id, and is exposed read
//! only through the admin diagnostics API (`/api/admin/v1/diagnostics/events`).
//!
//! The buffer deliberately records only the rendered event message plus the
//! correlation id. Structured field values are not copied, because worker
//! events may carry file paths, provider URLs, or other operator data that
//! must not accumulate in a process-wide snapshot. Messages are written by
//! this codebase without secrets by convention (see `logging.rs`), and are
//! truncated to a fixed size so a pathological message cannot grow the
//! buffer unboundedly.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};
use serde::Serialize;
use time::OffsetDateTime;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

use crate::correlation::CorrelationId;

/// Number of retained warning/error events.
pub const EVENT_CAPACITY: usize = 200;
/// Maximum retained message length (characters) per event.
pub const MAX_MESSAGE_CHARS: usize = 1_024;
/// Requests slower than this are logged at WARN with their route template.
pub const SLOW_REQUEST_MILLIS: u128 = 10_000;

static GLOBAL: OnceLock<Arc<EventBuffer>> = OnceLock::new();

/// One retained WARN/ERROR event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsEvent {
    pub timestamp: String,
    /// `WARN` or `ERROR`.
    pub level: &'static str,
    /// `tracing` target (module path), useful for scoping log filters.
    pub target: String,
    pub message: String,
    pub correlation_id: Option<String>,
}

/// Bounded ring buffer of recent warning/error events.
#[derive(Debug)]
pub struct EventBuffer {
    capacity: usize,
    entries: Mutex<VecDeque<DiagnosticsEvent>>,
}

impl EventBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    fn push(&self, event: DiagnosticsEvent) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() == self.capacity {
            entries.pop_front();
        }
        entries.push_back(event);
    }

    /// Records one synthetic event. Test/diagnostic seam only: production
    /// events arrive through [`EventBufferLayer`].
    #[doc(hidden)]
    pub fn record_for_tests(&self, level: &'static str, target: &str, message: &str) {
        self.push(DiagnosticsEvent {
            timestamp: OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string()),
            level,
            target: target.to_string(),
            message: truncate(message, MAX_MESSAGE_CHARS),
            correlation_id: None,
        });
    }

    /// Configured retained-event capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Newest-last snapshot, bounded by `limit` (0 means "all retained").
    #[must_use]
    pub fn snapshot(&self, limit: usize) -> Vec<DiagnosticsEvent> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let take = if limit == 0 {
            entries.len()
        } else {
            limit.min(entries.len())
        };
        entries.iter().skip(entries.len() - take).cloned().collect()
    }
}

/// Installs the process-wide buffer. Called once during logging setup; a
/// second call returns the first handle unchanged.#[must_use]
pub fn install(capacity: usize) -> Arc<EventBuffer> {
    GLOBAL
        .get_or_init(|| Arc::new(EventBuffer::new(capacity)))
        .clone()
}

/// Process-wide buffer, when logging initialized one.
#[must_use]
pub fn global() -> Option<Arc<EventBuffer>> {
    GLOBAL.get().cloned()
}

/// `tracing` layer feeding the ring buffer. Records WARN and ERROR events.
pub struct EventBufferLayer {
    buffer: Arc<EventBuffer>,
}

impl EventBufferLayer {
    #[must_use]
    pub fn new(buffer: Arc<EventBuffer>) -> Self {
        Self { buffer }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    correlation_id: Option<String>,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => {
                self.message = Some(truncate(&format!("{value:?}"), MAX_MESSAGE_CHARS));
            }
            "correlation_id" => {
                self.correlation_id = Some(truncate(&format!("{value:?}"), 64));
            }
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "correlation_id" {
            self.correlation_id = Some(truncate(value, 64));
        }
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}…")
}

impl<S> Layer<S> for EventBufferLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            _ => return,
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let timestamp = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string());
        self.buffer.push(DiagnosticsEvent {
            timestamp,
            level,
            target: metadata.target().to_string(),
            message: visitor
                .message
                .unwrap_or_else(|| "<event without message>".to_string()),
            correlation_id: visitor.correlation_id,
        });
    }
}

/// HTTP completion logging. Runs inside the correlation middleware so the
/// request's correlation id is available, and only records the normalized
/// route template — never the raw path or query string (stream and reset URLs
/// carry bearer material in path segments).
pub async fn request_logging(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
        .to_string();
    let correlation_id = request.extensions().get::<CorrelationId>().map(|id| id.0);
    let started = Instant::now();
    let response = next.run(request).await;
    let latency_ms = started.elapsed().as_millis();
    let status = response.status().as_u16();

    if status >= 500 {
        tracing::warn!(
            method = %method,
            route = %route,
            status,
            latency_ms,
            correlation_id = correlation_id.map(tracing::field::display),
            "request failed"
        );
    } else if status == 429 || latency_ms >= SLOW_REQUEST_MILLIS {
        tracing::warn!(
            method = %method,
            route = %route,
            status,
            latency_ms,
            correlation_id = correlation_id.map(tracing::field::display),
            "request completed with warning"
        );
    } else {
        tracing::debug!(
            method = %method,
            route = %route,
            status,
            latency_ms,
            correlation_id = correlation_id.map(tracing::field::display),
            "request completed"
        );
    }
    response
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn event(level: &'static str, message: &str) -> DiagnosticsEvent {
        DiagnosticsEvent {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            level,
            target: "melodee_server::probe".to_string(),
            message: message.to_string(),
            correlation_id: None,
        }
    }

    #[test]
    fn buffer_is_bounded_and_returns_newest_entries() {
        let buffer = EventBuffer::new(3);
        for index in 0..5 {
            buffer.push(event("WARN", &format!("event {index}")));
        }
        let all = buffer.snapshot(0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].message, "event 2");
        assert_eq!(all[2].message, "event 4");
        let newest = buffer.snapshot(1);
        assert_eq!(newest.len(), 1);
        assert_eq!(newest[0].message, "event 4");
    }

    #[test]
    fn messages_are_truncated_without_panicking_on_multibyte_boundaries() {
        let long = "é".repeat(MAX_MESSAGE_CHARS + 10);
        let truncated = truncate(&long, MAX_MESSAGE_CHARS);
        assert_eq!(truncated.chars().count(), MAX_MESSAGE_CHARS + 1);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate("short", 10), "short");
    }

    #[tokio::test]
    async fn request_logging_preserves_the_response_and_status() {
        use axum::{Router, body::Body, http::StatusCode, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/probe", get(|| async { (StatusCode::NO_CONTENT, "ok") }))
            .layer(axum::middleware::from_fn(request_logging));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn install_is_idempotent() {
        let first = install(8);
        let second = install(64);
        assert!(Arc::ptr_eq(&first, &second));
        assert!(global().is_some());
    }
}
