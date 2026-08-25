//! Melodee server library: Axum composition-root pieces exposed for
//! integration tests. The binary entry point lives in `main.rs`.
#![recursion_limit = "512"]

pub mod admin;
pub mod admin_access;
pub mod admin_catalog;
pub mod admin_config;
pub mod admin_device_profiles;
pub mod admin_identity;
pub mod admin_notifications;
pub mod admin_onboarding;
pub mod admin_policies;
pub mod admin_radio;
pub mod admin_review;
pub mod admin_scripts;
pub mod admin_system;
pub mod admin_ui;
pub mod correlation;
pub mod discovery_worker;
pub mod enrichment_worker;
pub mod jellyfin;
pub mod lastfm;
pub mod native;
pub mod native_stream;
pub mod notifications;
pub mod onboarding;
pub mod opensubsonic;
pub mod podcast_worker;
pub mod policy_worker;
pub mod providers;
pub mod resource_bounds;
pub mod scripting;
pub mod security_headers;
pub mod system;
pub mod transcode;

/// Builds the request span without recording the raw URI.
///
/// Native stream and reset-validation URLs carry bearer material in path segments, and query
/// strings can contain credentials supplied by external clients. Only the normalized Axum route
/// template is safe to attach to routine request telemetry.
pub fn make_http_span<B>(request: &axum::http::Request<B>) -> tracing::Span {
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(axum::extract::MatchedPath::as_str)
        .unwrap_or("<unmatched>");

    tracing::debug_span!(
        "http_request",
        method = %request.method(),
        route,
        version = ?request.version(),
        correlation_id = tracing::field::Empty,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::{
        io,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for CaptureWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn http_request_span_never_records_raw_path_or_query_secrets() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(capture.clone())
            .finish();
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/v1/auth/password-reset/validate/PATH_SECRET?token=QUERY_SECRET")
            .body(())
            .expect("test request");

        tracing::subscriber::with_default(subscriber, || {
            let span = super::make_http_span(&request);
            let _entered = span.enter();
            tracing::debug!("request accepted");
        });

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).expect("utf-8 trace");
        assert!(output.contains("http_request"));
        assert!(output.contains("route=\"<unmatched>\""));
        assert!(!output.contains("PATH_SECRET"));
        assert!(!output.contains("QUERY_SECRET"));
    }
}
