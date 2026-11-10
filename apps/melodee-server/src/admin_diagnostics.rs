//! Admin diagnostics API (`/api/admin/v1/diagnostics/*`).
//!
//! Purpose-built for troubleshooting setup and runtime failures — by an
//! operator or by a coding agent driving the HTTP API — without shell access
//! to the host. Every route requires the shared administrator gate
//! ([`AdminPrincipal`]); there is no anonymous diagnostics surface.
//!
//! Routes:
//!
//! - `GET /api/admin/v1/diagnostics/selftest` — deterministic pass/warn/fail
//!   checks (database, migrations, settings resolution, search, storage,
//!   streaming admission, configured deny lists, notifications) with a
//!   stable `code`, an operator `hint`, and redacted details.
//! - `GET /api/admin/v1/diagnostics/settings` — every effective setting with
//!   its winning layer and the runtime consumer/inert rationale from
//!   [`crate::settings_inventory`]. Secret values are always redacted.
//! - `GET /api/admin/v1/diagnostics/events?limit=N` — the bounded ring buffer
//!   of recent WARN/ERROR events with correlation ids.
//! - `GET /api/admin/v1/diagnostics/bundle` — a single redacted support bundle
//!   combining version/build identity, uptime, selftest, settings summary,
//!   streaming admission, and recent events. Intended to be attached to an
//!   issue or handed to an agent.
//!
//! Nothing here mutates state. Values that could leak credentials (secrets,
//! SMTP passwords, Last.fm keys, JWKS URLs) are redacted or reduced to
//! booleans/counts by construction.

use axum::{
    Json, Router,
    extract::{FromRequestParts, Query, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::{SettingKind, SettingsResolver, known_setting, migration_status};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};
use crate::settings_inventory::SETTING_INVENTORY;

/// Shared state for the diagnostics routes.
#[derive(Clone)]
pub struct AdminDiagnosticsState {
    pub admin: AdminAccessState,
    pub search: Option<melodee_search::SearchConfig>,
    pub direct_play: Option<crate::native_stream::DirectPlayService>,
    /// Process-wide direct-play capacity even when direct play is disabled.
    pub global_stream_cap: usize,
    pub per_user_stream_cap: usize,
    pub started_at: OffsetDateTime,
}

/// The administrator gate reused for this router's distinct state type. It
/// delegates to the shared extractor so there is exactly one authorization
/// implementation.
impl FromRequestParts<AdminDiagnosticsState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminDiagnosticsState,
    ) -> Result<Self, Self::Rejection> {
        <Self as FromRequestParts<AdminAccessState>>::from_request_parts(parts, &state.admin).await
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsErrorBody {
    code: &'static str,
    message: String,
    correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(DiagnosticsErrorBody {
            code,
            message: message.into(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin diagnostics store failure");
    admin_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "store_error",
        format!("{context}: {error}"),
    )
}

pub fn router() -> Router<AdminDiagnosticsState> {
    Router::new()
        .route("/api/admin/v1/diagnostics/selftest", get(selftest))
        .route("/api/admin/v1/diagnostics/settings", get(settings))
        .route("/api/admin/v1/diagnostics/events", get(events))
        .route("/api/admin/v1/diagnostics/bundle", get(bundle))
        .route(
            "/api/admin/v1/diagnostics/stream-probe",
            axum::routing::post(stream_probe),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    const fn rank(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Warn => 1,
            Self::Fail => 2,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    /// Stable machine code (`database`, `migrations`, `settings`, ...).
    pub code: &'static str,
    pub status: CheckStatus,
    pub summary: String,
    /// Redacted, non-secret detail for humans and agents.
    pub detail: Option<serde_json::Value>,
    /// What to do next when the check is not `pass`.
    pub hint: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTestReport {
    pub generated_at: String,
    pub overall: CheckStatus,
    pub checks: Vec<DiagnosticCheck>,
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

async fn database_check(pool: &sqlx::PgPool) -> DiagnosticCheck {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
    {
        Ok(1) => DiagnosticCheck {
            code: "database",
            status: CheckStatus::Pass,
            summary: "PostgreSQL query succeeded".to_string(),
            detail: None,
            hint: None,
        },
        Ok(_) | Err(_) => DiagnosticCheck {
            code: "database",
            status: CheckStatus::Fail,
            summary: "PostgreSQL is unreachable or returned an unexpected result".to_string(),
            detail: None,
            hint: Some(
                "Check DATABASE_URL, PostgreSQL availability, and pool limits; `melodee-cli doctor` prints the same probe.",
            ),
        },
    }
}

async fn migration_check(pool: &sqlx::PgPool) -> DiagnosticCheck {
    match migration_status(pool).await {
        Ok(status) if status.is_current() => DiagnosticCheck {
            code: "migrations",
            status: CheckStatus::Pass,
            summary: format!(
                "schema is current (migration {} of {})",
                status.applied, status.latest_available
            ),
            detail: Some(json!({
                "applied": status.applied,
                "latestAvailable": status.latest_available,
                "allSuccessful": status.all_successful,
            })),
            hint: None,
        },
        Ok(status) => DiagnosticCheck {
            code: "migrations",
            status: CheckStatus::Fail,
            summary: "database schema is not at the binary's migration head".to_string(),
            detail: Some(json!({
                "applied": status.applied,
                "latestAvailable": status.latest_available,
                "allSuccessful": status.all_successful,
            })),
            hint: Some(
                "Start the server once to apply migrations, or restore a backup taken from this binary's schema head.",
            ),
        },
        Err(error) => DiagnosticCheck {
            code: "migrations",
            status: CheckStatus::Fail,
            summary: "migration status could not be read".to_string(),
            detail: Some(json!({ "error": error.to_string() })),
            hint: Some(
                "Check database permissions on `_sqlx_migrations` and `melodee-cli doctor`.",
            ),
        },
    }
}

async fn settings_check(pool: &sqlx::PgPool) -> DiagnosticCheck {
    let resolver = SettingsResolver::new(pool);
    match resolver.resolve_all().await {
        Ok(resolved) => {
            let unverified = resolved
                .iter()
                .filter(|setting| {
                    crate::settings_inventory::setting_inventory(&setting.key).is_none_or(|entry| {
                        entry.consumer.is_empty() && entry.inert_reason.is_empty()
                    })
                })
                .count();
            let from_environment = resolved
                .iter()
                .filter(|setting| setting.source == melodee_persistence::SettingSource::Environment)
                .count();
            let status = if unverified == 0 {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            };
            DiagnosticCheck {
                code: "settings",
                status,
                summary: format!(
                    "{} settings resolved; {unverified} without a documented runtime consumer",
                    resolved.len()
                ),
                detail: Some(json!({
                    "resolved": resolved.len(),
                    "unverified": unverified,
                    "fromEnvironment": from_environment,
                    "inventoried": SETTING_INVENTORY.len(),
                })),
                hint: (unverified > 0).then_some(
                    "Open GET /api/admin/v1/diagnostics/settings and inspect `consumer`/`inertReason` for the affected keys.",
                ),
            }
        }
        Err(error) => DiagnosticCheck {
            code: "settings",
            status: CheckStatus::Fail,
            summary: "layered settings resolution failed".to_string(),
            detail: Some(json!({ "error": error.to_string() })),
            hint: Some(
                "Check the config file (`MELODEE__CONFIG_FILE`), the environment layer, and the `server_settings` table.",
            ),
        },
    }
}

async fn storage_check(state: &AdminDiagnosticsState) -> DiagnosticCheck {
    let report = crate::onboarding::evaluate_onboarding(&state.admin.pool).await;
    let relevant: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.name == "storage_paths" || check.name == "media_tools")
        .collect();
    let failing: Vec<&str> = relevant
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.name)
        .collect();
    let status = if relevant.is_empty() {
        CheckStatus::Warn
    } else if failing.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    };
    DiagnosticCheck {
        code: "storage",
        status,
        summary: if failing.is_empty() {
            "configured storage roles and media tools are usable".to_string()
        } else {
            format!("storage/media preflight failures: {}", failing.join(", "))
        },
        detail: Some(json!(
            relevant
                .iter()
                .map(|check| json!({
                    "name": check.name,
                    "required": check.required,
                    "passed": check.passed,
                    "detail": check.detail,
                    "remediation": check.remediation,
                }))
                .collect::<Vec<_>>()
        )),
        hint: (!failing.is_empty()).then_some(
            "Verify the canonical/inbound/staging/work roots exist, are absolute, do not overlap, and are writable by the server user; verify ffprobe/ffmpeg/fpcalc are on PATH or configured.",
        ),
    }
}

fn search_check(state: &AdminDiagnosticsState) -> DiagnosticCheck {
    match &state.search {
        Some(config) => {
            let root = &config.search_root;
            let status = if root.is_dir() {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            };
            DiagnosticCheck {
                code: "search",
                status,
                summary: if root.is_dir() {
                    "search projection is configured and its root exists".to_string()
                } else {
                    "search projection root is missing".to_string()
                },
                detail: Some(json_eq_root(root)),
                hint: (status == CheckStatus::Fail)
                    .then_some("Verify the search root path/permissions, then rebuild from GET /api/admin/v1/search/rebuild."),
            }
        }
        None => DiagnosticCheck {
            code: "search",
            status: CheckStatus::Warn,
            summary: "search projection is disabled".to_string(),
            detail: None,
            hint: Some(
                "Set the search-enabled configuration when server-side search is required; catalog and streaming are unaffected.",
            ),
        },
    }
}

fn json_eq_root(root: &std::path::Path) -> serde_json::Value {
    json!({ "root": root.display().to_string() })
}

fn streaming_check(state: &AdminDiagnosticsState) -> DiagnosticCheck {
    match &state.direct_play {
        Some(service) => {
            let available = service.available_permits();
            let capacity = service.capacity();
            DiagnosticCheck {
                code: "streaming",
                status: CheckStatus::Pass,
                summary: format!(
                    "direct play admitted {}/{} streams; per-user cap {}",
                    capacity.saturating_sub(available),
                    capacity,
                    if state.per_user_stream_cap == 0 {
                        "disabled".to_string()
                    } else {
                        state.per_user_stream_cap.to_string()
                    }
                ),
                detail: Some(json!({
                    "capacity": capacity,
                    "available": available,
                    "admitted": capacity.saturating_sub(available),
                    "perUserCap": state.per_user_stream_cap,
                })),
                hint: (available == 0).then_some(
                    "All direct-play permits are in use; the server queues new streams. Check `/api/admin/v1/workers/failures` and client reconnect behavior.",
                ),
            }
        }
        None => DiagnosticCheck {
            code: "streaming",
            status: CheckStatus::Fail,
            summary: "direct play is not configured (canonical storage root missing)".to_string(),
            detail: Some(json!({ "globalCap": state.global_stream_cap })),
            hint: Some(
                "Configure the canonical storage/publication root and restart; streaming and artwork serving require it.",
            ),
        },
    }
}

async fn blacklist_check(state: &AdminDiagnosticsState) -> DiagnosticCheck {
    match state.admin.auth.blacklist() {
        Some(cache) => {
            let status = cache.status().await;
            DiagnosticCheck {
                code: "blacklist",
                status: CheckStatus::Pass,
                summary: format!(
                    "configured deny lists loaded: {} email(s), {} IP(s)",
                    status.blacklisted_email_count, status.blacklisted_ip_count
                ),
                detail: Some(json!({
                    "blacklistedEmailCount": status.blacklisted_email_count,
                    "blacklistedIpCount": status.blacklisted_ip_count,
                    "cacheAgeSeconds": status.age_seconds,
                })),
                hint: None,
            }
        }
        None => DiagnosticCheck {
            code: "blacklist",
            status: CheckStatus::Warn,
            summary: "configuration deny lists are not installed in this process".to_string(),
            detail: None,
            hint: Some(
                "Restart the server so `security.blacklistedEmails`/`.blacklistedIPs` are resolved.",
            ),
        },
    }
}

fn notification_check(state: &AdminDiagnosticsState) -> DiagnosticCheck {
    let settings = state.admin.notifications.settings_summary();
    if settings.enabled && settings.smtp_configured {
        DiagnosticCheck {
            code: "notifications",
            status: CheckStatus::Pass,
            summary: "email delivery is enabled and SMTP is configured".to_string(),
            detail: Some(json!({
                "fromEmail": settings.from_email,
                "smtpHost": settings.smtp_host,
                "smtpPort": settings.smtp_port,
            })),
            hint: None,
        }
    } else {
        DiagnosticCheck {
            code: "notifications",
            status: CheckStatus::Warn,
            summary: if settings.enabled {
                "email delivery is enabled but SMTP is not fully configured".to_string()
            } else {
                "email delivery is disabled (fail-degraded)".to_string()
            },
            detail: None,
            hint: Some(
                "Configure `email.enabled`, `email.smtpHost`, `email.smtpPort`, and the from-address; delivery failures never fail the request path.",
            ),
        }
    }
}

async fn selftest_checks(state: &AdminDiagnosticsState) -> Vec<DiagnosticCheck> {
    let pool = &state.admin.pool;
    vec![
        database_check(pool).await,
        migration_check(pool).await,
        settings_check(pool).await,
        storage_check(state).await,
        search_check(state),
        streaming_check(state),
        blacklist_check(state).await,
        notification_check(state),
    ]
}

fn overall(checks: &[DiagnosticCheck]) -> CheckStatus {
    checks
        .iter()
        .map(|check| check.status)
        .max_by_key(|status| status.rank())
        .unwrap_or(CheckStatus::Pass)
}

/// GET /api/admin/v1/diagnostics/selftest
async fn selftest(State(state): State<AdminDiagnosticsState>, _admin: AdminPrincipal) -> Response {
    let checks = selftest_checks(&state).await;
    let report = SelfTestReport {
        generated_at: now_rfc3339(),
        overall: overall(&checks),
        checks,
    };
    if report.overall == CheckStatus::Fail {
        tracing::warn!("admin selftest reported a failing check");
    }
    Json(report).into_response()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingView {
    key: String,
    value: serde_json::Value,
    source: &'static str,
    is_secret: bool,
    /// Rust module that consumes the key, when known.
    consumer: Option<String>,
    /// Why the key has no runtime consumer.
    inert_reason: Option<String>,
    /// Registered kind (e.g. `boolean`, `integer`) — helps validate a value.
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsReport {
    generated_at: String,
    total: usize,
    secret_count: usize,
    settings: Vec<SettingView>,
}

/// GET /api/admin/v1/diagnostics/settings
async fn settings(State(state): State<AdminDiagnosticsState>, _admin: AdminPrincipal) -> Response {
    let resolver = SettingsResolver::new(&state.admin.pool);
    let resolved = match resolver.resolve_all().await {
        Ok(resolved) => resolved,
        Err(error) => return store_failure("settings resolution failed", error),
    };
    let secret_count = resolved.iter().filter(|setting| setting.is_secret).count();
    let settings = resolved
        .into_iter()
        .map(|setting| {
            let inventory = crate::settings_inventory::setting_inventory(&setting.key);
            let known = known_setting(&setting.key);
            SettingView {
                value: setting.to_redacted_json(),
                source: setting.source.as_str(),
                is_secret: setting.is_secret,
                consumer: inventory
                    .map(|entry| entry.consumer)
                    .filter(|consumer| !consumer.is_empty())
                    .map(str::to_string),
                inert_reason: inventory
                    .map(|entry| entry.inert_reason)
                    .filter(|reason| !reason.is_empty())
                    .map(str::to_string),
                kind: known.map(|known| {
                    match known.kind {
                        SettingKind::Bool => "boolean",
                        SettingKind::Int { .. } => "integer",
                        SettingKind::Float { .. } => "number",
                        SettingKind::Enum(_) => "enum",
                        SettingKind::Text => "text",
                    }
                    .to_string()
                }),
                key: setting.key,
            }
        })
        .collect::<Vec<_>>();
    Json(SettingsReport {
        generated_at: now_rfc3339(),
        total: settings.len(),
        secret_count,
        settings,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventQuery {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsReport {
    generated_at: String,
    capacity: usize,
    retained: usize,
    events: Vec<crate::diagnostics::DiagnosticsEvent>,
}

/// GET /api/admin/v1/diagnostics/events?limit=N
async fn events(
    State(_state): State<AdminDiagnosticsState>,
    _admin: AdminPrincipal,
    Query(query): Query<EventQuery>,
) -> Response {
    let Some(buffer) = crate::diagnostics::global() else {
        return admin_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "events_unavailable",
            "recent-event buffer is not installed in this process",
        );
    };
    let limit = query.limit.unwrap_or(100).min(1000);
    let events = buffer.snapshot(limit);
    Json(EventsReport {
        generated_at: now_rfc3339(),
        capacity: buffer.capacity(),
        retained: buffer.snapshot(0).len(),
        events,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamProbeRequest {
    track_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamProbeReport {
    track_id: Uuid,
    /// `ok`, `track_not_found`, `asset_file_missing`, `asset_size_mismatch`,
    /// or `direct_play_disabled`.
    verdict: &'static str,
    release_id: Option<Uuid>,
    asset_relative_path: Option<String>,
    byte_size: Option<u64>,
    mime_type: Option<String>,
    file_present: bool,
    direct_play_configured: bool,
    admitted: Option<usize>,
    capacity: Option<usize>,
    per_user_cap: usize,
    hint: Option<&'static str>,
}

/// POST /api/admin/v1/diagnostics/stream-probe
///
/// Answers "why will this Track not play?" for the exact resolution path the
/// native/OpenSubsonic/Jellyfin stream handlers use: active Release, playable
/// media asset, recorded byte size versus the file on disk, and current
/// direct-play admission. Read-only.
async fn stream_probe(
    State(state): State<AdminDiagnosticsState>,
    _admin: AdminPrincipal,
    Json(request): Json<StreamProbeRequest>,
) -> Response {
    let row: Option<(Uuid, String, i64, String)> = match sqlx::query_as(
        "SELECT r.public_api_id, m.canonical_relative_path, m.byte_size, m.mime_type
         FROM tracks t
         JOIN releases r ON r.id = t.release_id AND r.status = 'active'
         JOIN media_assets m ON m.id = t.playable_media_asset_id
         WHERE t.public_api_id = $1",
    )
    .bind(request.track_id)
    .fetch_optional(&state.admin.pool)
    .await
    {
        Ok(row) => row,
        Err(error) => return store_failure("stream probe query failed", error),
    };
    let Some((release_id, relative_path, byte_size, mime_type)) = row else {
        return Json(StreamProbeReport {
            track_id: request.track_id,
            verdict: "track_not_found",
            release_id: None,
            asset_relative_path: None,
            byte_size: None,
            mime_type: None,
            file_present: false,
            direct_play_configured: state.direct_play.is_some(),
            admitted: None,
            capacity: state.direct_play.as_ref().map(|service| service.capacity()),
            per_user_cap: state.per_user_stream_cap,
            hint: Some(
                "The Track has no active Release with a playable media asset. Check publication/catalog state for this Track.",
            ),
        })
        .into_response();
    };
    let Ok(byte_size) = u64::try_from(byte_size) else {
        return store_failure("stream probe size invalid", "negative byte size");
    };
    let Some(service) = state.direct_play.as_ref() else {
        return Json(StreamProbeReport {
            track_id: request.track_id,
            verdict: "direct_play_disabled",
            release_id: Some(release_id),
            asset_relative_path: Some(relative_path),
            byte_size: Some(byte_size),
            mime_type: Some(mime_type),
            file_present: false,
            direct_play_configured: false,
            admitted: None,
            capacity: None,
            per_user_cap: state.per_user_stream_cap,
            hint: Some(
                "Direct play is not configured: the canonical storage/publication root is missing. Configure it and restart.",
            ),
        })
        .into_response();
    };
    let asset = crate::native_stream::PlayableAsset {
        canonical_relative_path: relative_path.clone(),
        byte_size,
        mime_type: mime_type.clone(),
    };
    let file_present = service.asset_exists(&asset);
    let (verdict, hint) = if file_present {
        ("ok", None)
    } else {
        (
            "asset_file_missing",
            Some(
                "The catalog records this asset but the file is missing or its size differs from the recorded byte size. Re-run the storage/publication check and rebuild the catalog if the file was replaced.",
            ),
        )
    };
    Json(StreamProbeReport {
        track_id: request.track_id,
        verdict,
        release_id: Some(release_id),
        asset_relative_path: Some(relative_path),
        byte_size: Some(byte_size),
        mime_type: Some(mime_type),
        file_present,
        direct_play_configured: true,
        admitted: Some(
            service
                .capacity()
                .saturating_sub(service.available_permits()),
        ),
        capacity: Some(service.capacity()),
        per_user_cap: state.per_user_stream_cap,
        hint,
    })
    .into_response()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleReport {
    generated_at: String,
    server: serde_json::Value,
    uptime_seconds: i64,
    selftest: SelfTestReport,
    settings: serde_json::Value,
    streaming: serde_json::Value,
    recent_events: Vec<crate::diagnostics::DiagnosticsEvent>,
}

/// GET /api/admin/v1/diagnostics/bundle
async fn bundle(State(state): State<AdminDiagnosticsState>, _admin: AdminPrincipal) -> Response {
    let checks = selftest_checks(&state).await;
    let selftest = SelfTestReport {
        generated_at: now_rfc3339(),
        overall: overall(&checks),
        checks,
    };
    let resolver = SettingsResolver::new(&state.admin.pool);
    let settings = match resolver.resolve_all().await {
        Ok(resolved) => {
            let secrets = resolved.iter().filter(|setting| setting.is_secret).count();
            let unverified = resolved
                .iter()
                .filter(|setting| {
                    crate::settings_inventory::setting_inventory(&setting.key).is_none_or(|entry| {
                        entry.consumer.is_empty() && entry.inert_reason.is_empty()
                    })
                })
                .count();
            json!({
                "total": resolved.len(),
                "secrets": secrets,
                "unverified": unverified,
                "sourceCounts": {
                    "environment": resolved.iter().filter(|s| s.source == melodee_persistence::SettingSource::Environment).count(),
                    "configFile": resolved.iter().filter(|s| s.source == melodee_persistence::SettingSource::ConfigFile).count(),
                    "database": resolved.iter().filter(|s| s.source == melodee_persistence::SettingSource::Database).count(),
                    "default": resolved.iter().filter(|s| s.source == melodee_persistence::SettingSource::Default).count(),
                }
            })
        }
        Err(error) => json!({ "error": error.to_string() }),
    };
    let streaming = state.direct_play.as_ref().map_or_else(
        || json!({ "configured": false, "globalCap": state.global_stream_cap, "perUserCap": state.per_user_stream_cap }),
        |service| {
            json!({
                "configured": true,
                "capacity": service.capacity(),
                "available": service.available_permits(),
                "admitted": service.capacity().saturating_sub(service.available_permits()),
                "perUserCap": state.per_user_stream_cap,
            })
        },
    );
    let recent_events =
        crate::diagnostics::global().map_or_else(Vec::new, |buffer| buffer.snapshot(50));
    let uptime_seconds = (OffsetDateTime::now_utc() - state.started_at)
        .whole_seconds()
        .max(0);
    Json(BundleReport {
        generated_at: now_rfc3339(),
        server: json!({
            "name": "melodee",
            "version": env!("CARGO_PKG_VERSION"),
            "buildProfile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "pid": std::process::id(),
        }),
        uptime_seconds,
        selftest,
        settings,
        streaming,
        recent_events,
    })
    .into_response()
}
