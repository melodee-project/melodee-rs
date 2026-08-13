//! Scan endpoint group: startScan, getScanStatus (PRD-API-OS-004; golden
//! evidence `compatibility/opensubsonic/golden/community/os-scan-*`).
//!
//! Captured contract (QuartzDisabled baseline): `startScan` records a new scan
//! run and answers `{scanStatus: {scanning: true, count: 0}}` because the
//! scheduler cannot fire the job; `getScanStatus` answers the last run's state
//! — `{scanning: false, count: <last>}` once the run is closed. Any
//! authenticated user may start a scan (no admin requirement at baseline,
//! per `opensubsonic-capture.md` §6).

use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    payload: serde_json::Value,
) -> Response {
    OsReply::ok(payload).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn scan_status(scanning: bool, count: i64) -> serde_json::Value {
    serde_json::json!({ "scanStatus": { "scanning": scanning, "count": count } })
}

/// Most recent finished or in-progress run, ordered by started_at then id
/// (ADR-0130 decision 5: deterministic tiebreak on the row id).
const LATEST_RUN: &str = "SELECT track_count, finished_at
    FROM opensubsonic_scan_runs
    ORDER BY started_at DESC, id DESC LIMIT 1";

pub async fn start_scan(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    // Any authenticated user may startScan (no admin requirement at baseline).
    // The captured bypass synthesizes no user; a bypass request has no
    // captured scan behavior, so fail closed with the auth error envelope.
    let _ = match &principal {
        Some(Extension(p)) => p,
        None => {
            return OsReply::error(40, "Wrong username or password.").respond(
                &format_of(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    let now = OffsetDateTime::now_utc();
    let user_id = principal.as_ref().map(|Extension(p)| p.user_id);
    if let Err(error) = sqlx::query(
        "INSERT INTO opensubsonic_scan_runs (id, user_id, started_at, finished_at, track_count, created_at)
         VALUES ($1, $2, $3, NULL, 0, $3)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        tracing::error!(%error, "opensubsonic startScan insert failed");
        return OsReply::error(0, "A generic error.")
            .respond(&format_of(&params), &state.server_version, StatusCode::OK);
    }
    // QuartzDisabled baseline: the job cannot fire, so the run reports
    // scanning:true/count:0 immediately even though no tracks will be scanned.
    reply(&state, &params, scan_status(true, 0))
}

pub async fn get_scan_status(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = match &principal {
        Some(Extension(p)) => p,
        None => {
            return OsReply::error(40, "Wrong username or password.").respond(
                &format_of(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    let row = match sqlx::query(LATEST_RUN).fetch_optional(&state.pool).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getScanStatus failed");
            return OsReply::error(0, "A generic error.").respond(
                &format_of(&params),
                &state.server_version,
                StatusCode::OK,
            );
        }
    };
    let (scanning, count) = match row {
        Some(row) => {
            let count: i64 = row.try_get(0).unwrap_or(0);
            let finished_at: Option<OffsetDateTime> = row.try_get(1).unwrap_or(None);
            (finished_at.is_none(), count)
        }
        None => (false, 0),
    };
    reply(&state, &params, scan_status(scanning, count))
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router("startScan", start_scan))
        .merge(super::endpoint_router("getScanStatus", get_scan_status))
}
