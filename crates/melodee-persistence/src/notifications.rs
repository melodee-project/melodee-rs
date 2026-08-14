//! Phase 9 WP-09 notification template and delivery persistence (ADR-0134
//! §4, PRD-NOTIF-001/002) over `notification_templates` and
//! `notification_deliveries` (migration 0024).
//!
//! Templates are operator-manageable, locale-keyed text with simple
//! `{{placeholder}}` bodies; rendering and delivery live in the server
//! (`apps/melodee-server/src/notifications.rs`). Deleting a **seeded**
//! template (`user.password_reset`, `share.created`, `request.fulfilled`) is
//! allowed — it is a supported operator action that disables the rendered
//! body for that event (deliveries are then recorded as `skipped`) and is
//! never silently reverted by a later migration.
//!
//! Deliveries are an **append-only audit**: one row per attempted or
//! intentionally skipped delivery, never updated, and purged only by bounded
//! retention (`NotificationDeliveryStore::purge_older_than`). Rows never
//! store subjects, bodies, or secret material (PRD-NOTIF-002).

use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Bounded template key length, mirroring the migration CHECK.
pub const MAX_TEMPLATE_KEY_CHARS: usize = 100;
/// Bounded locale tag length (BCP-47-ish), mirroring the migration CHECK.
pub const MAX_LOCALE_CHARS: usize = 20;
/// Bounded subject length, mirroring the migration CHECK.
pub const MAX_SUBJECT_CHARS: usize = 500;
/// Serialized-size bound for each body, mirroring the migration CHECKs.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Bounded recipient length, mirroring the migration CHECK.
pub const MAX_RECIPIENT_CHARS: usize = 320;
/// Bounded delivery error length, mirroring the migration CHECK. Longer
/// transport errors are truncated on a char boundary before insert.
pub const MAX_DELIVERY_ERROR_CHARS: usize = 2000;
/// Upper bound for template list responses; the API layer clamps to this.
pub const MAX_TEMPLATE_LIST: u32 = 500;
/// Upper bound for delivery list responses; the API layer clamps to this.
pub const MAX_DELIVERY_LIST: u32 = 500;

type TemplateRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    OffsetDateTime,
);

type DeliveryRow = (Uuid, String, String, String, Option<String>, OffsetDateTime);

/// One stored notification template row (locale-keyed operator text).
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationTemplate {
    pub template_key: String,
    pub locale: String,
    pub subject: Option<String>,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub version: i32,
    pub updated_at: OffsetDateTime,
}

impl From<TemplateRow> for NotificationTemplate {
    fn from(row: TemplateRow) -> Self {
        let (template_key, locale, subject, body_text, body_html, version, updated_at) = row;
        Self {
            template_key,
            locale,
            subject,
            body_text,
            body_html,
            version,
            updated_at,
        }
    }
}

/// Upsert payload. On conflict the stored row is replaced and its `version`
/// is bumped, so operators can see template churn.
pub struct NotificationTemplateUpsert<'a> {
    pub template_key: &'a str,
    pub locale: &'a str,
    pub subject: Option<&'a str>,
    pub body_text: Option<&'a str>,
    pub body_html: Option<&'a str>,
}

/// Delivery outcome recorded in the audit. `Skipped` covers the honest
/// no-op cases: email delivery disabled by configuration, or no template
/// stored for the requested key/locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    Sent,
    Failed,
    Skipped,
}

impl DeliveryStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// One append-only delivery audit row. Contains no message content or
/// secret material by construction (PRD-NOTIF-002).
#[derive(Clone, Debug, PartialEq)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub template_key: String,
    pub recipient: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: OffsetDateTime,
}

impl From<DeliveryRow> for NotificationDelivery {
    fn from(row: DeliveryRow) -> Self {
        let (id, template_key, recipient, status, error, created_at) = row;
        Self {
            id,
            template_key,
            recipient,
            status,
            error,
            created_at,
        }
    }
}

/// Audit insert payload. `error` is truncated to the migration bound on a
/// char boundary so transport messages always fit.
pub struct NewNotificationDelivery<'a> {
    pub template_key: &'a str,
    pub recipient: &'a str,
    pub status: DeliveryStatus,
    pub error: Option<&'a str>,
}

#[derive(Debug, Error)]
pub enum NotificationStoreError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("notification template not found")]
    TemplateNotFound,
    #[error("invalid notification template: {0}")]
    InvalidTemplate(&'static str),
    #[error("invalid notification delivery: {0}")]
    InvalidDelivery(&'static str),
}

fn validate_template_key(template_key: &str) -> Result<(), NotificationStoreError> {
    if template_key.trim().is_empty() {
        return Err(NotificationStoreError::InvalidTemplate(
            "template_key must be non-blank",
        ));
    }
    if template_key.chars().count() > MAX_TEMPLATE_KEY_CHARS {
        return Err(NotificationStoreError::InvalidTemplate(
            "template_key exceeds 100 characters",
        ));
    }
    Ok(())
}

/// BCP-47-ish shape shared with the migration CHECK: 2-3 ASCII letters,
/// then optional `-`-separated 2-8 character alphanumeric subtags (en,
/// en-US, pt-BR, zh-Hant). Phase 10 localization owns any widening.
pub fn is_valid_locale(locale: &str) -> bool {
    let mut segments = locale.split('-');
    let Some(language) = segments.next() else {
        return false;
    };
    let valid_language = (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic());
    if !valid_language {
        return false;
    }
    locale.chars().count() <= MAX_LOCALE_CHARS
        && segments.all(|segment| {
            (2..=8).contains(&segment.len())
                && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn validate_locale(locale: &str) -> Result<(), NotificationStoreError> {
    if !is_valid_locale(locale) {
        return Err(NotificationStoreError::InvalidTemplate(
            "locale must be a BCP-47-ish tag of at most 20 characters",
        ));
    }
    Ok(())
}

fn validate_optional_subject(subject: Option<&str>) -> Result<(), NotificationStoreError> {
    if let Some(subject) = subject
        && subject.chars().count() > MAX_SUBJECT_CHARS
    {
        return Err(NotificationStoreError::InvalidTemplate(
            "subject exceeds 500 characters",
        ));
    }
    Ok(())
}

fn validate_optional_body(body: Option<&str>) -> Result<(), NotificationStoreError> {
    if let Some(body) = body
        && body.len() > MAX_BODY_BYTES
    {
        return Err(NotificationStoreError::InvalidTemplate(
            "body exceeds 65536 bytes",
        ));
    }
    Ok(())
}

/// Truncates on a char boundary so recorded transport errors always fit the
/// audit column.
fn truncate_error(error: &str) -> String {
    if error.chars().count() <= MAX_DELIVERY_ERROR_CHARS {
        return error.to_string();
    }
    // Reserve one character for the truncation marker so the stored value
    // always fits the CHECK bound.
    let mut truncated: String = error.chars().take(MAX_DELIVERY_ERROR_CHARS - 1).collect();
    truncated.push('…');
    truncated
}

/// Stateless entry point for notification template administration.
#[derive(Clone, Copy, Debug, Default)]
pub struct NotificationTemplateStore;

impl NotificationTemplateStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Bounded, deterministic listing: template key, then locale.
    pub async fn list(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NotificationTemplate>, NotificationStoreError> {
        let rows = sqlx::query_as::<_, TemplateRow>(
            "SELECT template_key,locale,subject,body_text,body_html,version,updated_at \
             FROM notification_templates ORDER BY template_key, locale LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit.min(MAX_TEMPLATE_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(NotificationTemplate::from).collect())
    }

    pub async fn get(
        &self,
        pool: &PgPool,
        template_key: &str,
        locale: &str,
    ) -> Result<NotificationTemplate, NotificationStoreError> {
        let row = sqlx::query_as::<_, TemplateRow>(
            "SELECT template_key,locale,subject,body_text,body_html,version,updated_at \
             FROM notification_templates WHERE template_key=$1 AND locale=$2",
        )
        .bind(template_key)
        .bind(locale)
        .fetch_optional(pool)
        .await?;
        row.map(NotificationTemplate::from)
            .ok_or(NotificationStoreError::TemplateNotFound)
    }

    /// Creates or replaces one (key, locale) template. Replacement bumps
    /// `version` so template churn is visible to operators.
    pub async fn upsert(
        &self,
        pool: &PgPool,
        upsert: &NotificationTemplateUpsert<'_>,
        now: OffsetDateTime,
    ) -> Result<NotificationTemplate, NotificationStoreError> {
        validate_template_key(upsert.template_key)?;
        validate_locale(upsert.locale)?;
        validate_optional_subject(upsert.subject)?;
        validate_optional_body(upsert.body_text)?;
        validate_optional_body(upsert.body_html)?;
        let row = sqlx::query_as::<_, TemplateRow>(
            "INSERT INTO notification_templates \
               (template_key,locale,subject,body_text,body_html,version,updated_at) \
             VALUES ($1,$2,$3,$4,$5,1,$6) \
             ON CONFLICT (template_key, locale) DO UPDATE SET \
               subject = EXCLUDED.subject, body_text = EXCLUDED.body_text, \
               body_html = EXCLUDED.body_html, \
               version = notification_templates.version + 1, updated_at = EXCLUDED.updated_at \
             RETURNING template_key,locale,subject,body_text,body_html,version,updated_at",
        )
        .bind(upsert.template_key)
        .bind(upsert.locale)
        .bind(upsert.subject)
        .bind(upsert.body_text)
        .bind(upsert.body_html)
        .bind(now)
        .fetch_one(pool)
        .await?;
        Ok(NotificationTemplate::from(row))
    }

    /// Deletes one template. Seeded keys are deletable by design (see module
    /// docs); a missing row reports [`NotificationStoreError::TemplateNotFound`].
    pub async fn delete(
        &self,
        pool: &PgPool,
        template_key: &str,
        locale: &str,
    ) -> Result<(), NotificationStoreError> {
        let result =
            sqlx::query("DELETE FROM notification_templates WHERE template_key=$1 AND locale=$2")
                .bind(template_key)
                .bind(locale)
                .execute(pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(NotificationStoreError::TemplateNotFound);
        }
        Ok(())
    }
}

/// Stateless entry point for the append-only delivery audit.
#[derive(Clone, Copy, Debug, Default)]
pub struct NotificationDeliveryStore;

impl NotificationDeliveryStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Appends one delivery audit row. The delivery outcome of a
    /// notification never rolls back the operation that triggered it
    /// (PRD-NOTIF-002), so callers record after the fact.
    pub async fn record_delivery(
        &self,
        pool: &PgPool,
        delivery: &NewNotificationDelivery<'_>,
        now: OffsetDateTime,
    ) -> Result<NotificationDelivery, NotificationStoreError> {
        validate_template_key(delivery.template_key).map_err(|error| match error {
            NotificationStoreError::InvalidTemplate(message) => {
                NotificationStoreError::InvalidDelivery(message)
            }
            other => other,
        })?;
        if delivery.recipient.trim().is_empty()
            || delivery.recipient.chars().count() > MAX_RECIPIENT_CHARS
        {
            return Err(NotificationStoreError::InvalidDelivery(
                "recipient must be non-blank and at most 320 characters",
            ));
        }
        let error = delivery.error.map(truncate_error);
        let row = sqlx::query_as::<_, DeliveryRow>(
            "INSERT INTO notification_deliveries \
               (id,template_key,recipient,status,error,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             RETURNING id,template_key,recipient,status,error,created_at",
        )
        .bind(Uuid::now_v7())
        .bind(delivery.template_key)
        .bind(delivery.recipient)
        .bind(delivery.status.as_str())
        .bind(error)
        .bind(now)
        .fetch_one(pool)
        .await?;
        Ok(NotificationDelivery::from(row))
    }

    /// Bounded listing, newest first.
    pub async fn list_recent(
        &self,
        pool: &PgPool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NotificationDelivery>, NotificationStoreError> {
        let rows = sqlx::query_as::<_, DeliveryRow>(
            "SELECT id,template_key,recipient,status,error,created_at \
             FROM notification_deliveries ORDER BY created_at DESC, id DESC LIMIT $1 OFFSET $2",
        )
        .bind(i64::from(limit.min(MAX_DELIVERY_LIST)))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(NotificationDelivery::from).collect())
    }

    /// Bounded retention: removes audit rows older than `cutoff`, returning
    /// the number removed. The table is append-only and otherwise never
    /// mutated; this is the single sanctioned deletion path (ADR-0134 §4).
    pub async fn purge_older_than(
        &self,
        pool: &PgPool,
        cutoff: OffsetDateTime,
    ) -> Result<u64, NotificationStoreError> {
        let result = sqlx::query("DELETE FROM notification_deliveries WHERE created_at < $1")
            .bind(cutoff)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}
