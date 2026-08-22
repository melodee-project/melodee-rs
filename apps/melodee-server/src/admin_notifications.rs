//! Admin API: notification template administration, delivery
//! audit, and operator test-send (PRD-NOTIF-001..003, ADR-0134 §4).
//!
//! These routes extend the Rust-native operator API (`/api/admin/v1`,
//! SPEC 19.4) over `notification_templates`/`notification_deliveries`
//! (migration 0024). Authorization: the shared native-JWT administrator gate
//! ([`crate::admin_access::AdminPrincipal`]). Every mutation is audit-logged
//! via `tracing` with the acting administrator's public id.
//!
//! Deleting a **seeded** template (`user.password_reset`, `share.created`,
//! `request.fulfilled`) is allowed: it disables the rendered body for that
//! event — subsequent deliveries are recorded as `skipped` — and no
//! migration silently restores it. No route returns message bodies of past
//! deliveries (the audit stores none) or any secret material
//! (PRD-NOTIF-002).

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::{
    MAX_RECIPIENT_CHARS, NotificationDelivery, NotificationDeliveryStore, NotificationStoreError,
    NotificationTemplate, NotificationTemplateStore, NotificationTemplateUpsert,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};
use crate::notifications::DEFAULT_LOCALE;

/// Maximum rows returned by listings; clamped even when a client requests
/// more.
const MAX_LIST_LIMIT: u32 = 500;

/// Stable admin API error envelope (matches `admin.rs`/`admin_access.rs`
/// conventions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminNotificationErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(AdminNotificationErrorBody {
            code,
            message: code.to_string(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_error(error: NotificationStoreError) -> Response {
    let (status, code) = match &error {
        NotificationStoreError::TemplateNotFound => (StatusCode::NOT_FOUND, "not_found"),
        NotificationStoreError::InvalidTemplate(_) => (StatusCode::BAD_REQUEST, "invalid_template"),
        NotificationStoreError::InvalidDelivery(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        NotificationStoreError::Database(_) => {
            tracing::error!(%error, "admin notification store failure");
            (StatusCode::INTERNAL_SERVER_ERROR, "store_error")
        }
    };
    (
        status,
        Json(AdminNotificationErrorBody {
            code,
            // Validation messages are static, operator-safe constraint
            // labels — never echoed values.
            message: error.to_string(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

/// One notification template as exposed to operators. `subject`/`bodyText`/
/// `bodyHtml` are the full operator-manageable text; `null` means unset.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationTemplateDto {
    pub template_key: String,
    pub locale: String,
    pub subject: Option<String>,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub version: i32,
    pub updated_at: String,
}

fn template_dto(template: &NotificationTemplate) -> Result<NotificationTemplateDto, ()> {
    Ok(NotificationTemplateDto {
        template_key: template.template_key.clone(),
        locale: template.locale.clone(),
        subject: template.subject.clone(),
        body_text: template.body_text.clone(),
        body_html: template.body_html.clone(),
        version: template.version,
        updated_at: template.updated_at.format(&Rfc3339).map_err(|_| ())?,
    })
}

/// One delivery audit row. Contains no message content — the audit never
/// stores subjects or bodies (PRD-NOTIF-002).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDeliveryDto {
    pub id: Uuid,
    pub template_key: String,
    pub recipient: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
}

fn delivery_dto(delivery: &NotificationDelivery) -> Result<NotificationDeliveryDto, ()> {
    Ok(NotificationDeliveryDto {
        id: delivery.id,
        template_key: delivery.template_key.clone(),
        recipient: delivery.recipient.clone(),
        status: delivery.status.clone(),
        error: delivery.error.clone(),
        created_at: delivery.created_at.format(&Rfc3339).map_err(|_| ())?,
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

/// Full-replacement upsert body: `null` clears the field on the stored row.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertTemplateRequest {
    pub subject: Option<String>,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSendRequest {
    pub template_key: String,
    /// Defaults to `en` (the server event locale until per-user
    /// localization).
    pub locale: Option<String>,
    pub recipient: String,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/notifications/templates",
            get(list_templates),
        )
        .route(
            "/api/admin/v1/notifications/templates/{templateKey}/{locale}",
            get(get_template)
                .put(upsert_template)
                .delete(delete_template),
        )
        .route(
            "/api/admin/v1/notifications/deliveries",
            get(list_deliveries),
        )
        .route(
            "/api/admin/v1/notifications/test-send",
            axum::routing::post(test_send),
        )
 // Template bodies are individually bounded at 64 KiB by the store;
 // the envelope admits subject + both bodies plus JSON overhead.
        .route_layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
}

async fn list_templates(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match NotificationTemplateStore::new()
        .list(&state.pool, limit, params.offset)
        .await
    {
        Ok(templates) => match templates
            .iter()
            .map(template_dto)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(dtos) => Json(dtos).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn get_template(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path((template_key, locale)): Path<(String, String)>,
) -> Response {
    match NotificationTemplateStore::new()
        .get(&state.pool, &template_key, &locale)
        .await
    {
        Ok(template) => match template_dto(&template) {
            Ok(dto) => Json(dto).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

async fn upsert_template(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((template_key, locale)): Path<(String, String)>,
    Json(request): Json<UpsertTemplateRequest>,
) -> Response {
    let upsert = NotificationTemplateUpsert {
        template_key: &template_key,
        locale: &locale,
        subject: request.subject.as_deref(),
        body_text: request.body_text.as_deref(),
        body_html: request.body_html.as_deref(),
    };
    match NotificationTemplateStore::new()
        .upsert(&state.pool, &upsert, OffsetDateTime::now_utc())
        .await
    {
        Ok(template) => {
            tracing::info!(
                actor = %admin.public_api_id,
                template_key = %template.template_key,
                locale = %template.locale,
                version = template.version,
                "admin upserted notification template"
            );
            match template_dto(&template) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}

async fn delete_template(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((template_key, locale)): Path<(String, String)>,
) -> Response {
    match NotificationTemplateStore::new()
        .delete(&state.pool, &template_key, &locale)
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                template_key,
                locale,
                "admin deleted notification template"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => store_error(error),
    }
}

async fn list_deliveries(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<ListParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    match NotificationDeliveryStore::new()
        .list_recent(&state.pool, limit, params.offset)
        .await
    {
        Ok(deliveries) => match deliveries
            .iter()
            .map(delivery_dto)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(dtos) => Json(dtos).into_response(),
            Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
        },
        Err(error) => store_error(error),
    }
}

/// Operator verification hook (PRD-NOTIF-003): renders the template and
/// attempts a real delivery through the configured transport, returning the
/// honestly recorded audit row — including `skipped` when delivery is
/// disabled and `failed` with a bounded, secret-free error when the relay
/// refuses.
async fn test_send(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<TestSendRequest>,
) -> Response {
    let recipient = request.recipient.trim();
    if recipient.is_empty()
        || recipient.chars().count() > MAX_RECIPIENT_CHARS
        || !recipient.contains('@')
    {
        return admin_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let locale = request.locale.as_deref().unwrap_or(DEFAULT_LOCALE);
    // Operator verification placeholders only: template placeholders that a
    // wiring site would supply (resetUrl, shareUrl,...) render verbatim.
    let placeholders: Vec<(String, String)> = Vec::new();
    match state
        .notifications
        .send_templated(&request.template_key, locale, recipient, &placeholders)
        .await
    {
        Ok(delivery) => {
            tracing::info!(
                actor = %admin.public_api_id,
                template_key = %delivery.template_key,
                status = %delivery.status,
                "admin notification test-send recorded"
            );
            match delivery_dto(&delivery) {
                Ok(dto) => Json(dto).into_response(),
                Err(()) => admin_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error"),
            }
        }
        Err(error) => store_error(error),
    }
}
