//! notification persistence acceptance (ADR-0134 §4,
//! PRD-NOTIF-001/002): migration shape (seeded `en` templates), template
//! CRUD round-trip, bounds enforcement with the SQL CHECKs as backstop,
//! locale-keyed PK behavior, the append-only delivery audit, and bounded
//! retention purge.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    DeliveryStatus, NewNotificationDelivery, NotificationDeliveryStore, NotificationStoreError,
    NotificationTemplateStore, NotificationTemplateUpsert,
};
use time::{Duration, OffsetDateTime};

fn upsert<'a>(
    template_key: &'a str,
    locale: &'a str,
    subject: Option<&'a str>,
) -> NotificationTemplateUpsert<'a> {
    NotificationTemplateUpsert {
        template_key,
        locale,
        subject,
        body_text: Some("Hello {{name}}"),
        body_html: Some("<p>Hello {{name}}</p>"),
    }
}

#[tokio::test]
async fn migration_seeds_default_en_templates() {
    let db = TestDb::create("notifications_seed").await;
    let store = NotificationTemplateStore::new();

    let seeded = store.list(&db.pool, 50, 0).await.expect("list seeds");
    let keys: Vec<&str> = seeded.iter().map(|row| row.template_key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["request.fulfilled", "share.created", "user.password_reset"],
        "all three wired events are seeded, ordered by key"
    );
    assert!(seeded.iter().all(|row| row.locale == "en"));
    assert!(seeded.iter().all(|row| row.version == 1));

    // Legacy-captured subject (email.resetPassword.subject).
    let reset = store
        .get(&db.pool, "user.password_reset", "en")
        .await
        .expect("seeded reset template");
    assert_eq!(
        reset.subject.as_deref(),
        Some("Reset your Melodee password")
    );
    assert!(
        reset
            .body_text
            .as_deref()
            .is_some_and(|body| body.contains("{{resetUrl}}"))
    );
    db.drop().await;
}

#[tokio::test]
async fn template_crud_round_trip_and_version_churn() {
    let db = TestDb::create("notifications_crud").await;
    let store = NotificationTemplateStore::new();
    let now = OffsetDateTime::now_utc();

    let created = store
        .upsert(&db.pool, &upsert("custom.event", "en", Some("Hello")), now)
        .await
        .expect("create");
    assert_eq!(created.version, 1);
    assert_eq!(created.body_text.as_deref(), Some("Hello {{name}}"));

    // Upsert on the same (key, locale) replaces and bumps the version.
    let replaced = store
        .upsert(
            &db.pool,
            &upsert("custom.event", "en", Some("Hello again")),
            now + Duration::seconds(1),
        )
        .await
        .expect("replace");
    assert_eq!(replaced.version, 2);
    assert_eq!(replaced.subject.as_deref(), Some("Hello again"));
    assert!(replaced.updated_at > created.updated_at);

    // Locale is part of the PK: other locales coexist independently.
    let german = store
        .upsert(
            &db.pool,
            &upsert("custom.event", "de-DE", Some("Hallo")),
            now,
        )
        .await
        .expect("create de-DE");
    assert_eq!(german.version, 1);
    let listed = store.list(&db.pool, 50, 0).await.expect("list");
    let custom: Vec<_> = listed
        .iter()
        .filter(|row| row.template_key == "custom.event")
        .collect();
    assert_eq!(custom.len(), 2);
    assert_eq!(custom[0].locale, "de-DE", "listing orders by key, locale");
    assert_eq!(custom[1].locale, "en");

    store
        .delete(&db.pool, "custom.event", "de-DE")
        .await
        .expect("delete");
    assert!(matches!(
        store.get(&db.pool, "custom.event", "de-DE").await,
        Err(NotificationStoreError::TemplateNotFound)
    ));
    assert!(matches!(
        store.delete(&db.pool, "custom.event", "de-DE").await,
        Err(NotificationStoreError::TemplateNotFound)
    ));

    // Deleting a seeded key is a supported operator action.
    store
        .delete(&db.pool, "share.created", "en")
        .await
        .expect("delete seeded");
    assert!(matches!(
        store.get(&db.pool, "share.created", "en").await,
        Err(NotificationStoreError::TemplateNotFound)
    ));
    db.drop().await;
}

#[tokio::test]
async fn bounds_are_enforced_in_code_and_by_sql_backstop() {
    let db = TestDb::create("notifications_bounds").await;
    let store = NotificationTemplateStore::new();
    let now = OffsetDateTime::now_utc();

    for bad_key in ["", "   ", &"k".repeat(101)] {
        let result = store
            .upsert(&db.pool, &upsert(bad_key, "en", None), now)
            .await;
        assert!(
            matches!(result, Err(NotificationStoreError::InvalidTemplate(_))),
            "key {bad_key:?}"
        );
    }
    for bad_locale in ["", "e", "english", "en_US", "en-", "en-x", &"en-".repeat(8)] {
        let result = store
            .upsert(&db.pool, &upsert("k", bad_locale, None), now)
            .await;
        assert!(
            matches!(result, Err(NotificationStoreError::InvalidTemplate(_))),
            "locale {bad_locale:?}"
        );
    }
    for good_locale in ["en", "en-US", "pt-BR", "zh-Hant", "es-419"] {
        assert!(
            store
                .upsert(&db.pool, &upsert("k", good_locale, None), now)
                .await
                .is_ok(),
            "locale {good_locale:?}"
        );
    }

    // Subject and body bounds (64 KiB body bound is byte-based).
    assert!(matches!(
        store
            .upsert(&db.pool, &upsert("k", "en", Some(&"s".repeat(501))), now)
            .await,
        Err(NotificationStoreError::InvalidTemplate(_))
    ));
    let mut big_body = upsert("k", "en", None);
    let oversized_body = "b".repeat(64 * 1024 + 1);
    big_body.body_text = Some(&oversized_body);
    assert!(matches!(
        store.upsert(&db.pool, &big_body, now).await,
        Err(NotificationStoreError::InvalidTemplate(_))
    ));

    // The migration CHECKs are the fail-closed backstop against raw SQL.
    for bad_sql in [
        "INSERT INTO notification_templates (template_key, locale) VALUES ('  ', 'en')",
        "INSERT INTO notification_templates (template_key, locale) VALUES ('k', 'english')",
        "INSERT INTO notification_templates (template_key, locale, subject) \
         VALUES ('k', 'en', repeat('s', 501))",
    ] {
        assert!(
            sqlx::query(bad_sql).execute(&db.pool).await.is_err(),
            "{bad_sql}"
        );
    }
    db.drop().await;
}

#[tokio::test]
async fn delivery_audit_is_append_only_bounded_and_retained() {
    let db = TestDb::create("notifications_deliveries").await;
    let store = NotificationDeliveryStore::new();
    let now = OffsetDateTime::now_utc();

    for (status, recipient) in [
        (DeliveryStatus::Sent, "first@example.test"),
        (DeliveryStatus::Failed, "second@example.test"),
        (DeliveryStatus::Skipped, "third@example.test"),
    ] {
        store
            .record_delivery(
                &db.pool,
                &NewNotificationDelivery {
                    template_key: "share.created",
                    recipient,
                    status,
                    error: (status == DeliveryStatus::Failed).then_some("smtp relay refused"),
                },
                now,
            )
            .await
            .expect("record");
    }

    let listed = store.list_recent(&db.pool, 50, 0).await.expect("list");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed[0].recipient, "third@example.test",
        "newest first (equal timestamps order by id desc)"
    );
    assert_eq!(listed[0].status, "skipped");
    let failed = listed
        .iter()
        .find(|row| row.status == "failed")
        .expect("failed row");
    assert_eq!(failed.error.as_deref(), Some("smtp relay refused"));

    // Bounds: blank/over-long recipients and over-long errors.
    for bad_recipient in ["", "  ", &"r".repeat(321)] {
        assert!(matches!(
            store
                .record_delivery(
                    &db.pool,
                    &NewNotificationDelivery {
                        template_key: "share.created",
                        recipient: bad_recipient,
                        status: DeliveryStatus::Sent,
                        error: None,
                    },
                    now,
                )
                .await,
            Err(NotificationStoreError::InvalidDelivery(_))
        ));
    }
    let long_error = "e".repeat(5000);
    let truncated = store
        .record_delivery(
            &db.pool,
            &NewNotificationDelivery {
                template_key: "share.created",
                recipient: "ops@example.test",
                status: DeliveryStatus::Failed,
                error: Some(&long_error),
            },
            now,
        )
        .await
        .expect("record truncated error");
    assert!(
        truncated
            .error
            .as_deref()
            .is_some_and(|error| error.chars().count() <= 2000 && error.ends_with('…'))
    );

    // The status CHECK is the fail-closed backstop for the audit vocabulary.
    let raw = sqlx::query(
        "INSERT INTO notification_deliveries (id, template_key, recipient, status) \
         VALUES ($1, 'k', 'r@example.test', 'bogus')",
    )
    .bind(uuid::Uuid::now_v7())
    .execute(&db.pool)
    .await;
    assert!(raw.is_err(), "unknown status must violate the CHECK");

    // Retention purge removes only rows older than the cutoff.
    sqlx::query(
        "INSERT INTO notification_deliveries (id, template_key, recipient, status, created_at) \
         VALUES ($1, 'share.created', 'old@example.test', 'sent', $2)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(now - Duration::days(90))
    .execute(&db.pool)
    .await
    .expect("insert old row");
    let purged = store
        .purge_older_than(&db.pool, now - Duration::days(30))
        .await
        .expect("purge");
    assert_eq!(purged, 1);
    let remaining = store.list_recent(&db.pool, 50, 0).await.expect("list");
    assert!(
        remaining
            .iter()
            .all(|row| row.recipient != "old@example.test")
    );
    assert_eq!(remaining.len(), 4);
    db.drop().await;
}
