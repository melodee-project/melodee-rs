//! Jellyfin error envelope (PRD-API-JF-001). The captured ProblemDetails shape:
//! `{type, title, status, detail, traceId, errorCode}` with `application/json;
//! charset=utf-8`. Failures are indistinguishable across token failure causes.

use axum::{
    Json,
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct JellyfinProblemDetails {
    #[serde(rename = "type")]
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    #[serde(rename = "traceId")]
    pub trace_id: Uuid,
    #[serde(rename = "errorCode")]
    pub error_code: Option<String>,
}

impl JellyfinProblemDetails {
    pub fn unauthorized(detail: &str) -> Self {
        Self {
            r#type: "about:blank".to_string(),
            title: "Unauthorized".to_string(),
            status: 401,
            detail: detail.to_string(),
            trace_id: Uuid::now_v7(),
            error_code: None,
        }
    }

    pub fn bad_request(detail: &str) -> Self {
        Self {
            r#type: "about:blank".to_string(),
            title: "Bad Request".to_string(),
            status: 400,
            detail: detail.to_string(),
            trace_id: Uuid::now_v7(),
            error_code: None,
        }
    }

    pub fn not_found(detail: &str) -> Self {
        Self {
            r#type: "about:blank".to_string(),
            title: "Not Found".to_string(),
            status: 404,
            detail: detail.to_string(),
            trace_id: Uuid::now_v7(),
            error_code: None,
        }
    }

    pub fn into_json_response(self, status: StatusCode) -> Response {
        let mut response = (status, Json(self)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        response
    }
}

/// Returns the captured 401 ProblemDetails for any token failure (missing,
/// garbage, expired, or revoked — all indistinguishable, ADR-0131).
pub fn unauthorized() -> Response {
    JellyfinProblemDetails::unauthorized("Missing or invalid authentication token.")
        .into_json_response(StatusCode::UNAUTHORIZED)
}

/// Returns the captured 400 ProblemDetails for a malformed AuthenticateByName
/// body (missing `Pw` or `Username`).
pub fn bad_request(detail: &str) -> Response {
    JellyfinProblemDetails::bad_request(detail).into_json_response(StatusCode::BAD_REQUEST)
}

/// Returns the captured 404 ProblemDetails for an unknown item id.
pub fn not_found(detail: &str) -> Response {
    JellyfinProblemDetails::not_found(detail).into_json_response(StatusCode::NOT_FOUND)
}

/// Returns the captured 403 ProblemDetails.
pub fn forbidden(detail: &str) -> Response {
    let problem = JellyfinProblemDetails {
        r#type: "about:blank".to_string(),
        title: "Forbidden".to_string(),
        status: 403,
        detail: detail.to_string(),
        trace_id: Uuid::now_v7(),
        error_code: None,
    };
    problem.into_json_response(StatusCode::FORBIDDEN)
}

/// 500 ProblemDetails for unexpected internal failures.
pub fn internal_server_error() -> Response {
    let problem = JellyfinProblemDetails {
        r#type: "about:blank".to_string(),
        title: "Internal Server Error".to_string(),
        status: 500,
        detail: "An unexpected error occurred.".to_string(),
        trace_id: Uuid::now_v7(),
        error_code: None,
    };
    problem.into_json_response(StatusCode::INTERNAL_SERVER_ERROR)
}

/// 416 ProblemDetails for unsatisfiable byte ranges (captured Jellyfin
/// behavior, distinct from OpenSubsonic's 404 and native's 400).
pub fn range_not_satisfiable(detail: &str) -> Response {
    let problem = JellyfinProblemDetails {
        r#type: "about:blank".to_string(),
        title: "Range Not Satisfiable".to_string(),
        status: 416,
        detail: detail.to_string(),
        trace_id: Uuid::now_v7(),
        error_code: None,
    };
    let response = problem.into_json_response(StatusCode::RANGE_NOT_SATISFIABLE);
    let _ = Body::default;
    response
}
