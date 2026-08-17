//! Phase 9 WP-09 notification rendering and delivery (PRD-NOTIF-001..003,
//! ADR-0134 §4).
//!
//! Templates are stored rows (`notification_templates`, migration 0024)
//! rendered with a deliberately tiny `{{placeholder}}` substitution grammar
//! — no logic, no includes — so user-visible text stays externalized for
//! Phase 10 localization (PRD-NOTIF-003). Delivery uses a single FOSS SMTP
//! transport (lettre, rustls) with no new infrastructure; delivery failures
//! are recorded in the append-only `notification_deliveries` audit and
//! logged with secrets redacted, and are **never** propagated to roll back
//! the operation that triggered the notification (PRD-NOTIF-002).
//!
//! Settings come from [`EmailSettings`], a small value struct. It is built
//! through WP-08's layered settings resolution
//! ([`melodee_persistence::SettingsResolver`]): explicit `MELODEE__EMAIL__*`
//! environment keys (or the config file) win over DB-backed
//! `server_settings` rows, which win over the legacy seeded defaults —
//! matching the legacy `MelodeeConfigurationFactory` overlay direction
//! (PRD-CONFIG-003). No call site changed when the DB layer landed.

use std::{future::Future, pin::Pin, sync::Arc};

use crate::resource_bounds::ResourceGauge;

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
};
use melodee_persistence::{
    DeliveryStatus, NewNotificationDelivery, NotificationDelivery, NotificationDeliveryStore,
    NotificationStoreError, NotificationTemplateStore,
};
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::Instrument as _;

/// Environment keys (the complete email set this module honors), mirroring
/// the legacy `email.*` group in
/// `design/evidence/phase-9/configuration-inventory.md` §14.
pub const ENV_EMAIL_ENABLED: &str = "MELODEE__EMAIL__ENABLED";
pub const ENV_EMAIL_FROM_NAME: &str = "MELODEE__EMAIL__FROM_NAME";
pub const ENV_EMAIL_FROM_EMAIL: &str = "MELODEE__EMAIL__FROM_EMAIL";
pub const ENV_EMAIL_SMTP_HOST: &str = "MELODEE__EMAIL__SMTP_HOST";
pub const ENV_EMAIL_SMTP_PORT: &str = "MELODEE__EMAIL__SMTP_PORT";
pub const ENV_EMAIL_SMTP_USERNAME: &str = "MELODEE__EMAIL__SMTP_USERNAME";
pub const ENV_EMAIL_SMTP_PASSWORD: &str = "MELODEE__EMAIL__SMTP_PASSWORD";
pub const ENV_EMAIL_SMTP_USE_SSL: &str = "MELODEE__EMAIL__SMTP_USE_SSL";
pub const ENV_EMAIL_SMTP_USE_START_TLS: &str = "MELODEE__EMAIL__SMTP_USE_START_TLS";

/// Locale used for server-wired event notifications until per-user locales
/// land with Phase 10 localization. Template lookup falls back to it for any
/// requested locale.
pub const DEFAULT_LOCALE: &str = "en";

/// Bounded retention for the delivery audit; enforced on every recorded
/// delivery so the append-only table stays bounded without a new worker.
const DELIVERY_RETENTION: time::Duration = time::Duration::days(30);

/// Bounded SMTP round-trip so a stuck relay never hangs a delivery task.
const SMTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Boxed future used by the object-safe transport boundary.
pub type NotificationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Email delivery settings (legacy `email.*` parity). The password is never
/// exposed: no accessor returns it, `Debug` redacts it, and it never reaches
/// logs, DTOs, or the delivery audit (PRD-NOTIF-002).
#[derive(Clone)]
pub struct EmailSettings {
    pub enabled: bool,
    pub from_name: String,
    pub from_email: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    smtp_password: String,
    /// Implicit TLS (legacy `email.smtpUseSsl`, conventionally port 465).
    pub smtp_use_ssl: bool,
    /// StartTLS upgrade (legacy `email.smtpUseStartTls`, port 587).
    pub smtp_use_start_tls: bool,
}

impl std::fmt::Debug for EmailSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmailSettings")
            .field("enabled", &self.enabled)
            .field("from_name", &self.from_name)
            .field("from_email", &self.from_email)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field("smtp_password", &"<redacted>")
            .field("smtp_use_ssl", &self.smtp_use_ssl)
            .field("smtp_use_start_tls", &self.smtp_use_start_tls)
            .finish()
    }
}

impl Default for EmailSettings {
    /// Legacy seeded defaults: delivery disabled, StartTLS on port 587.
    fn default() -> Self {
        Self {
            enabled: false,
            from_name: "Melodee".to_string(),
            from_email: String::new(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
            smtp_use_ssl: false,
            smtp_use_start_tls: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum NotificationConfigError {
    #[error("{0} must be 'true' or 'false'")]
    InvalidBoolean(&'static str),
    #[error("{0} must be a valid port (1-65535)")]
    InvalidPort(&'static str),
    #[error("{0} is required when {ENV_EMAIL_ENABLED} is true")]
    MissingRequired(&'static str),
    #[error("{ENV_EMAIL_SMTP_USE_SSL} and {ENV_EMAIL_SMTP_USE_START_TLS} are mutually exclusive")]
    ConflictingTls,
    #[error("email from address is not a valid mailbox")]
    InvalidFromAddress,
    #[error("smtp transport initialization failed")]
    TransportInit,
    #[error("layered settings resolution failed: {0}")]
    Layered(String),
}

impl EmailSettings {
    /// Builds settings from the explicit environment keys only. Retained as
    /// the env-only fast path; production composition uses
    /// [`EmailSettings::from_resolved`]. Validation here stays the single
    /// gate either way.
    pub fn from_env() -> Result<Self, NotificationConfigError> {
        let defaults = Self::default();
        let settings = Self {
            enabled: parse_bool_env(ENV_EMAIL_ENABLED)?.unwrap_or(defaults.enabled),
            from_name: std::env::var(ENV_EMAIL_FROM_NAME).unwrap_or(defaults.from_name),
            from_email: std::env::var(ENV_EMAIL_FROM_EMAIL).unwrap_or_default(),
            smtp_host: std::env::var(ENV_EMAIL_SMTP_HOST).unwrap_or_default(),
            smtp_port: parse_port_env(ENV_EMAIL_SMTP_PORT)?.unwrap_or(defaults.smtp_port),
            smtp_username: std::env::var(ENV_EMAIL_SMTP_USERNAME).unwrap_or_default(),
            smtp_password: std::env::var(ENV_EMAIL_SMTP_PASSWORD).unwrap_or_default(),
            smtp_use_ssl: parse_bool_env(ENV_EMAIL_SMTP_USE_SSL)?.unwrap_or(defaults.smtp_use_ssl),
            smtp_use_start_tls: parse_bool_env(ENV_EMAIL_SMTP_USE_START_TLS)?
                .unwrap_or(defaults.smtp_use_start_tls),
        };
        settings.validate()?;
        Ok(settings)
    }

    /// Builds settings through WP-08 layered resolution (PRD-CONFIG-003):
    /// `MELODEE__EMAIL__*` environment keys, then the optional config file,
    /// then `server_settings` DB rows, then the legacy seeded defaults. The
    /// resolver produces typed values per the inventory classification, so
    /// extraction here never re-parses text. `validate` remains the single
    /// semantic gate.
    pub async fn from_resolved(
        resolver: &melodee_persistence::SettingsResolver<'_>,
    ) -> Result<Self, NotificationConfigError> {
        async fn value(
            resolver: &melodee_persistence::SettingsResolver<'_>,
            key: &str,
        ) -> Result<Option<serde_json::Value>, NotificationConfigError> {
            resolver
                .resolve(key)
                .await
                .map(|resolved| resolved.value)
                .map_err(|error| NotificationConfigError::Layered(error.to_string()))
        }
        fn as_bool(value: Option<serde_json::Value>) -> Option<bool> {
            value.and_then(|value| value.as_bool())
        }
        fn as_string(value: Option<serde_json::Value>) -> Option<String> {
            value.and_then(|value| value.as_str().map(str::to_string))
        }
        let defaults = Self::default();
        let settings = Self {
            enabled: as_bool(value(resolver, "email.enabled").await?).unwrap_or(defaults.enabled),
            from_name: as_string(value(resolver, "email.fromName").await?)
                .unwrap_or(defaults.from_name),
            from_email: as_string(value(resolver, "email.fromEmail").await?).unwrap_or_default(),
            smtp_host: as_string(value(resolver, "email.smtpHost").await?).unwrap_or_default(),
            smtp_port: match value(resolver, "email.smtpPort").await? {
                Some(port) => port
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok())
                    .filter(|port| *port > 0)
                    .ok_or(NotificationConfigError::InvalidPort(ENV_EMAIL_SMTP_PORT))?,
                None => defaults.smtp_port,
            },
            smtp_username: as_string(value(resolver, "email.smtpUsername").await?)
                .unwrap_or_default(),
            smtp_password: as_string(value(resolver, "email.smtpPassword").await?)
                .unwrap_or_default(),
            smtp_use_ssl: as_bool(value(resolver, "email.smtpUseSsl").await?)
                .unwrap_or(defaults.smtp_use_ssl),
            smtp_use_start_tls: as_bool(value(resolver, "email.smtpUseStartTls").await?)
                .unwrap_or(defaults.smtp_use_start_tls),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), NotificationConfigError> {
        if self.smtp_use_ssl && self.smtp_use_start_tls {
            return Err(NotificationConfigError::ConflictingTls);
        }
        if self.enabled {
            if self.from_email.trim().is_empty() {
                return Err(NotificationConfigError::MissingRequired(
                    ENV_EMAIL_FROM_EMAIL,
                ));
            }
            if self.sender_mailbox().is_none() {
                return Err(NotificationConfigError::InvalidFromAddress);
            }
            if self.smtp_host.trim().is_empty() {
                return Err(NotificationConfigError::MissingRequired(
                    ENV_EMAIL_SMTP_HOST,
                ));
            }
        }
        Ok(())
    }

    /// The RFC 5322 From mailbox. Returns `None` when the configured address
    /// is not a valid mailbox; `validate` rejects that state when enabled.
    fn sender_mailbox(&self) -> Option<Mailbox> {
        // lettre's mailbox parser is deliberately lenient; enforce the one
        // structural property Melodee requires (non-empty user@domain)
        // before delegating.
        let email = self.from_email.trim();
        let (user, domain) = email.rsplit_once('@')?;
        if user.is_empty() || domain.is_empty() {
            return None;
        }
        let name = (!self.from_name.trim().is_empty()).then(|| self.from_name.trim().to_string());
        format!("{} <{email}>", name.unwrap_or_default())
            .trim()
            .parse::<Mailbox>()
            .ok()
    }
}

fn parse_bool_env(key: &'static str) -> Result<Option<bool>, NotificationConfigError> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(NotificationConfigError::InvalidBoolean(key)),
        })
        .transpose()
}

fn parse_port_env(key: &'static str) -> Result<Option<u16>, NotificationConfigError> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .trim()
                .parse::<u16>()
                .map_err(|_| NotificationConfigError::InvalidPort(key))
                .and_then(|port| {
                    if port == 0 {
                        Err(NotificationConfigError::InvalidPort(key))
                    } else {
                        Ok(port)
                    }
                })
        })
        .transpose()
}

/// Escapes the five HTML-significant characters for substituted values in
/// `body_html` rendering. Template text itself is trusted operator content;
/// only substituted values are escaped.
fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Renders `{{placeholder}}` substitution. Unknown or malformed tokens are
/// left verbatim (never an error, never silently blanked), so a template
/// that references a placeholder a wiring site does not supply is visibly
/// wrong to the operator rather than silently empty.
#[must_use]
pub fn render(template: &str, placeholders: &[(String, String)], escape_values: bool) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        output.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        match after_open.find("}}") {
            Some(close) => {
                let name = after_open[..close].trim();
                match placeholders.iter().find(|(key, _)| key == name) {
                    Some((_, value)) if escape_values => output.push_str(&escape_html(value)),
                    Some((_, value)) => output.push_str(value),
                    None => output.push_str(&rest[open..open + 2 + close + 2]),
                }
                rest = &after_open[close + 2..];
            }
            // Unclosed token: emit the remainder (including `{{`) verbatim.
            None => {
                output.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    output
}

/// A fully rendered outbound message. Contains no secret material beyond
/// what the triggering event placed in its placeholders (e.g. a reset URL);
/// it is never logged or persisted — only delivery metadata is audited.
#[derive(Debug)]
pub struct OutgoingMessage {
    pub to: String,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("email delivery is disabled by configuration")]
    Disabled,
    #[error("recipient address is not a valid mailbox")]
    InvalidRecipient,
    #[error("message build failed")]
    MessageBuild,
    #[error("smtp delivery failed: {0}")]
    Smtp(String),
}

/// Delivery transport boundary. `SmtpTransport` is the production
/// implementation; `NoopTransport` is installed whenever delivery is
/// disabled and makes the skip an explicit, auditable outcome.
pub trait NotificationTransport: Send + Sync {
    fn send<'a>(
        &'a self,
        message: &'a OutgoingMessage,
    ) -> NotificationFuture<'a, Result<(), TransportError>>;
}

/// Lettres SMTP transport honoring the legacy `email.*` semantics: implicit
/// TLS (`useSsl`, port 465), StartTLS (`useStartTls`, port 587), or — only
/// when both flags are off — plaintext relay (conventionally port 25).
pub struct SmtpTransport {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl std::fmt::Debug for SmtpTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never surface credentials; lettre's own Debug would print the
        // transport builder including credentials.
        formatter
            .debug_struct("SmtpTransport")
            .finish_non_exhaustive()
    }
}

impl SmtpTransport {
    pub fn new(settings: &EmailSettings) -> Result<Self, NotificationConfigError> {
        let from = settings
            .sender_mailbox()
            .ok_or(NotificationConfigError::InvalidFromAddress)?;
        let builder = if settings.smtp_use_ssl {
            AsyncSmtpTransport::<Tokio1Executor>::relay(settings.smtp_host.trim())
                .map_err(|_| NotificationConfigError::TransportInit)?
        } else if settings.smtp_use_start_tls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(settings.smtp_host.trim())
                .map_err(|_| NotificationConfigError::TransportInit)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(settings.smtp_host.trim())
        };
        let builder = builder.port(settings.smtp_port).timeout(Some(SMTP_TIMEOUT));
        let builder = if settings.smtp_username.trim().is_empty() {
            builder
        } else {
            builder.credentials(Credentials::new(
                settings.smtp_username.trim().to_string(),
                settings.smtp_password.clone(),
            ))
        };
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

impl NotificationTransport for SmtpTransport {
    fn send<'a>(
        &'a self,
        message: &'a OutgoingMessage,
    ) -> NotificationFuture<'a, Result<(), TransportError>> {
        Box::pin(async move {
            let recipient = message
                .to
                .trim()
                .parse::<Mailbox>()
                .map_err(|_| TransportError::InvalidRecipient)?;
            let builder = Message::builder()
                .from(self.from.clone())
                .to(recipient)
                .subject(message.subject.clone());
            let built = match (&message.body_text, &message.body_html) {
                (Some(text), Some(html)) => builder.multipart(
                    MultiPart::alternative()
                        .singlepart(SinglePart::plain(text.clone()))
                        .singlepart(SinglePart::html(html.clone())),
                ),
                (Some(text), None) => builder.body(text.clone()),
                (None, Some(html)) => builder
                    .header(lettre::message::header::ContentType::TEXT_HTML)
                    .body(html.clone()),
                (None, None) => builder.body(String::new()),
            }
            .map_err(|_| TransportError::MessageBuild)?;
            self.transport
                .send(built)
                .await
                .map_err(|error| TransportError::Smtp(error.to_string()))?;
            Ok(())
        })
    }
}

/// Installed when delivery is disabled: every send is an explicit,
/// auditable skip rather than a silent drop.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopTransport;

impl NotificationTransport for NoopTransport {
    fn send<'a>(
        &'a self,
        _message: &'a OutgoingMessage,
    ) -> NotificationFuture<'a, Result<(), TransportError>> {
        Box::pin(async { Err(TransportError::Disabled) })
    }
}

/// Renders stored templates and delivers them through the configured
/// transport, always recording the outcome in the delivery audit.
pub struct NotificationService {
    pool: PgPool,
    transport: Arc<dyn NotificationTransport>,
    /// Process bound on concurrent SMTP round-trips (R2.4, PRD-INGEST-009).
    /// Dispatches queue on this semaphore instead of spawning an unbounded
    /// number of outbound deliveries; the durable audit row is written by
    /// `send_templated` after the send attempt, so a queued dispatch is never
    /// lost work — the send still runs once admitted.
    permits: Arc<tokio::sync::Semaphore>,
    gauge: Option<Arc<crate::resource_bounds::ResourceGauge>>,
}

/// Fixed dispatch capacity for outbound email (R2.4). Four concurrent SMTP
/// round-trips (each bounded by `SMTP_TIMEOUT`) cap relay load while keeping
/// password-reset latency negligible at any realistic dispatch rate.
const NOTIFICATION_DISPATCH_CONCURRENCY: usize = 4;

impl std::fmt::Debug for NotificationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NotificationService")
            .finish_non_exhaustive()
    }
}

impl NotificationService {
    /// Builds the service from validated settings: SMTP when enabled, the
    /// auditable no-op otherwise.
    pub fn new(pool: PgPool, settings: &EmailSettings) -> Result<Self, NotificationConfigError> {
        settings.validate()?;
        if settings.enabled {
            Ok(Self {
                pool,
                transport: Arc::new(SmtpTransport::new(settings)?),
                permits: Arc::new(tokio::sync::Semaphore::new(
                    NOTIFICATION_DISPATCH_CONCURRENCY,
                )),
                gauge: crate::resource_bounds::ResourceBounds::global().map(|registry| {
                    registry.register("notifications.dispatch", NOTIFICATION_DISPATCH_CONCURRENCY)
                }),
            })
        } else {
            Ok(Self::noop(pool))
        }
    }

    /// Service with delivery disabled; every send is recorded as `skipped`.
    #[must_use]
    pub fn noop(pool: PgPool) -> Self {
        Self {
            pool,
            transport: Arc::new(NoopTransport),
            permits: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_DISPATCH_CONCURRENCY,
            )),
            gauge: crate::resource_bounds::ResourceBounds::global().map(|registry| {
                registry.register("notifications.dispatch", NOTIFICATION_DISPATCH_CONCURRENCY)
            }),
        }
    }

    /// Renders `template_key` for `locale` (falling back to `en`) and
    /// attempts delivery, recording the outcome. A missing template is an
    /// honest `skipped` row, not an error. Only persistence failures are
    /// returned as errors; transport failures are data in the audit row.
    pub async fn send_templated(
        &self,
        template_key: &str,
        locale: &str,
        recipient: &str,
        placeholders: &[(String, String)],
    ) -> Result<NotificationDelivery, NotificationStoreError> {
        let templates = NotificationTemplateStore::new();
        let template = match templates.get(&self.pool, template_key, locale).await {
            Ok(template) => Some(template),
            Err(NotificationStoreError::TemplateNotFound) if locale != DEFAULT_LOCALE => {
                match templates
                    .get(&self.pool, template_key, DEFAULT_LOCALE)
                    .await
                {
                    Ok(template) => Some(template),
                    Err(NotificationStoreError::TemplateNotFound) => None,
                    Err(error) => return Err(error),
                }
            }
            Err(NotificationStoreError::TemplateNotFound) => None,
            Err(error) => return Err(error),
        };
        let now = OffsetDateTime::now_utc();
        let (status, error) = match template {
            None => (
                DeliveryStatus::Skipped,
                Some("template not found".to_string()),
            ),
            Some(template) => {
                // Subjects are single-line headers; CR/LF from a template
                // would break the wire format, so they are flattened.
                let subject = render(
                    template.subject.as_deref().unwrap_or_default(),
                    placeholders,
                    false,
                )
                .replace(['\r', '\n'], " ");
                let message = OutgoingMessage {
                    to: recipient.to_string(),
                    subject,
                    body_text: template
                        .body_text
                        .as_deref()
                        .map(|body| render(body, placeholders, false)),
                    body_html: template
                        .body_html
                        .as_deref()
                        .map(|body| render(body, placeholders, true)),
                };
                match self.transport.send(&message).await {
                    Ok(()) => (DeliveryStatus::Sent, None),
                    Err(TransportError::Disabled) => (
                        DeliveryStatus::Skipped,
                        Some("email delivery is disabled by configuration".to_string()),
                    ),
                    Err(error) => (DeliveryStatus::Failed, Some(error.to_string())),
                }
            }
        };
        let delivery = NotificationDeliveryStore::new()
            .record_delivery(
                &self.pool,
                &NewNotificationDelivery {
                    template_key,
                    recipient,
                    status,
                    error: error.as_deref(),
                },
                now,
            )
            .await?;
        // Bounded retention on every record keeps the append-only audit
        // bounded without a dedicated worker. A purge failure never fails
        // the delivery being recorded.
        if let Err(error) = NotificationDeliveryStore::new()
            .purge_older_than(&self.pool, now - DELIVERY_RETENTION)
            .await
        {
            tracing::warn!(%error, "notification delivery retention purge failed");
        }
        Ok(delivery)
    }

    /// Fire-and-record dispatch for event wiring: the send runs off the
    /// request path and its outcome (including persistence failure) is
    /// logged, never propagated (PRD-NOTIF-002).
    pub fn dispatch(
        self: &Arc<Self>,
        template_key: &'static str,
        recipient: String,
        placeholders: Vec<(String, String)>,
    ) {
        let service = Arc::clone(self);
        // R2.5: capture the in-flight request's correlation id before the
        // spawn crosses the task boundary; never log the recipient's address
        // or template content beyond the stable key.
        let correlation_id = crate::correlation::current();
        let span = tracing::info_span!(
            "notification_dispatch",
            template_key,
            correlation_id = tracing::field::Empty,
        );
        if let Some(id) = correlation_id {
            span.record("correlation_id", tracing::field::display(id));
        }
        tokio::spawn(
            async move {
                // Queue on the process dispatch bound before touching the relay:
                // cancellation of this task while waiting simply never admits.
                if let Some(gauge) = &service.gauge {
                    gauge.begin_wait();
                }
                let admission = service.permits.clone().acquire_owned().await;
                let _admission = match admission {
                    Ok(permit) => Some((permit, service.gauge.as_ref().map(ResourceGauge::admit))),
                    Err(error) => {
                        if let Some(gauge) = &service.gauge {
                            gauge.wait_failed();
                        }
                        let _ = error;
                        None
                    }
                };
                match service
                    .send_templated(template_key, DEFAULT_LOCALE, &recipient, &placeholders)
                    .await
                {
                    Ok(delivery) => match delivery.status.as_str() {
                        // A skip with delivery disabled is the expected steady
                        // state on unconfigured servers, not an alarm.
                        status if status == DeliveryStatus::Skipped.as_str() => {
                            tracing::info!(template_key, "notification delivery skipped");
                        }
                        status if status == DeliveryStatus::Failed.as_str() => {
                            tracing::warn!(template_key, "notification delivery failed");
                        }
                        _ => {}
                    },
                    Err(error) => {
                        tracing::warn!(%error, template_key, "notification delivery audit failed");
                    }
                }
            }
            .instrument(span),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn placeholders(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn render_substitutes_known_placeholders() {
        let values = placeholders(&[("resetUrl", "https://m.example/reset?token=abc")]);
        assert_eq!(
            render("Reset here: {{resetUrl}}", &values, false),
            "Reset here: https://m.example/reset?token=abc"
        );
        assert_eq!(
            render(
                "{{a}}-{{ b }}-{{a}}",
                &placeholders(&[("a", "1"), ("b", "2")]),
                false
            ),
            "1-2-1"
        );
    }

    #[test]
    fn render_escapes_substituted_values_only_in_html_mode() {
        let values = placeholders(&[("description", "<b>x</b> & \"y\" 'z'")]);
        assert_eq!(
            render("{{description}}", &values, true),
            "&lt;b&gt;x&lt;/b&gt; &amp; &quot;y&quot; &#39;z&#39;"
        );
        // Text bodies are verbatim.
        assert_eq!(
            render("{{description}}", &values, false),
            "<b>x</b> & \"y\" 'z'"
        );
        // Template text itself is trusted: only values are escaped.
        assert_eq!(
            render(
                "<p>{{description}}</p>",
                &placeholders(&[("description", "&")]),
                true
            ),
            "<p>&amp;</p>"
        );
    }

    #[test]
    fn render_leaves_unknown_and_malformed_tokens_verbatim() {
        let values = placeholders(&[("known", "v")]);
        assert_eq!(
            render("{{known}} {{unknown}}", &values, false),
            "v {{unknown}}"
        );
        assert_eq!(render("{{unclosed", &values, false), "{{unclosed");
        assert_eq!(render("{{}}", &values, false), "{{}}");
    }

    #[test]
    fn settings_validate_enforced_requirements() {
        let mut settings = EmailSettings::default();
        assert!(settings.validate().is_ok(), "disabled needs nothing");

        settings.enabled = true;
        assert!(matches!(
            settings.validate(),
            Err(NotificationConfigError::MissingRequired(
                ENV_EMAIL_FROM_EMAIL
            ))
        ));
        settings.from_email = "not-a-mailbox".to_string();
        assert!(matches!(
            settings.validate(),
            Err(NotificationConfigError::InvalidFromAddress)
        ));
        settings.from_email = "melodee@example.test".to_string();
        assert!(matches!(
            settings.validate(),
            Err(NotificationConfigError::MissingRequired(
                ENV_EMAIL_SMTP_HOST
            ))
        ));
        settings.smtp_host = "smtp.example.test".to_string();
        assert!(settings.validate().is_ok());

        settings.smtp_use_ssl = true;
        assert!(matches!(
            settings.validate(),
            Err(NotificationConfigError::ConflictingTls)
        ));
        settings.smtp_use_start_tls = false;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn settings_debug_redacts_the_password() {
        let settings = EmailSettings {
            smtp_password: "super-secret-password".to_string(),
            ..EmailSettings::default()
        };
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("super-secret-password"));
        assert!(rendered.contains("<redacted>"));
    }

    /// R2.5 redaction audit: a failing delivery must never write the SMTP
    /// password into the log stream, regardless of the transport error text.
    #[tokio::test]
    async fn delivery_failure_logs_never_contain_the_smtp_password() {
        use std::sync::Mutex;

        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("capture lock")
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        struct MakeCapture(Capture);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeCapture {
            type Writer = Capture;
            fn make_writer(&self) -> Self::Writer {
                self.0.clone()
            }
        }

        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(MakeCapture(capture.clone()))
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        const SECRET: &str = "r2-5-redaction-sentinel-password";
        let settings = EmailSettings {
            smtp_host: "127.0.0.1".to_string(),
            smtp_port: 1,
            smtp_username: "operator".to_string(),
            smtp_password: SECRET.to_string(),
            from_email: "melodee@example.test".to_string(),
            ..EmailSettings::default()
        };
        let transport = SmtpTransport::new(&settings).expect("transport builds");
        let message = OutgoingMessage {
            to: "user@example.test".to_string(),
            subject: "subject".to_string(),
            body_text: Some("body".to_string()),
            body_html: None,
        };
        let outcome = NotificationTransport::send(&transport, &message).await;
        assert!(outcome.is_err(), "port 1 must refuse the connection");

        let logged =
            String::from_utf8(capture.0.lock().expect("capture lock").clone()).expect("utf8 logs");
        assert!(
            !logged.contains(SECRET),
            "log stream leaked the SMTP password: {logged}"
        );
    }
}
