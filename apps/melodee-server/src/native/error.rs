//! Native-v1 error and authentication rejection shapes.

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeApiErrorBody {
    pub code: &'static str,
    pub message: &'static str,
    pub correlation_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum NativeApiError {
    #[error("bad request")]
    BadRequest,
    #[error("bad request: {0}")]
    CapturedBadRequest(&'static str),
    #[error("resource not found")]
    NotFound,
    #[error("resource not found: {0}")]
    CapturedNotFound(&'static str),
    #[error("captured internal failure: {0}")]
    CapturedInternal(&'static str),
    #[error("access forbidden")]
    Forbidden,
    #[error("native API dependency unavailable")]
    Unavailable,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("search operation failed")]
    Search(#[from] melodee_search::SearchError),
    #[error("native user-state operation failed")]
    UserState(#[from] melodee_persistence::NativeUserStateStoreError),
}

impl NativeApiError {
    fn response_facts(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "BAD_REQUEST", "Invalid request"),
            Self::CapturedBadRequest(message) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", message),
            Self::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND", "Resource not found"),
            Self::CapturedNotFound(message) => (StatusCode::NOT_FOUND, "NOT_FOUND", message),
            Self::CapturedInternal(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
            }
            Self::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN", "Access denied"),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVICE_UNAVAILABLE",
                "Service temporarily unavailable",
            ),
            Self::UserState(melodee_persistence::NativeUserStateStoreError::NotFound) => {
                (StatusCode::NOT_FOUND, "NOT_FOUND", "Resource not found")
            }
            Self::UserState(melodee_persistence::NativeUserStateStoreError::InvalidRequest(_)) => {
                (StatusCode::BAD_REQUEST, "BAD_REQUEST", "Invalid request")
            }
            Self::UserState(melodee_persistence::NativeUserStateStoreError::Conflict) => (
                StatusCode::CONFLICT,
                "CONFLICT",
                "Request conflicts with existing state",
            ),
            Self::Database(_)
            | Self::Search(_)
            | Self::UserState(melodee_persistence::NativeUserStateStoreError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "An unexpected error occurred",
            ),
        }
    }
}

impl IntoResponse for NativeApiError {
    fn into_response(self) -> axum::response::Response {
        if matches!(
            self,
            Self::Database(_) | Self::Search(_) | Self::Unavailable | Self::CapturedInternal(_)
        ) {
            tracing::error!(error = ?self, "native API request failed");
        }
        let (status, code, message) = self.response_facts();
        (
            status,
            Json(NativeApiErrorBody {
                code,
                message,
                correlation_id: crate::correlation::envelope_id(),
            }),
        )
            .into_response()
    }
}

/// Captured JWT rejection is an empty 401, not the JSON error envelope used
/// after a request reaches a native controller.
pub fn unauthorized() -> axum::response::Response {
    StatusCode::UNAUTHORIZED.into_response()
}
