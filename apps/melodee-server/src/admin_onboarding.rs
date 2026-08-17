//! Phase 9 WP-08 Admin API: first-run onboarding status and completion
//! (PRD-SETUP-001/002, ADR-0134 §3).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4). Authorization: the shared native-JWT administrator gate
//! ([`crate::admin_access::AdminPrincipal`]) — onboarding status can reveal
//! deployment details (paths, configured features), so it is admin-gated
//! like every other operator surface.
//!
//! - `GET /api/admin/v1/onboarding/status` returns the six preflight checks
//!   (pass/fail + remediation), the completion marker, the migration-bypass
//!   heuristic result (see `crate::onboarding` module docs for the exact
//!   definition), and `canComplete`.
//! - `POST /api/admin/v1/onboarding/complete` re-evaluates the checks and
//!   fails closed with `409` while any required check fails; otherwise it
//!   writes the singleton `onboarding_state` row with the check evidence
//!   (never secret material) and returns the fresh status.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_persistence::OnboardingState;
use serde::Serialize;
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};
use crate::onboarding::{OnboardingError, OnboardingReport, PreflightCheck};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightCheckDto {
    pub name: &'static str,
    pub required: bool,
    pub passed: bool,
    pub detail: String,
    pub remediation: Option<String>,
}

impl From<&PreflightCheck> for PreflightCheckDto {
    fn from(check: &PreflightCheck) -> Self {
        Self {
            name: check.name,
            required: check.required,
            passed: check.passed,
            detail: check.detail.clone(),
            remediation: check.remediation.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingStatusDto {
    pub completed: bool,
    pub bypassed: bool,
    pub can_complete: bool,
    pub completed_by: Option<String>,
    pub checks: Vec<PreflightCheckDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
    pub checks: Vec<PreflightCheckDto>,
}

fn status_dto(report: &OnboardingReport, state: Option<&OnboardingState>) -> OnboardingStatusDto {
    OnboardingStatusDto {
        completed: report.completed,
        bypassed: report.bypassed,
        can_complete: report.can_complete,
        completed_by: state.and_then(|state| state.completed_by.clone()),
        checks: report.checks.iter().map(PreflightCheckDto::from).collect(),
    }
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/onboarding/status", get(onboarding_status))
        .route(
            "/api/admin/v1/onboarding/complete",
            post(complete_onboarding),
        )
}

async fn onboarding_status(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
) -> Response {
    let report = crate::onboarding::evaluate_onboarding(&state.pool).await;
    let stored = melodee_persistence::OnboardingStore::new()
        .get_state(&state.pool)
        .await
        .ok();
    Json(status_dto(&report, stored.as_ref())).into_response()
}

async fn complete_onboarding(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
) -> Response {
    let actor = admin.public_api_id.to_string();
    match crate::onboarding::complete_onboarding(&state.pool, &actor).await {
        Ok(stored) => {
            tracing::info!(
                actor = %admin.public_api_id,
                "admin completed onboarding"
            );
            let report = crate::onboarding::evaluate_onboarding(&state.pool).await;
            Json(status_dto(&report, Some(&stored))).into_response()
        }
        Err(OnboardingError::ChecksFailed(report)) => (
            StatusCode::CONFLICT,
            Json(OnboardingErrorBody {
                code: "onboarding_checks_failed",
                message: "required preflight checks are failing".to_string(),
                correlation_id: crate::correlation::envelope_id(),
                checks: report.checks.iter().map(PreflightCheckDto::from).collect(),
            }),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "onboarding completion failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OnboardingErrorBody {
                    code: "store_error",
                    message: "store_error".to_string(),
                    correlation_id: crate::correlation::envelope_id(),
                    checks: Vec::new(),
                }),
            )
                .into_response()
        }
    }
}
