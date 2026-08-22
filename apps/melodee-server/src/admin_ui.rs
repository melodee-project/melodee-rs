//! Admin SPA static serving (ADR-0138).
//!
//! The built admin SPA (`admin-ui/dist`) is served at `/admin` via
//! tower-http's `fs` feature with SPA fallback: any unmatched path under
//! `/admin` serves `index.html` so client-side deep links (e.g.
//! `/admin/processing/queue`) load the shell. The mount never shadows the
//! API — `/api/*` routes are registered on the same router and Axum matches
//! exact route templates before the nested service prefix.
//!
//! A missing dist directory is **not** fatal: the server starts, all APIs
//! serve normally, a warning is logged, and `/admin` falls through to the
//! outer 404 fallback until the SPA bundle is built and deployed.

use std::env;
use std::path::{Path, PathBuf};

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Canonical hierarchical configuration key for the SPA build output.
pub const ENV_ADMIN_UI_DIST: &str = "MELODEE__ADMIN_UI__DIST";
/// Default dist location, resolved relative to the working directory.
pub const DEFAULT_ADMIN_UI_DIST: &str = "admin-ui/dist";

/// Resolves the SPA dist directory: `MELODEE__ADMIN_UI__DIST` when set and
/// non-empty, otherwise `admin-ui/dist` relative to the working directory.
#[must_use]
pub fn configured_dist() -> PathBuf {
    select_dist(env::var(ENV_ADMIN_UI_DIST).ok())
}

fn select_dist(value: Option<String>) -> PathBuf {
    match value {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => PathBuf::from(DEFAULT_ADMIN_UI_DIST),
    }
}

/// The static file service for one dist directory: files under `dist`, with
/// `index.html` (status 200) as the fallback for SPA deep links so the
/// client-side router can resolve them.
fn spa_service(dist: &Path) -> ServeDir<ServeFile> {
    ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html")))
}

/// Mounts the admin SPA at `/admin` when the dist directory exists; otherwise
/// logs a warning and returns the router unchanged (the APIs keep serving and
/// `/admin` falls through to the outer fallback, which answers 404).
pub fn nest_admin_spa<S>(app: Router<S>, dist: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    if dist.is_dir() {
        app.nest_service("/admin", spa_service(dist))
    } else {
        tracing::warn!(
            path = %dist.display(),
            "admin SPA dist directory is missing; /admin will 404 until the bundle is built"
        );
        app
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_resolution_prefers_env_and_falls_back_to_default() {
        assert_eq!(select_dist(None), PathBuf::from(DEFAULT_ADMIN_UI_DIST));
        assert_eq!(
            select_dist(Some("  ".to_string())),
            PathBuf::from(DEFAULT_ADMIN_UI_DIST)
        );
        assert_eq!(
            select_dist(Some("/tmp/spa".to_string())),
            PathBuf::from("/tmp/spa")
        );
    }
}
