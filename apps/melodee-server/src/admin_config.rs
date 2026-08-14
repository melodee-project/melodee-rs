//! Phase 9 WP-08 Admin API: configuration export/import (PRD-CONFIG-002,
//! PRD-AUTH-007, ADR-0134 §3).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4) over `server_settings` (migration 0025). Authorization: the
//! shared native-JWT administrator gate
//! ([`crate::admin_access::AdminPrincipal`]).
//!
//! - `GET /api/admin/v1/config/export` returns the export document: every
//!   known registry setting's database-or-default value plus custom stored
//!   keys, with every secret value replaced by the fixed redaction marker
//!   and a header warning. Environment/config-file overlays are deployment
//!   concerns and are deliberately not captured (import writes DB rows;
//!   env/file keep winning at runtime — see `melodee-persistence`
//!   `settings` module docs).
//! - `POST /api/admin/v1/config/import?preview=…&allowUnknown=…` validates a
//!   document against the 190-key registry classification (kinds, numeric
//!   ranges, enums) and returns the preview diff (added / changed /
//!   unchanged / secrets by name only / unknown). `preview` defaults to
//!   `true` — an import never applies silently. The redaction marker is
//!   never accepted as a value (fail closed), and unknown keys are rejected
//!   unless `allowUnknown=true`. Applied imports commit their rows and their
//!   bounded audit entry (in the onboarding evidence) atomically.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_persistence::{
    MAX_IMPORT_DOCUMENT_BYTES, SettingsError, SettingsStore, apply_config_import, export_config,
    plan_config_import,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};

/// Stable admin API error envelope (matches `admin.rs`/`admin_access.rs`
/// conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminConfigErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(AdminConfigErrorBody {
            code,
            message,
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn settings_error(error: SettingsError) -> Response {
    let (status, code) = match &error {
        SettingsError::InvalidKey(_) => (StatusCode::BAD_REQUEST, "invalid_key"),
        SettingsError::InvalidValue { .. } => (StatusCode::BAD_REQUEST, "invalid_value"),
        SettingsError::KnownKeyNotDeletable(_) => (StatusCode::CONFLICT, "known_key_not_deletable"),
        SettingsError::RedactedMarker(_) => (StatusCode::BAD_REQUEST, "redacted_marker_rejected"),
        SettingsError::InvalidDocument(_) => (StatusCode::BAD_REQUEST, "invalid_document"),
        SettingsError::ConfigFile(_) => (StatusCode::BAD_REQUEST, "invalid_document"),
        SettingsError::InvalidOnboarding(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        SettingsError::Database(_) => {
            tracing::error!(%error, "admin config store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    // Validation messages name keys and expected shapes only — never
    // echoed values (PRD-AUTH-005/007).
    admin_error(status, code, error.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportParams {
    /// Defaults to true: imports preview unless explicitly applied.
    #[serde(default = "default_preview")]
    pub preview: bool,
    /// Admit keys outside the 190-key registry as custom rows.
    #[serde(default)]
    pub allow_unknown: bool,
}

fn default_preview() -> bool {
    true
}

/// Import response: the preview diff plus whether anything was written.
/// Secret keys are named only; values never appear.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigImportResponse {
    pub applied: bool,
    #[serde(flatten)]
    pub plan: melodee_persistence::ConfigImportPlan,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/config/export", get(export_settings))
        .route("/api/admin/v1/config/import", post(import_settings))
        // Import documents are bounded (settings ≤ 64 KiB each, ≤ 1000 keys).
        .route_layer(axum::extract::DefaultBodyLimit::max(
            MAX_IMPORT_DOCUMENT_BYTES,
        ))
}

async fn export_settings(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
) -> Response {
    match export_config(&state.pool, OffsetDateTime::now_utc()).await {
        Ok(document) => Json(document).into_response(),
        Err(error) => settings_error(error),
    }
}

async fn import_settings(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Query(params): Query<ImportParams>,
    Json(document): Json<serde_json::Value>,
) -> Response {
    let store = SettingsStore::new();
    let stored = match store.list_raw(&state.pool).await {
        Ok(stored) => stored,
        Err(error) => return settings_error(error),
    };
    let plan = match plan_config_import(&document, &stored) {
        Ok(plan) => plan,
        Err(error) => return settings_error(error),
    };
    if params.preview {
        return Json(ConfigImportResponse {
            applied: false,
            plan,
        })
        .into_response();
    }
    let now = OffsetDateTime::now_utc();
    let actor = admin.public_api_id.to_string();
    match apply_config_import(
        &state.pool,
        &document,
        &plan,
        params.allow_unknown,
        &actor,
        now,
    )
    .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                added = plan.added.len(),
                changed = plan.changed.len(),
                secrets_reentered = plan.secrets_reentered.len(),
                unknown = plan.unknown.len(),
                "admin applied configuration import"
            );
            Json(ConfigImportResponse {
                applied: true,
                plan,
            })
            .into_response()
        }
        Err(error) => settings_error(error),
    }
}
