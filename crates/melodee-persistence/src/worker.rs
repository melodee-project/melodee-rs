//! Process-unique worker identity (Phase 1A work-package part XVII).
//!
//! Format: `{hostname}-{pid}-{uuidv7}`.
//!
//! Correctness never depends on hostname uniqueness: the uuidv7 component
//! guarantees distinct identities for two workers on one machine (and across
//! machines sharing hostnames). Hostname and pid are diagnostic context only.

use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(String);

impl WorkerId {
    #[must_use]
    pub fn new() -> Self {
        let host = std::env::var("HOSTNAME")
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "unknown-host".to_string());
        Self(format!(
            "{}-{}-{}",
            host,
            std::process::id(),
            Uuid::now_v7()
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkerId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerId;

    #[test]
    fn worker_ids_are_unique_per_instance() {
        let a = WorkerId::new();
        let b = WorkerId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn worker_id_format_has_three_parts() {
        let id = WorkerId::new();
        let parts: Vec<&str> = id.as_str().rsplitn(3, '-').collect();
        assert_eq!(parts.len(), 3, "hostname-pid-uuid: {id}");
    }
}
