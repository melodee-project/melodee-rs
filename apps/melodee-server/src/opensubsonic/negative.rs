//! Intentional Gone (410) and Not Implemented (501) endpoints
//! (PRD-API-OS-005). Every captured negative endpoint answers its exact
//! status with an empty body and `Cache-Control: no-cache`. Silently turning
//! these into 404/500 is a compatibility regression; changing them requires
//! the full contract-change process (compatibility test, product decision,
//! accepted ADR, migration/client plan).

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{OsState, endpoint_router};

async fn gone() -> Response {
    negative(StatusCode::GONE)
}

async fn not_implemented() -> Response {
    negative(StatusCode::NOT_IMPLEMENTED)
}

fn negative(status: StatusCode) -> Response {
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// jukeboxControl is a conditional endpoint whose captured disabled and
/// enabled-without-backend states collapse to the same 410 (see
/// `opensubsonic-capture.md` §6), so it lives with the Gone handlers.
pub fn router() -> Router<OsState> {
    Router::new()
        .merge(endpoint_router("getChatMessages", gone))
        .merge(endpoint_router("addChatMessage", gone))
        .merge(endpoint_router("hls", gone))
        .merge(endpoint_router("getCaptions", gone))
        .merge(endpoint_router("getVideoInfo", gone))
        .merge(endpoint_router("getVideos", gone))
        .merge(endpoint_router("jukeboxControl", gone))
        .merge(endpoint_router("getUsers", not_implemented))
        .merge(endpoint_router("updateUser", not_implemented))
        .merge(endpoint_router("deleteUser", not_implemented))
        .merge(endpoint_router("changePassword", not_implemented))
}
