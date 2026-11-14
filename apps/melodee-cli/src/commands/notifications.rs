//! Notification template administration and delivery test-send
//! (mirrors the admin API semantics; fail-degraded and audited).

use clap::Subcommand;

use super::common::{parse_nonblank, pool};

#[derive(Debug, Subcommand)]
pub(crate) enum NotificationCommands {
    /// Notification template operations.
    Templates {
        #[command(subcommand)]
        command: NotificationTemplateCommands,
    },
    /// Render a template and attempt a real delivery through the configured
    /// transport, printing the honestly recorded audit row (sent / failed /
    /// skipped) — including when email delivery is disabled.
    TestSend {
        /// Template key (e.g. user.password_reset).
        #[arg(long, value_parser = parse_nonblank)]
        template: String,
        /// Recipient email address.
        #[arg(long, value_parser = parse_nonblank)]
        to: String,
        /// Locale tag (defaults to en; missing locales fall back to en).
        #[arg(long)]
        locale: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum NotificationTemplateCommands {
    /// List stored notification templates (bounded), key then locale.
    List {
        /// Maximum rows to print (bounded).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

/// Notification commands go through the same stores and delivery service as
/// the server (PRD-CLI-002: never bypass safety), mirroring the admin API
/// semantics. Email settings resolve through the layered sources
/// (env/config-file over `server_settings` over defaults); a disabled
/// transport records an honest `skipped` delivery instead of failing the
/// command (PRD-NOTIF-002/003).
pub(crate) async fn run(command: NotificationCommands) {
    let pool = pool().await;
    match command {
        NotificationCommands::Templates {
            command: NotificationTemplateCommands::List { limit },
        } => {
            match melodee_persistence::NotificationTemplateStore::new()
                .list(&pool, limit, 0)
                .await
            {
                Ok(templates) => {
                    for template in &templates {
                        println!(
                            "{:<24} {:<8} v{} subject={:?} updated={}",
                            template.template_key,
                            template.locale,
                            template.version,
                            template.subject.as_deref().unwrap_or("-"),
                            template.updated_at,
                        );
                    }
                    println!("-- {} template(s)", templates.len());
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        NotificationCommands::TestSend {
            template,
            to,
            locale,
        } => {
            let settings_resolver = melodee_persistence::SettingsResolver::from_env(&pool)
                .unwrap_or_else(|error| {
                    eprintln!("error: invalid settings configuration: {error}");
                    std::process::exit(2);
                });
            let settings = melodee_application::notifications::EmailSettings::from_resolved(
                &settings_resolver,
            )
            .await
            .unwrap_or_else(|error| {
                eprintln!("error: invalid email configuration: {error}");
                std::process::exit(2);
            });
            let service = melodee_application::notifications::NotificationService::new(
                pool.clone(),
                &settings,
            )
            .unwrap_or_else(|error| {
                eprintln!("error: invalid email configuration: {error}");
                std::process::exit(2);
            });
            match service
                .send_templated(&template, locale.as_deref().unwrap_or("en"), to.trim(), &[])
                .await
            {
                Ok(delivery) => {
                    println!("delivery_id: {}", delivery.id);
                    println!("template:    {}", delivery.template_key);
                    println!("recipient:   {}", delivery.recipient);
                    println!("status:      {}", delivery.status);
                    println!("error:       {}", delivery.error.as_deref().unwrap_or("-"));
                    println!("created_at:  {}", delivery.created_at);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
    }
}
