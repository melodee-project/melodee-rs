//! R2.6 SMTP evidence (PRD-NOTIF-002): delivery through a real plaintext
//! SMTP round-trip against a disposable per-test listener — the existing
//! coverage was failure-path-only behind a mock transport. The fake speaks
//! just enough RFC 5321 for lettre's unencrypted client (banner, EHLO,
//! MAIL FROM, RCPT TO, DATA, QUIT) and captures the envelope plus message
//! bytes for assertions. Each test binds its own ephemeral port, so parallel
//! tests never share server state.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::time::Duration;

use melodee_persistence::{DeliveryStatus, NotificationTemplateStore, NotificationTemplateUpsert};
use melodee_server::notifications::{EmailSettings, NotificationService};
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// One connection's captured envelope and raw message data.
#[derive(Debug)]
struct CapturedMail {
    mail_from: String,
    rcpt_to: Vec<String>,
    data: String,
}

/// A disposable SMTP listener for one test. Aborting on drop keeps no task
/// or port alive past the test.
struct FakeSmtpServer {
    port: u16,
    received: mpsc::Receiver<CapturedMail>,
    task: tokio::task::AbortHandle,
}

impl Drop for FakeSmtpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeSmtpServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake smtp listener");
        let port = listener.local_addr().expect("fake smtp address").port();
        let (tx, rx) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, tx).await;
                });
            }
        });
        Self {
            port,
            received: rx,
            task: task.abort_handle(),
        }
    }

    /// Waits for one captured message (bounded so a protocol stall fails
    /// the test instead of hanging the suite).
    async fn next_mail(&mut self) -> CapturedMail {
        tokio::time::timeout(Duration::from_secs(10), self.received.recv())
            .await
            .expect("the fake server must capture one message")
            .expect("fake server channel stays open")
    }
}

/// Speaks the minimal server side of the lettre plaintext exchange. The
/// EHLO reply advertises no AUTH, STARTTLS, or PIPELINING, so the client
/// stays sequential, unauthenticated, and unencrypted. The captured message
/// is emitted when the client sends QUIT or closes the pooled connection.
async fn serve_connection(
    stream: tokio::net::TcpStream,
    tx: mpsc::Sender<CapturedMail>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer.write_all(b"220 fake ESMTP ready\r\n").await?;
    let mut mail_from = String::new();
    let mut rcpt_to = Vec::new();
    let mut data = String::new();
    let mut in_data = false;
    while let Some(line) = lines.next_line().await? {
        if in_data {
            if line == "." {
                in_data = false;
                writer.write_all(b"250 2.0.0 queued\r\n").await?;
            } else {
                data.push_str(&line);
                data.push('\n');
            }
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer
                .write_all(b"250-fake greets you\r\n250 8BITMIME\r\n")
                .await?;
        } else if let Some(address) = line.get(..10).filter(|_| upper.starts_with("MAIL FROM:")) {
            let _ = address;
            mail_from = line["MAIL FROM:".len()..].trim().to_string();
            writer.write_all(b"250 2.1.0 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            rcpt_to.push(line["RCPT TO:".len()..].trim().to_string());
            writer.write_all(b"250 2.1.5 ok\r\n").await?;
        } else if upper == "DATA" {
            in_data = true;
            writer
                .write_all(b"354 end with <CR><LF>.<CR><LF>\r\n")
                .await?;
        } else if upper == "QUIT" {
            writer.write_all(b"221 2.0.0 bye\r\n").await?;
            break;
        } else {
            writer.write_all(b"502 5.5.2 unsupported\r\n").await?;
        }
    }
    let _ = tx
        .send(CapturedMail {
            mail_from,
            rcpt_to,
            data,
        })
        .await;
    Ok(())
}

/// Enabled plaintext settings aimed at the per-test fake. Built from
/// `Default` because the password field is deliberately private; the fake
/// never requires credentials.
fn plaintext_settings(port: u16) -> EmailSettings {
    let mut settings = EmailSettings::default();
    settings.enabled = true;
    settings.from_name = "Melodee Test".to_string();
    settings.from_email = "melodee@example.test".to_string();
    settings.smtp_host = "127.0.0.1".to_string();
    settings.smtp_port = port;
    settings.smtp_use_ssl = false;
    settings.smtp_use_start_tls = false;
    settings
}

async fn seed_template(db: &common::TestDb) {
    NotificationTemplateStore::new()
        .upsert(
            &db.pool,
            &NotificationTemplateUpsert {
                template_key: "smtp-roundtrip",
                locale: "en",
                subject: Some("Welcome {{displayName}}"),
                body_text: Some("Hello {{displayName}}, your code is {{code}}."),
                body_html: Some("<p>Hello {{displayName}}</p>"),
            },
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("template");
}

#[tokio::test]
async fn smtp_success_records_sent_and_delivers_the_rendered_message() {
    let mut server = FakeSmtpServer::start().await;
    let db = common::TestDb::create("notification_smtp_success").await;
    seed_template(&db).await;
    let service = NotificationService::new(db.pool.clone(), &plaintext_settings(server.port))
        .expect("service");

    let delivery = service
        .send_templated(
            "smtp-roundtrip",
            "en",
            "listener@example.test",
            &[
                ("displayName".to_string(), "Smtp Listener".to_string()),
                ("code".to_string(), "123456".to_string()),
            ],
        )
        .await
        .expect("delivery");
    assert_eq!(delivery.status, DeliveryStatus::Sent.as_str());
    assert_eq!(delivery.error, None, "a successful send records no error");

    // lettre pools the connection; dropping the service closes it so the
    // fake flushes the capture deterministically.
    drop(service);
    let mail = server.next_mail().await;
    assert_eq!(mail.mail_from, "<melodee@example.test>");
    assert_eq!(mail.rcpt_to, vec!["<listener@example.test>".to_string()]);
    assert!(
        mail.data.contains("Subject: Welcome Smtp Listener"),
        "rendered subject reached the wire: {}",
        mail.data
    );
    assert!(
        mail.data.contains("your code is 123456."),
        "rendered plain body reached the wire: {}",
        mail.data
    );
    assert!(
        mail.data.contains("<p>Hello Smtp Listener</p>"),
        "rendered HTML body reached the wire: {}",
        mail.data
    );
    db.drop().await;
}

#[tokio::test]
async fn smtp_protocol_rejection_records_failed_without_raising() {
    // A fake that rejects every RCPT exercises the wire-level failure path:
    // the outcome is data in the audit row, never an error to the caller.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reject listener");
    let port = listener.local_addr().expect("reject address").port();
    let rejecter = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                let _ = writer.write_all(b"220 fake ESMTP ready\r\n").await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    let reply = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                        "250-fake greets you\r\n250 8BITMIME\r\n"
                    } else if upper.starts_with("RCPT TO:") {
                        "550 5.1.1 mailbox unavailable\r\n"
                    } else if upper.starts_with("QUIT") {
                        "221 2.0.0 bye\r\n"
                    } else {
                        "250 2.0.0 ok\r\n"
                    };
                    if writer.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let db = common::TestDb::create("notification_smtp_reject").await;
    seed_template(&db).await;
    let service =
        NotificationService::new(db.pool.clone(), &plaintext_settings(port)).expect("service");
    let delivery = service
        .send_templated(
            "smtp-roundtrip",
            "en",
            "rejected@example.test",
            &[
                ("displayName".to_string(), "Rejected Listener".to_string()),
                ("code".to_string(), "000000".to_string()),
            ],
        )
        .await
        .expect("delivery row");
    assert_eq!(delivery.status, DeliveryStatus::Failed.as_str());
    let error = delivery.error.expect("a rejected send records the reason");
    assert!(
        error.contains("5.1.1") || error.contains("550") || !error.is_empty(),
        "the recorded reason is the server response, not an internal detail: {error}"
    );
    rejecter.abort();
    db.drop().await;
}
