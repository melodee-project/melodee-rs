//! Attribution for non-human Admin API callers (ADR-0196).
//!
//! An AI harness or coding agent authenticates as an administrator, so the
//! durable audit actor stays the administrator's public id. An optional
//! `X-Melodee-Agent` header lets the caller state which harness acted; it is
//! truncated, stripped of control characters, and recorded in tracing and
//! automated proposal authority. It never replaces or weakens the
//! administrator gate.

use axum::http::HeaderMap;

/// Maximum stored agent label length. Long labels add no audit value and make
/// logs and proposal authority rows noisy.
const MAX_AGENT_LABEL_CHARS: usize = 64;

/// Returns a display-safe agent label when the caller supplied one.
///
/// The value is advisory attribution only; it is never used for
/// authorization or as an identity key.
pub fn agent_label(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("x-melodee-agent")?.to_str().ok()?.trim();
    let sanitized = raw
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_AGENT_LABEL_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    (!sanitized.is_empty()).then_some(sanitized)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header::HeaderName};

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-melodee-agent"),
            HeaderValue::from_str(value).expect("valid test header"),
        );
        headers
    }

    #[test]
    fn missing_header_is_none() {
        assert_eq!(None, agent_label(&HeaderMap::new()));
    }

    #[test]
    fn blank_header_is_none() {
        assert_eq!(None, agent_label(&headers("   ")));
    }

    #[test]
    fn label_is_trimmed_and_bounded() {
        let label = agent_label(&headers(&format!("  {}  ", "a".repeat(200)))).expect("label");
        assert_eq!(MAX_AGENT_LABEL_CHARS, label.len());
    }
}
