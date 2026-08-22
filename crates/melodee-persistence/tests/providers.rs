//! external provider cache and Last.fm link persistence
//! acceptance (ADR-0134 §7 as amended 2026-08-14, PRD-EXT-001..004,
//! PRD-AUTH-002): migration shape (nothing seeded), provider cache
//! round-trip/freshness/purge with the enumerated-vocabulary CHECK as
//! backstop, Last.fm link round-trip/re-link/unlink idempotency, ON DELETE
//! CASCADE from `native_users`, and the at-rest invariant that the raw
//! session key is never persisted (digest + AEAD ciphertext only).

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_auth::{
    LASTFM_SEALING_KEY_DOMAIN, PasswordHashAlgorithm, SecretCipher, SecretString,
    derive_sealing_key, lastfm_session_key_digest,
};
use melodee_persistence::{
    AuthStore, KNOWN_PROVIDERS, LastfmLinkStore, NativeUserImport, ProviderCacheEntry,
    ProviderCacheStore, ProviderStoreError,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

/// A sentinel raw session key; tests prove these bytes never appear in the
/// stored row.
const SENTINEL_SESSION_KEY: &str = "sentinel-lastfm-session-key-9f8e7d6c5b4a";

fn test_cipher() -> SecretCipher {
    let secret = SecretString::new("test-only server secret, 32+ bytes long".to_string());
    SecretCipher::new(&derive_sealing_key(LASTFM_SEALING_KEY_DOMAIN, &secret)).expect("cipher")
}

fn entry(provider: &str, request_key: &str, fetched: OffsetDateTime) -> ProviderCacheEntry {
    ProviderCacheEntry {
        provider: provider.to_string(),
        request_key: request_key.to_string(),
        payload: serde_json::json!({"results": [{"name": "Fixture Artist"}]}),
        fetched_at: fetched,
        expires_at: fetched + Duration::hours(6),
    }
}

async fn seed_user(db: &TestDb, username: &str) -> Uuid {
    let normalized_username = username.to_ascii_uppercase();
    let email = format!("{username}@example.test");
    let normalized_email = email.to_ascii_uppercase();
    let roles = vec!["user".to_string()];
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username,
                normalized_username: &normalized_username,
                email: &email,
                normalized_email: &normalized_email,
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &roles,
                created_at: NOW,
                updated_at: NOW,
            },
        )
        .await
        .expect("test user")
        .id
}

#[tokio::test]
async fn migration_seeds_nothing_and_enforces_the_provider_vocabulary() {
    let db = TestDb::create("providers_shape").await;
    let cache = ProviderCacheStore::new();

    // Every enumerated provider round-trips.
    for provider in KNOWN_PROVIDERS {
        cache
            .upsert(&db.pool, &entry(provider, &format!("key-{provider}"), NOW))
            .await
            .expect("upsert enumerated provider");
    }
    // An unknown provider is rejected by the store and by the SQL CHECK
    // backstop (fail closed on both layers).
    assert!(matches!(
        cache.upsert(&db.pool, &entry("amg", "key", NOW)).await,
        Err(ProviderStoreError::InvalidEntry(_))
    ));
    let backstop = sqlx::query(
        "INSERT INTO external_provider_cache (provider, request_key, payload, fetched_at, expires_at) \
         VALUES ('amg', 'key', '{}'::jsonb, now(), now() + interval '1 hour')",
    )
    .execute(&db.pool)
    .await;
    assert!(backstop.is_err(), "CHECK rejects unknown providers");
    db.drop().await;
}

#[tokio::test]
async fn cache_round_trip_freshness_and_purge() {
    let db = TestDb::create("providers_cache").await;
    let cache = ProviderCacheStore::new();

    cache
        .upsert(&db.pool, &entry("lastfm", "hashed-key", NOW))
        .await
        .expect("upsert");
    let fresh = cache
        .get_if_fresh(&db.pool, "lastfm", "hashed-key", NOW + Duration::hours(1))
        .await
        .expect("get")
        .expect("fresh entry");
    assert_eq!(fresh.payload["results"][0]["name"], "Fixture Artist");

    // Expired rows read as absent...
    assert!(
        cache
            .get_if_fresh(&db.pool, "lastfm", "hashed-key", NOW + Duration::hours(7))
            .await
            .expect("get")
            .is_none()
    );

    //...upsert replaces in place (PK conflict), including refreshing a
    // previously expired entry...
    let refreshed = ProviderCacheEntry {
        payload: serde_json::json!({"results": []}),
        ..entry("lastfm", "hashed-key", NOW + Duration::hours(8))
    };
    cache.upsert(&db.pool, &refreshed).await.expect("refresh");
    let reloaded = cache
        .get_if_fresh(&db.pool, "lastfm", "hashed-key", NOW + Duration::hours(9))
        .await
        .expect("get")
        .expect("refreshed");
    assert_eq!(reloaded.payload["results"], serde_json::json!([]));

    //...and the bounded purge removes only expired rows.
    cache
        .upsert(
            &db.pool,
            &entry("discogs", "still-fresh", NOW + Duration::hours(8)),
        )
        .await
        .expect("upsert fresh");
    let stale = ProviderCacheEntry {
        expires_at: NOW + Duration::hours(1),
        ..entry("wikidata", "stale", NOW)
    };
    cache.upsert(&db.pool, &stale).await.expect("upsert stale");
    let purged = cache
        .purge_expired(&db.pool, NOW + Duration::hours(2))
        .await
        .expect("purge");
    assert_eq!(purged, 1);
    assert!(
        cache
            .get_if_fresh(&db.pool, "discogs", "still-fresh", NOW + Duration::hours(9))
            .await
            .expect("get")
            .is_some()
    );

    // Bounds: oversized payload and inverted expiry are rejected before SQL.
    let oversized = ProviderCacheEntry {
        payload: serde_json::json!("x".repeat(300 * 1024)),
        ..entry("lastfm", "big", NOW)
    };
    assert!(matches!(
        cache.upsert(&db.pool, &oversized).await,
        Err(ProviderStoreError::InvalidEntry(_))
    ));
    let inverted = ProviderCacheEntry {
        expires_at: NOW,
        ..entry("lastfm", "inverted", NOW)
    };
    assert!(matches!(
        cache.upsert(&db.pool, &inverted).await,
        Err(ProviderStoreError::InvalidEntry(_))
    ));
    db.drop().await;
}

#[tokio::test]
async fn lastfm_link_round_trip_relink_and_unlink_are_idempotent() {
    let db = TestDb::create("providers_link").await;
    let store = LastfmLinkStore::new();
    let cipher = test_cipher();
    let user = seed_user(&db, "lastfmuser").await;

    assert!(!store.exists(&db.pool, user).await.expect("exists"));
    assert_eq!(store.count(&db.pool).await.expect("count"), 0);

    let ciphertext = cipher.seal(SENTINEL_SESSION_KEY.as_bytes()).expect("seal");
    let digest = lastfm_session_key_digest(SENTINEL_SESSION_KEY.as_bytes());
    store
        .link(
            &db.pool,
            user,
            Some("lastfmuser"),
            &digest,
            &ciphertext,
            NOW,
        )
        .await
        .expect("link");
    assert!(store.exists(&db.pool, user).await.expect("exists"));
    assert_eq!(store.count(&db.pool).await.expect("count"), 1);

    let record = store
        .get(&db.pool, user)
        .await
        .expect("get")
        .expect("linked");
    assert_eq!(record.lastfm_username.as_deref(), Some("lastfmuser"));
    assert_eq!(record.session_key_digest, digest);
    assert_eq!(
        cipher.open(&record.session_key_ciphertext).expect("unseal"),
        SENTINEL_SESSION_KEY.as_bytes()
    );
    // Debug output never carries key material.
    let debug = format!("{record:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(SENTINEL_SESSION_KEY));

    // Re-link replaces the row (legacy ADR-0004: re-authentication is the
    // only recovery flow).
    let rotated = cipher.seal(b"rotated-session-key").expect("seal");
    let rotated_digest = lastfm_session_key_digest(b"rotated-session-key");
    store
        .link(
            &db.pool,
            user,
            None,
            &rotated_digest,
            &rotated,
            NOW + Duration::hours(1),
        )
        .await
        .expect("re-link");
    let record = store
        .get(&db.pool, user)
        .await
        .expect("get")
        .expect("linked");
    assert_eq!(record.lastfm_username, None);
    assert_eq!(record.session_key_digest, rotated_digest);
    assert_eq!(store.count(&db.pool).await.expect("count"), 1);

    // Unlink is idempotent (legacy disconnect succeeds when unlinked).
    assert!(store.unlink(&db.pool, user).await.expect("unlink"));
    assert!(!store.unlink(&db.pool, user).await.expect("unlink again"));
    assert!(store.get(&db.pool, user).await.expect("get").is_none());
    db.drop().await;
}

#[tokio::test]
async fn lastfm_link_cascades_on_user_delete_and_never_persists_plaintext() {
    let db = TestDb::create("providers_plaintext").await;
    let store = LastfmLinkStore::new();
    let cipher = test_cipher();
    let user = seed_user(&db, "cascadeuser").await;

    let ciphertext = cipher.seal(SENTINEL_SESSION_KEY.as_bytes()).expect("seal");
    let digest = lastfm_session_key_digest(SENTINEL_SESSION_KEY.as_bytes());
    store
        .link(&db.pool, user, None, &digest, &ciphertext, NOW)
        .await
        .expect("link");

    // The raw session key bytes never appear anywhere in the stored row
    // (ADR-0134 §7 amended: digest + AEAD ciphertext only).
    let row: (Option<String>, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT lastfm_username, session_key_digest, session_key_ciphertext \
         FROM native_lastfm_links WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .expect("raw row");
    let mut raw_bytes = Vec::new();
    raw_bytes.extend_from_slice(row.0.unwrap_or_default().as_bytes());
    raw_bytes.extend_from_slice(&row.1);
    raw_bytes.extend_from_slice(&row.2);
    assert!(
        !raw_bytes
            .windows(SENTINEL_SESSION_KEY.len())
            .any(|window| window == SENTINEL_SESSION_KEY.as_bytes()),
        "raw session key must never be persisted"
    );
    assert_eq!(row.1.as_slice(), digest.as_slice(), "digest persisted");

    // Deleting the user cascades the link.
    sqlx::query("DELETE FROM native_users WHERE id=$1")
        .bind(user)
        .execute(&db.pool)
        .await
        .expect("delete user");
    assert!(!store.exists(&db.pool, user).await.expect("exists"));
    db.drop().await;
}
