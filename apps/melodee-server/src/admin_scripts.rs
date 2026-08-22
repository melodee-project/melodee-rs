//! Admin API: scripting hook administration, execution audit,
//! and operator test-run (PRD-SCRIPT-001..005, ADR-0134 §6, ADR-0135).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`, SPEC
//! 19.4) over `script_registry`/`script_runs` (migration 0027).
//! Authorization: the shared native-JWT administrator gate
//! ([`crate::admin_access::AdminPrincipal`]). Scripts are operator-installed
//! only — there is no user-submitted script surface anywhere in the server
//! (ADR-0135 trust boundary). Every mutation is audit-logged via `tracing`
//! with the acting administrator's public id.
//!
//! The test-run endpoint replaces the legacy Blazor editor "test" button
//! (the spec'd legacy REST `POST /api/scripts/validate` was **never
//! implemented** in the frozen tree — inventory §2/§4.8 — and is not
//! recreated as a public route). Each script DTO carries `wired`: only wired
//! events have a live server-side evaluation point; scripts registered for
//! other events are inert until their hook lands.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_persistence::{
    RegisteredScript, ScriptRegistryStore, ScriptRun, ScriptRunStore, ScriptStoreError,
    ScriptUpsert,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};
use crate::scripting::{DEFAULT_TIMEOUT_MS, EvaluationDto, WIRED_EVENTS};

/// Maximum rows returned by listings; clamped even when a client requests
/// more.
const MAX_LIST_LIMIT: u32 = 500;

/// Stable admin API error envelope (matches `admin.rs`/`admin_access.rs`
/// conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminScriptErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(AdminScriptErrorBody {
            code,
            message: code.to_string(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_error(error: ScriptStoreError) -> Response {
    let (status, code) = match &error {
        ScriptStoreError::ScriptNotFound => (StatusCode::NOT_FOUND, "not_found"),
        ScriptStoreError::InvalidScript(_) => (StatusCode::BAD_REQUEST, "invalid_script"),
        ScriptStoreError::InvalidRun(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        ScriptStoreError::Database(_) => {
            tracing::error!(%error, "admin script store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminScriptErrorBody {
            code,
            // Validation messages are static, operator-safe constraint
            // labels — never echoed values.
            message: error.to_string(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

/// One registered script as exposed to operators. `wired` reports whether
/// the event has a live server-side evaluation point in this build.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptDto {
    pub id: Uuid,
    pub event: String,
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub wired: bool,
    pub timeout_ms: i32,
    pub memory_bytes: i64,
    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
}

fn script_dto(script: &RegisteredScript) -> Result<ScriptDto, ()> {
    Ok(ScriptDto {
        id: script.id,
        event: script.event.clone(),
        name: script.name.clone(),
        source: script.source.clone(),
        enabled: script.enabled,
        wired: WIRED_EVENTS.contains(&script.event.as_str()),
        timeout_ms: script.timeout_ms,
        memory_bytes: script.memory_bytes,
        version: script.version,
        created_at: script.created_at.format(&Rfc3339).map_err(|_| ())?,
        updated_at: script.updated_at.format(&Rfc3339).map_err(|_| ())?,
    })
}

/// One execution audit row.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRunDto {
    pub id: Uuid,
    pub script_id: Option<Uuid>,
    pub event: String,
    pub outcome: String,
    pub duration_ms: i64,
    pub error: Option<String>,
    pub created_at: String,
}

fn run_dto(run: &ScriptRun) -> Result<ScriptRunDto, ()> {
    Ok(ScriptRunDto {
        id: run.id,
        script_id: run.script_id,
        event: run.event.clone(),
        outcome: run.outcome.clone(),
        duration_ms: run.duration_ms,
        error: run.error.clone(),
        created_at: run.created_at.format(&Rfc3339).map_err(|_| ())?,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    /// Maximum rows to return (clamped to MAX_LIST_LIMIT). Default 50.
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    /// Zero-based offset for paging.
    #[serde(default)]
    pub offset: u32,
}

fn default_list_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRunsParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Optional audit scoping to one script.
    pub script_id: Option<Uuid>,
}

/// Full-replacement upsert body. `enabled` is never accepted here: a
/// circuit-breaker-disabled script requires the explicit enable route.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertScriptRequest {
    pub source: String,
    pub timeout_ms: Option<i32>,
    pub memory_bytes: Option<i64>,
}

/// Operator test-run request: a stored script (`scriptId`, whose current
/// body is evaluated) or an ad-hoc `source`, against an admin-supplied ctx
/// mock. Exactly one of the two must be present.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunRequest {
    pub event: String,
    pub script_id: Option<Uuid>,
    pub source: Option<String>,
    pub ctx: serde_json::Value,
    /// Overrides the stored script's timeout (bounded by the registry
    /// ceiling); defaults to the stored value or the ADR-0135 default.
    pub timeout_ms: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunResponse {
    pub evaluation: EvaluationDto,
    pub run: ScriptRunDto,
    /// Effective deny action for this build (quarantine moves are deferred;
    /// legacy `delete` stays rewritten to `skip`).
    pub effective_on_deny: &'static str,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/scripts", get(list_scripts))
        .route(
            "/api/admin/v1/scripts/{event}/{name}",
            get(get_script).put(upsert_script),
        )
        .route("/api/admin/v1/scripts/id/{id}", get(get_script_by_id).delete(delete_script))
        .route("/api/admin/v1/scripts/id/{id}/enable", post(enable_script))
        .route("/api/admin/v1/scripts/id/{id}/disable", post(disable_script))
        .route("/api/admin/v1/scripts/runs", get(list_runs))
        .route("/api/admin/v1/scripts/test-run", post(test_run))
 // Source bodies are individually bounded at 256 KiB by the store and
 // ctx payloads by the executor; the envelope admits one of each plus
 // JSON overhead.
        .route_layer(axum::extract::DefaultBodyLimit::max(768 * 1024))
}

/// List response carries the wired-event registry so operators can tell live
/// hooks from inert registrations.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptListResponse {
    pub scripts: Vec<ScriptDto>,
    pub wired_events: &'static [&'static str],
}

async fn list_scripts(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match ScriptRegistryStore::new()
        .list(&state.pool, limit, params.offset)
        .await
    {
        Ok(scripts) => match scripts
            .iter()
            .map(script_dto)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(dtos) => Json(ScriptListResponse {
                scripts: dtos,
                wired_events: WIRED_EVENTS,
            })
            .into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn get_script(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path((event, name)): Path<(String, String)>,
) -> Response {
    match ScriptRegistryStore::new()
        .get_by_name(&state.pool, &event, &name)
        .await
    {
        Ok(script) => match script_dto(&script) {
            Ok(dto) => Json(dto).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn get_script_by_id(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match ScriptRegistryStore::new().get(&state.pool, id).await {
        Ok(script) => match script_dto(&script) {
            Ok(dto) => Json(dto).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn upsert_script(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((event, name)): Path<(String, String)>,
    Json(request): Json<UpsertScriptRequest>,
) -> Response {
    let upsert = ScriptUpsert {
        event: &event,
        name: &name,
        source: &request.source,
        timeout_ms: request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
        // Mirrors the registry column default (advisory; ADR-0135 §3).
        memory_bytes: request.memory_bytes.unwrap_or(16 * 1024 * 1024),
    };
    match ScriptRegistryStore::new()
        .upsert(&state.pool, &upsert, OffsetDateTime::now_utc())
        .await
    {
        Ok(script) => {
            tracing::info!(
                actor = %admin.public_api_id,
                script_id = %script.id,
                event = %script.event,
                name = %script.name,
                version = script.version,
                "admin upserted script"
            );
            match script_dto(&script) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn delete_script(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match ScriptRegistryStore::new().delete(&state.pool, id).await {
        Ok(()) => {
            tracing::info!(actor = %admin.public_api_id, script_id = %id, "admin deleted script");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => store_error(error),
    }
}

async fn set_enabled(
    state: AdminAccessState,
    admin: AdminPrincipal,
    id: Uuid,
    enabled: bool,
) -> Response {
    match ScriptRegistryStore::new()
        .set_enabled(&state.pool, id, enabled, OffsetDateTime::now_utc())
        .await
    {
        Ok(script) => {
            tracing::info!(
                actor = %admin.public_api_id,
                script_id = %id,
                enabled,
                "admin changed script enabled state"
            );
            match script_dto(&script) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn enable_script(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    set_enabled(state, admin, id, true).await
}

async fn disable_script(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    set_enabled(state, admin, id, false).await
}

async fn list_runs(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListRunsParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match ScriptRunStore::new()
        .list_recent(&state.pool, params.script_id, limit, params.offset)
        .await
    {
        Ok(runs) => match runs.iter().map(run_dto).collect::<Result<Vec<_>, _>>() {
            Ok(dtos) => Json(dtos).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

/// Operator verification hook (PRD-SCRIPT-001/004): evaluates a stored or
/// ad-hoc script against an admin-supplied ctx mock in the real sandbox and
/// returns the honestly recorded audit row.
async fn test_run(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<TestRunRequest>,
) -> Response {
    if request.event.trim().is_empty()
        || request.event.chars().count() > melodee_persistence::MAX_SCRIPT_EVENT_CHARS
    {
        return admin_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let (script_id, source, timeout_ms) = match (request.script_id, request.source) {
        (Some(id), None) => match ScriptRegistryStore::new().get(&state.pool, id).await {
            Ok(script) => (
                Some(script.id),
                script.source.clone(),
                request.timeout_ms.unwrap_or(script.timeout_ms),
            ),
            Err(error) => return store_error(error),
        },
        (None, Some(source)) => (
            None,
            source,
            request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
        ),
        // Exactly one of scriptId / source.
        _ => return admin_error(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    if source.trim().is_empty() {
        return admin_error(StatusCode::BAD_REQUEST, "invalid_script");
    }
    match state
        .scripting
        .test_run(&request.event, script_id, &source, timeout_ms, &request.ctx)
        .await
    {
        Ok((evaluation, run)) => {
            tracing::info!(
                actor = %admin.public_api_id,
                event = %request.event,
                script_id = ?script_id,
                outcome = %evaluation.outcome.as_str(),
                "admin script test-run recorded"
            );
            match run_dto(&run) {
                Ok(run) => Json(TestRunResponse {
                    evaluation: EvaluationDto::from(&evaluation),
                    run,
                    effective_on_deny: crate::scripting::EFFECTIVE_ON_DENY,
                })
                .into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(crate::scripting::ScriptingError::Store(error)) => store_error(error),
        Err(error) => {
            tracing::error!(%error, "admin script test-run failed");
            admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    }
}
