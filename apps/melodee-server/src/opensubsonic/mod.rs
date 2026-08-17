//! OpenSubsonic compatibility adapter (ADR-0102, PRD-API-OS-001..007).
//!
//! Protocol-owned module: legacy `album`/`song` vocabulary lives only inside
//! this adapter's DTOs; the core domain keeps `Release`/`Track`. No wire DTO
//! is shared with the native-v1 or Jellyfin adapters.

pub mod annotation;
pub mod auth;
pub mod bookmarks;
pub mod browse;
pub mod envelope;
pub mod lists;
pub mod media;
pub mod negative;
pub mod params;
pub mod playlists;
pub mod playqueue;
pub mod podcasts;
pub mod radio;
pub mod scan;
pub mod shares;
pub mod system;
pub mod users;

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use uuid::Uuid;

/// One configured music folder (ingestion source or the canonical storage
/// root), rendered with the captured `library_{uuid}` id form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicFolder {
    pub id: Uuid,
    pub name: String,
}

impl MusicFolder {
    #[must_use]
    pub fn wire_id(&self) -> String {
        format!("library_{}", self.id)
    }
}

/// Shared state for the OpenSubsonic adapter.
#[derive(Clone)]
pub struct OsState {
    pub pool: sqlx::PgPool,
    /// Server version echoed as the volatile `serverVersion` envelope field.
    pub server_version: String,
    pub public_origin: String,
    /// Host that triggers the captured auth bypass when it equals the
    /// request Host header (the legacy `system.baseUrl` rule).
    pub base_url_host: Option<String>,
    pub music_folders: Vec<Arc<MusicFolder>>,
    pub podcasts_enabled: bool,
    pub downloads_enabled: bool,
    pub jukebox_enabled: bool,
    pub direct_play: Option<crate::native_stream::DirectPlayService>,
    /// Shared configured-transcoding service (ADR-0147); `None` disables
    /// transcoding (transcode requests fail closed, direct play unaffected).
    pub transcoding: Option<crate::transcode::TranscodingService>,
    pub client_addresses: crate::native_stream::ClientAddressResolver,
    pub search: Option<melodee_search::SearchConfig>,
}

const ENV_BASE_URL_HOST: &str = "MELODEE__OPENSUBSONIC__BASE_URL_HOST";
const ENV_STORAGE_LIBRARY_ID: &str = "MELODEE__OPENSUBSONIC__STORAGE_LIBRARY_ID";
const ENV_PODCASTS_ENABLED: &str = "MELODEE__OPENSUBSONIC__PODCASTS_ENABLED";
const ENV_DOWNLOADS_ENABLED: &str = "MELODEE__OPENSUBSONIC__DOWNLOADS_ENABLED";
const ENV_JUKEBOX_ENABLED: &str = "MELODEE__OPENSUBSONIC__JUKEBOX_ENABLED";

fn env_flag(name: &str, default: bool) -> Result<bool, &'static str> {
    match std::env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err("invalid boolean"),
        },
        Err(_) => Ok(default),
    }
}

impl OsState {
    /// Builds adapter state from the composition-root environment. Music
    /// folders are the configured ingestion sources plus the canonical
    /// storage root, mirroring the legacy Libraries rows.
    #[allow(clippy::too_many_arguments)]
    pub fn from_env(
        pool: sqlx::PgPool,
        public_origin: String,
        ingestion: &melodee_ingestion::IngestionConfig,
        podcast_default_enabled: bool,
        direct_play: Option<crate::native_stream::DirectPlayService>,
        transcoding: Option<crate::transcode::TranscodingService>,
        client_addresses: crate::native_stream::ClientAddressResolver,
        search: Option<melodee_search::SearchConfig>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut music_folders: Vec<Arc<MusicFolder>> = ingestion
            .sources
            .iter()
            .filter_map(|source| {
                source.id.map(|id| {
                    Arc::new(MusicFolder {
                        id,
                        name: source.name.clone(),
                    })
                })
            })
            .collect();
        if ingestion.storage_root.is_some() {
            let storage_id = match std::env::var(ENV_STORAGE_LIBRARY_ID) {
                Ok(value) => Uuid::parse_str(&value)
                    .map_err(|_| "MELODEE__OPENSUBSONIC__STORAGE_LIBRARY_ID must be a UUID")?,
                // Documented default for the canonical storage music folder;
                // deployments preserving migrated legacy folder ids set
                // MELODEE__OPENSUBSONIC__STORAGE_LIBRARY_ID instead.
                Err(_) => Uuid::parse_str("6b1c9f4a-2c3e-5f0a-9d8b-7c6a5e4d3f2a")
                    .map_err(|_| "internal default storage library id is invalid")?,
            };
            music_folders.push(Arc::new(MusicFolder {
                id: storage_id,
                name: "Storage".to_string(),
            }));
        }
        Ok(Self {
            pool,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            public_origin,
            base_url_host: std::env::var(ENV_BASE_URL_HOST)
                .ok()
                .filter(|value| !value.is_empty()),
            music_folders,
            podcasts_enabled: env_flag(ENV_PODCASTS_ENABLED, podcast_default_enabled)
                .map_err(|_| "MELODEE__OPENSUBSONIC__PODCASTS_ENABLED must be a boolean")?,
            downloads_enabled: env_flag(ENV_DOWNLOADS_ENABLED, true)
                .map_err(|_| "MELODEE__OPENSUBSONIC__DOWNLOADS_ENABLED must be a boolean")?,
            jukebox_enabled: env_flag(ENV_JUKEBOX_ENABLED, false)
                .map_err(|_| "MELODEE__OPENSUBSONIC__JUKEBOX_ENABLED must be a boolean")?,
            direct_play,
            transcoding,
            client_addresses,
            search,
        })
    }
}

/// Registers one endpoint under both the plain and `.view` route forms with
/// GET and POST methods (PRD-API-OS-001; formPost extension).
pub fn endpoint_router<S, H, T>(name: &str, handler: H) -> Router<S>
where
    H: axum::handler::Handler<T, S> + Clone + Send + Sync + 'static,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            &format!("/rest/{name}"),
            get(handler.clone()).post(handler.clone()),
        )
        .route(
            &format!("/rest/{name}.view"),
            get(handler.clone()).post(handler),
        )
}

async fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// Public routes: ping (including the captured server-root `/ping` alias)
/// and getOpenSubsonicExtensions. Everything else — including the 410/501
/// negative matrix, which the legacy controllers answered only after their
/// authorization filter ran — is protected by the auth middleware in
/// [`configured_router`].
fn public_router() -> Router<OsState> {
    Router::new()
        .merge(endpoint_router("ping", system::ping))
        .route("/ping", get(system::ping).post(system::ping))
        .merge(endpoint_router(
            "getOpenSubsonicExtensions",
            system::get_opensubsonic_extensions,
        ))
        .fallback(not_found)
}

/// Protected routes: the captured auth contract (bypass + token/password)
/// applies to everything except the public surface.
fn protected_router() -> Router<OsState> {
    Router::new()
        .merge(endpoint_router("getLicense", system::get_license))
        .merge(endpoint_router(
            "getMusicFolders",
            system::get_music_folders,
        ))
        .merge(endpoint_router("getPodcasts", podcasts::get_podcasts))
        .merge(endpoint_router(
            "getNewestPodcasts",
            podcasts::get_newest_podcasts,
        ))
        .merge(lists::router())
        .merge(annotation::router())
        .merge(browse::router())
        .merge(bookmarks::router())
        .merge(playqueue::router())
        .merge(playlists::router())
        .merge(media::router())
        .merge(shares::router())
        .merge(radio::router())
        .merge(scan::router())
        .merge(users::router())
        .merge(podcasts::router())
        .merge(negative::router())
}

/// Builds OpenSubsonic routes without the auth middleware. Tests install
/// principals or bypass markers directly.
pub fn router() -> Router<OsState> {
    public_router().merge(protected_router())
}

/// Builds the production OpenSubsonic router: parameter parsing for every
/// route, then the captured authentication contract on protected routes.
pub fn configured_router(state: OsState) -> Router {
    let protected = protected_router().route_layer(middleware::from_fn_with_state(
        state.clone(),
        auth::authenticate,
    ));
    public_router()
        .merge(protected)
        .layer(middleware::from_fn(auth::parse_params))
        .with_state(state)
}
