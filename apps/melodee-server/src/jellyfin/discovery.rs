//! Jellyfin discovery and system endpoints (PRD-API-JF-002, PRD-API-JF-004).

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use super::JfState;

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/System/Info/Public", get(system_info_public))
        .route("/System/Ping", get(system_ping).post(system_ping))
        .route("/System/Info", get(system_info))
}

async fn system_info_public(State(state): State<JfState>) -> Response {
    let body = json!({
        "LocalAddress": state.local_address,
        "ServerName": state.server_name,
        "Version": state.server_version,
        "ProductName": state.server_name,
        "OperatingSystem": "Unix",
        "Id": state.server_id,
        "StartupWizardCompleted": true
    });
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

async fn system_ping(State(_state): State<JfState>) -> Response {
    // Captured: Ping returns 204 No Content (GET and POST, anonymous).
    StatusCode::NO_CONTENT.into_response()
}

async fn system_info(State(state): State<JfState>) -> Response {
    let body = json!({
        "OperatingSystemDisplayName": "Unix 7.1.5.201",
        "HasPendingRestart": false,
        "IsShuttingDown": false,
        "SupportsLibraryMonitor": true,
        "WebSocketPortNumber": 0,
        "CanSelfRestart": false,
        "CanLaunchWebBrowser": false,
        "ProgramDataPath": null,
        "ItemsByNamePath": null,
        "CachePath": null,
        "LogPath": null,
        "InternalMetadataPath": null,
        "TranscodingTempPath": null,
        "HasUpdateAvailable": false,
        "LocalAddress": state.local_address,
        "ServerName": state.server_name,
        "Version": state.server_version,
        "ProductName": state.server_name,
        "OperatingSystem": "Unix",
        "Id": state.server_id,
        "StartupWizardCompleted": true
    });
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}
