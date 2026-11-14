//! OpenSubsonic authentication (PRD-API-OS-003, ADR-0113, ADR-0130).
//!
//! Transports: query string or urlencoded form POST. Forms: `u + t + s`
//! (token = md5(opensubsonic secret + salt), case-insensitive hex) and
//! `u + p` (plaintext or `enc:` hex) verified against the bcrypt password
//! hash. The legacy `jwt` parameter is never consulted (ADR-0130 decision 1:
//! no JWT branch exists), and `apiKey` is advertised but never honored
//! (ADR-0130 decision 2). The captured localhost / baseUrl / `/images/`
//! bypass synthesizes no user.
//!
//! All failures are the captured identical HTTP 200 + error 40 envelope —
//! except podcast endpoints, which answer HTTP 403 + error 40.

use axum::{
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use md5::{Digest, Md5};
use melodee_persistence::{AuthLookup, AuthStore};
use uuid::Uuid;

use super::{
    OsState,
    envelope::{Format, OsReply, negotiate},
    params::{self, Params},
};

const MAX_FORM_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Authenticated OpenSubsonic identity installed by [`authenticate`].
#[derive(Debug, Clone)]
pub struct OsPrincipal {
    pub user_id: Uuid,
    pub public_api_id: Uuid,
    pub username: String,
    pub is_admin: bool,
    pub is_editor: bool,
    pub roles: Vec<String>,
}

impl OsPrincipal {
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles
            .iter()
            .any(|held| held.eq_ignore_ascii_case(role))
    }
}

/// Marker extension: the captured bypass applies (local request, host equals
/// the configured base URL, or the path contains `/images/`). No user is
/// synthesized — handlers see no [`OsPrincipal`], matching the legacy
/// BlankUserInfo behavior.
#[derive(Debug, Clone, Copy)]
pub struct OsBypass;

fn is_podcast_path(path: &str) -> bool {
    path.to_ascii_lowercase().contains("podcast")
}

fn auth_failure(params: &Params, path: &str, server_version: &str) -> Response {
    // Captured podcast-controller quirk (`os-getpodcasts-norole`): auth
    // failures on podcast routes return HTTP 403 with the raw ResponseData
    // wrapper, not the subsonic envelope other controllers use.
    if is_podcast_path(path) {
        return super::podcasts::podcasts_raw_error(
            server_version,
            None,
            StatusCode::FORBIDDEN,
            40,
            "Wrong username or password.",
        );
    }
    OsReply::error(40, "Wrong username or password.").respond(
        &negotiate(params),
        server_version,
        StatusCode::OK,
    )
}

fn is_bypass(request: &Request, state: &OsState) -> bool {
    if !state.legacy_auth_bypass {
        return false;
    }
    if request.uri().path().contains("/images/") {
        return true;
    }
    // Captured bypass semantics require the EFFECTIVE client address: the
    // legacy server ran with UseForwardedHeaders, so a trusted proxy's
    // Forwarded/X-Forwarded-For replaces RemoteIp (the capture
    // defeated the bypass exactly this way) and X-Forwarded-Host replaces the
    // Host used for the baseUrl comparison.
    let Some(connect_info) = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .copied()
    else {
        return false;
    };
    let peer = connect_info.0;
    let header_str = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let Ok(client_ip) = state.client_addresses.resolve(
        peer,
        header_str("forwarded"),
        header_str("x-forwarded-for"),
    ) else {
        // Malformed proxy data must not fall back to the loopback proxy peer.
        return false;
    };
    if client_ip.is_loopback() {
        return true;
    }
    if let Some(base_host) = &state.base_url_host {
        let forwarded_host = state
            .client_addresses
            .is_trusted(peer.ip())
            .then(|| header_str("x-forwarded-host"))
            .flatten();
        let host = forwarded_host
            .or_else(|| header_str("host"))
            .map(|value| value.split(':').next().unwrap_or(value));
        if host == Some(base_host.as_str()) {
            return true;
        }
    }
    false
}

/// Parses query + form parameters once and installs them for downstream
/// handlers. Applied to every `/rest` route so handlers never re-read bodies.
pub async fn parse_params(mut request: Request, next: Next) -> Response {
    let mut params = Params::default();
    if let Some(query) = request.uri().query() {
        params.merge_query(query);
    }
    let is_form = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .starts_with("application/x-www-form-urlencoded")
        });
    if is_form {
        let (parts, body) = request.into_parts();
        match to_bytes(body, MAX_FORM_BODY_BYTES).await {
            Ok(bytes) => {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    params.merge_query(text);
                }
                request = Request::from_parts(parts, Body::from(bytes.to_vec()));
            }
            Err(_) => {
                return OsReply::error(40, "Wrong username or password.").respond(
                    &Format::Xml,
                    "0.0.0",
                    StatusCode::OK,
                );
            }
        }
    }
    request.extensions_mut().insert(params);
    next.run(request).await
}

/// OpenSubsonic authentication middleware for protected routes.
pub async fn authenticate(
    State(state): State<OsState>,
    mut request: Request,
    next: Next,
) -> Response {
    let params = request
        .extensions()
        .get::<Params>()
        .cloned()
        .unwrap_or_else(|| {
            let mut params = Params::default();
            if let Some(query) = request.uri().query() {
                params.merge_query(query);
            }
            params
        });
    let path = request.uri().path().to_string();

    if is_bypass(&request, &state) {
        request.extensions_mut().insert(OsBypass);
        return next.run(request).await;
    }

    let Some(username) = params.get("u") else {
        return auth_failure(&params, &path, &state.server_version);
    };
    let normalized = username.trim().to_uppercase();
    let record = match AuthStore::new()
        .user_for_auth(&state.pool, AuthLookup::Username(&normalized))
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => return auth_failure(&params, &path, &state.server_version),
        Err(error) => {
            tracing::error!(%error, "opensubsonic auth lookup failed");
            return auth_failure(&params, &path, &state.server_version);
        }
    };
    if !record.user.can_authenticate() {
        return auth_failure(&params, &path, &state.server_version);
    }

    // The `jwt` and `apiKey` parameters are deliberately never consulted
    // (ADR-0130 decisions 1-2). Only the token and password branches decide.
    let authenticated = match (params.get("t"), params.get("s")) {
        (Some(token), Some(salt)) => match load_user_secret(&state, record.user.id).await {
            Ok(Some(secret)) => {
                let mut hasher = Md5::new();
                hasher.update(secret.as_bytes());
                hasher.update(salt.as_bytes());
                let expected = params::hex_encode(&hasher.finalize());
                expected.eq_ignore_ascii_case(token)
            }
            _ => false,
        },
        _ => match params.get("p") {
            Some(password) => {
                let decoded = if let Some(hex) = password.strip_prefix("enc:") {
                    params::hex_decode(hex).and_then(|bytes| String::from_utf8(bytes).ok())
                } else {
                    Some(password.to_string())
                };
                match decoded {
                    Some(candidate) => crate::password_work::verify_password(
                        candidate.as_bytes(),
                        record.password_hash_algorithm,
                        record.password_hash.expose_secret(),
                    )
                    .await
                    .unwrap_or(false),
                    None => false,
                }
            }
            None => false,
        },
    };
    if !authenticated {
        return auth_failure(&params, &path, &state.server_version);
    }

    request.extensions_mut().insert(OsPrincipal {
        user_id: record.user.id,
        public_api_id: record.user.public_api_id,
        username: record.user.username,
        is_admin: record.user.is_admin,
        is_editor: record.user.is_editor,
        roles: record.user.roles,
    });
    next.run(request).await
}

async fn load_user_secret(state: &OsState, user_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT secret FROM opensubsonic_user_secrets WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn podcast_paths_are_detected_case_insensitively() {
        assert!(is_podcast_path("/rest/getPodcasts.view"));
        assert!(is_podcast_path("/rest/streamPodcastEpisode"));
        assert!(!is_podcast_path("/rest/ping.view"));
    }
}
