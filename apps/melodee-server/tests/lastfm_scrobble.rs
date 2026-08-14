//! Phase 9 WP-07 Last.fm integration acceptance (PRD-EXT-001..004,
//! PRD-AUTH-002; legacy ADR-0003/0004; ADR-0134 §7 amended) against a local
//! mock Last.fm Web Services server — no real network. Covers the session
//! exchange success/failure contract, the exact scrobble/now-playing
//! parameter mapping (including the processing-time timestamp and
//! `trackNumber ?? 0` quirks), the invalid-session link clearing (API errors
//! 4/9/10), the unconfigured/unlinked silent no-ops, disconnect idempotency,
//! the at-rest secret invariant, and the PRD-EXT-002 no-provider-required
//! catalog-entry invariant.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{Request, StatusCode},
    routing::get,
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use melodee_auth::{
    LASTFM_SEALING_KEY_DOMAIN, PasswordHashAlgorithm, SecretCipher, SecretString,
    derive_sealing_key, lastfm_session_key_digest,
};
use melodee_persistence::{AuthStore, LastfmLinkStore, NativeUserImport, SettingsStore};
use melodee_server::lastfm::{
    DispatchOutcome, LastfmDispatch, LastfmSession, install_api_base_url_for_testing,
    store_linked_session,
};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const API_KEY: &str = "test-lastfm-api-key";
const SHARED_SECRET: &str = "test-lastfm-shared-secret";
const SERVER_SECRET: &str = "test-only-server-secret-0123456789abcdef";
const SESSION_KEY: &str = "mock-session-key-valid";
const STALE_SESSION_KEY: &str = "mock-session-key-stale";

#[derive(Debug, Clone)]
struct Captured {
    method: String,
    query: String,
    body: String,
}

struct MockHandle {
    base_url: String,
    captured: Arc<Mutex<Vec<Captured>>>,
}

/// The process-wide mock Last.fm server. Behavior is keyed off request
/// content so parallel tests share one server: the session exchange accepts
/// only `token=valid-token`; scrobbles signed with the stale session key get
/// API error 9 (invalid session), every other scrobble is accepted.
async fn mock() -> &'static MockHandle {
    static MOCK: tokio::sync::OnceCell<MockHandle> = tokio::sync::OnceCell::const_new();
    MOCK.get_or_init(|| async {
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/2.0/",
                get({
                    let captured = captured.clone();
                    move |Query(params): Query<Vec<(String, String)>>| {
                        let captured = captured.clone();
                        async move {
                            let query = params
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join("&");
                            captured.lock().expect("captured").push(Captured {
                                method: "GET".to_string(),
                                query: query.clone(),
                                body: String::new(),
                            });
                            let token = params
                                .iter()
                                .find(|(k, _)| k == "token")
                                .map(|(_, v)| v.as_str());
                            if token == Some("valid-token") {
                                serde_json::json!({
                                    "session": {"key": SESSION_KEY, "name": "mocklastfmuser", "subscriber": 0}
                                })
                                .to_string()
                            } else {
                                serde_json::json!({"error": 14, "message": "Unauthorized Token"})
                                    .to_string()
                            }
                        }
                    }
                })
                .post({
                    let captured = captured.clone();
                    move |body: String| {
                        let captured = captured.clone();
                        async move {
                            captured.lock().expect("captured").push(Captured {
                                method: "POST".to_string(),
                                query: String::new(),
                                body: body.clone(),
                            });
                            let params = form_decode(&body);
                            let method = param(&params, "method").unwrap_or_default();
                            let sk = param(&params, "sk").unwrap_or_default();
                            match (method.as_str(), sk.as_str()) {
                                ("track.scrobble", STALE_SESSION_KEY) => {
                                    serde_json::json!({"error": 9, "message": "Invalid session key"})
                                        .to_string()
                                }
                                ("track.scrobble", _) => serde_json::json!({
                                    "scrobbles": {"@attr": {"accepted": 1, "ignored": 0}, "scrobble": {}}
                                })
                                .to_string(),
                                ("track.updateNowPlaying", _) => {
                                    serde_json::json!({"nowplaying": {}}).to_string()
                                }
                                _ => serde_json::json!({"error": 6, "message": "Invalid parameters"})
                                    .to_string(),
                            }
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let base_url = format!("http://{address}");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        // Route wiring builds clients against this override (production
        // servers never install one).
        assert!(install_api_base_url_for_testing(&base_url));
        MockHandle { base_url, captured }
    })
    .await
}

fn form_decode(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(name), percent_decode(value))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).expect("utf-8 form value")
}

fn param(params: &[(String, String)], name: &str) -> Option<String> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// Recomputes the Last.fm `api_sig` for captured parameters (everything
/// except `api_sig` and `format`), per the protocol spec.
fn expected_signature(params: &[(String, String)]) -> String {
    let mut signed: Vec<(String, String)> = params
        .iter()
        .filter(|(name, _)| name != "api_sig" && name != "format")
        .cloned()
        .collect();
    signed.sort();
    use md5::Digest;
    let mut hasher = md5::Md5::new();
    for (name, value) in &signed {
        hasher.update(name.as_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(SHARED_SECRET.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn captured_requests(mock: &MockHandle) -> Vec<Captured> {
    mock.captured.lock().expect("captured").clone()
}

#[derive(Debug)]
struct Signer;
impl NativeStreamUrlSigner for Signer {
    fn stream_url(&self, _: Uuid, _: Uuid, _: &str) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Ok(String::new()) })
    }
}

async fn seed_user(db: &common::TestDb, username: &str) -> melodee_persistence::StoredNativeUser {
    let now = OffsetDateTime::now_utc();
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
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("test user")
}

fn native_state(db: &common::TestDb) -> NativeState {
    NativeState {
        pool: db.pool.clone(),
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            db.pool.clone(),
        )),
        search: None,
        public_origin: "http://melodee.test".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(Signer),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_string(),
                90,
            )
            .expect("test auth config"),
        ),
    }
}

async fn native_request(
    state: NativeState,
    principal: NativePrincipal,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).expect("json")))
        .expect("request");
    request.extensions_mut().insert(principal);
    request
        .extensions_mut()
        .insert(NativeClientBinding("test-client".to_string()));
    let response = melodee_server::native::router()
        .with_state(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON body")
    };
    (status, body)
}

fn principal(user: &melodee_persistence::StoredNativeUser) -> NativePrincipal {
    NativePrincipal {
        user_id: user.id,
        public_api_id: user.public_api_id,
        is_admin: false,
        is_editor: false,
    }
}

/// Configures Last.fm scrobbling plus the server secret through the WP-08
/// layered settings store.
async fn configure_lastfm(db: &common::TestDb) {
    let store = SettingsStore::new();
    let now = OffsetDateTime::now_utc();
    for (key, value) in [
        ("scrobbling.lastFm.Enabled", serde_json::json!(true)),
        ("scrobbling.lastFm.apiKey", serde_json::json!(API_KEY)),
        (
            "scrobbling.lastFm.sharedSecret",
            serde_json::json!(SHARED_SECRET),
        ),
        ("security.secretKey", serde_json::json!(SERVER_SECRET)),
    ] {
        store
            .set(&db.pool, key, &value, None, now)
            .await
            .expect("set");
    }
}

fn test_cipher() -> SecretCipher {
    SecretCipher::new(&derive_sealing_key(
        LASTFM_SEALING_KEY_DOMAIN,
        &SecretString::new(SERVER_SECRET.to_string()),
    ))
    .expect("cipher")
}

#[tokio::test]
async fn auth_url_configured_returns_url_shape_and_unconfigured_keeps_golden() {
    let db = common::TestDb::create("lastfm_auth_url").await;
    let user = seed_user(&db, "lastfm-auth-url").await;
    let state = native_state(&db);
    let principal = principal(&user);

    // Unconfigured: the frozen golden shape (byte-identical contract).
    let (status, body) = native_request(
        state.clone(),
        principal,
        "GET",
        "/api/v1/scrobble/lastfm/auth-url?callback=https%3A%2F%2Fclient.example%2Fcallback",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "BAD_REQUEST");
    assert_eq!(body["message"], "Last.fm not configured");

    // Configured: the source-evidenced success shape (capture gap recorded
    // in the evidence doc).
    SettingsStore::new()
        .set(
            &db.pool,
            "scrobbling.lastFm.apiKey",
            &serde_json::json!(API_KEY),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("set apiKey");
    let (status, body) = native_request(
        state.clone(),
        principal,
        "GET",
        "/api/v1/scrobble/lastfm/auth-url?callback=https%3A%2F%2Fclient.example%2Fcallback%3Fx%3D1",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["url"],
        format!(
            "https://www.last.fm/api/auth/?api_key={API_KEY}&cb=https%3A%2F%2Fclient.example%2Fcallback%3Fx%3D1"
        )
    );

    // Callback validation still rejects relative/foreign schemes.
    let (status, body) = native_request(
        state,
        principal,
        "GET",
        "/api/v1/scrobble/lastfm/auth-url?callback=ftp://client.example/cb",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Invalid callback url");
    db.drop().await;
}

#[tokio::test]
async fn session_exchange_success_links_account_and_never_persists_plaintext() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_session_ok").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-session-ok").await;
    let state = native_state(&db);

    let (status, body) = native_request(
        state,
        principal(&user),
        "POST",
        "/api/v1/scrobble/lastfm/session",
        serde_json::json!({"token": "valid-token"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"message": "Last.fm linked"}));

    // The exchange request hit the mock with the signed legacy parameter
    // set (evidence doc §3).
    let exchange = captured_requests(mock)
        .into_iter()
        .find(|request| {
            request.method == "GET"
                && request.query.contains("auth.getSession")
                && request.query.contains("token=valid-token")
        })
        .expect("exchange request");
    assert!(exchange.query.contains("api_key=test-lastfm-api-key"));
    assert!(exchange.query.contains("token=valid-token"));
    assert!(exchange.query.contains("api_sig="));
    assert!(exchange.query.contains("format=json"));

    // The link is stored as digest + AEAD ciphertext; the raw session key
    // never appears in the row bytes (ADR-0134 §7 amended).
    let link = LastfmLinkStore::new()
        .get(&db.pool, user.id)
        .await
        .expect("get")
        .expect("linked");
    assert_eq!(link.lastfm_username.as_deref(), Some("mocklastfmuser"));
    assert_eq!(
        link.session_key_digest,
        lastfm_session_key_digest(SESSION_KEY.as_bytes())
    );
    assert_eq!(
        test_cipher()
            .open(&link.session_key_ciphertext)
            .expect("open"),
        SESSION_KEY.as_bytes()
    );
    let raw: (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT session_key_digest, session_key_ciphertext FROM native_lastfm_links WHERE user_id=$1",
    )
    .bind(user.id)
    .fetch_one(&db.pool)
    .await
    .expect("raw row");
    let mut raw_bytes = raw.0.clone();
    raw_bytes.extend_from_slice(&raw.1);
    assert!(
        !raw_bytes
            .windows(SESSION_KEY.len())
            .any(|window| window == SESSION_KEY.as_bytes()),
        "raw session key must never be persisted"
    );
    db.drop().await;
}

#[tokio::test]
async fn session_exchange_failure_keeps_the_captured_400_shape() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_session_fail").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-session-fail").await;
    let state = native_state(&db);

    let (status, body) = native_request(
        state,
        principal(&user),
        "POST",
        "/api/v1/scrobble/lastfm/session",
        serde_json::json!({"token": "unauthorized-token"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "BAD_REQUEST");
    assert_eq!(body["message"], "Unable to exchange Last.fm session key");
    assert!(
        !LastfmLinkStore::new()
            .exists(&db.pool, user.id)
            .await
            .expect("exists")
    );
    assert!(
        captured_requests(mock)
            .iter()
            .any(|request| request.query.contains("token=unauthorized-token"))
    );
    db.drop().await;
}

#[tokio::test]
async fn scrobble_dispatch_maps_the_legacy_parameters_exactly() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_scrobble_map").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-scrobble-map").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7301,
        CatalogFixturePresentation {
            release_title: "Scrobble Mapping Release",
            release_artist: "Scrobble Release Artist",
            track_title: "Scrobble Mapping Track",
            track_artist: "Scrobble Track Artist",
            release_date: "2026-08-14",
        },
    )
    .await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track");
    let (track_number, duration_ms): (Option<i32>, Option<i64>) = sqlx::query_as(
        "SELECT track.track_number, COALESCE(track.duration_ms, media.duration_ms) \
         FROM tracks track JOIN media_assets media ON media.id = track.playable_media_asset_id \
         WHERE track.public_api_id = $1",
    )
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("track facts");

    // Link the user with the valid session key via the same store path the
    // session route uses.
    store_linked_session(
        &db.pool,
        user.id,
        &LastfmSession {
            key: SecretString::new(SESSION_KEY.to_string()),
            username: None,
        },
    )
    .await
    .expect("link");

    let before = OffsetDateTime::now_utc().unix_timestamp();
    let dispatch = LastfmDispatch::with_api_base_url(db.pool.clone(), &mock.base_url);
    let outcome = dispatch.submit_scrobble(user.id, track_public_id).await;
    let after = OffsetDateTime::now_utc().unix_timestamp();
    assert_eq!(outcome, DispatchOutcome::Submitted);

    let request = captured_requests(mock)
        .into_iter()
        .find(|request| {
            request.method == "POST"
                && request.body.contains("track.scrobble")
                && request.body.contains(&format!("sk={SESSION_KEY}"))
        })
        .expect("scrobble request");
    let params = form_decode(&request.body);
    assert_eq!(param(&params, "method").as_deref(), Some("track.scrobble"));
    assert_eq!(param(&params, "sk").as_deref(), Some(SESSION_KEY));
    assert_eq!(param(&params, "api_key").as_deref(), Some(API_KEY));
    assert_eq!(
        param(&params, "track").as_deref(),
        Some("Scrobble Mapping Track")
    );
    assert_eq!(
        param(&params, "artist").as_deref(),
        Some("Scrobble Track Artist")
    );
    assert_eq!(
        param(&params, "album").as_deref(),
        Some("Scrobble Mapping Release")
    );
    assert_eq!(
        param(&params, "albumArtist").as_deref(),
        Some("Scrobble Release Artist")
    );
    // Legacy quirks: trackNumber ?? 0, chosenByUser = !randomized, and the
    // timestamp stamped at processing time (never client-submitted).
    assert_eq!(
        param(&params, "trackNumber").as_deref(),
        Some(track_number.unwrap_or(0).to_string().as_str())
    );
    assert_eq!(param(&params, "chosenByUser").as_deref(), Some("1"));
    let timestamp: i64 = param(&params, "timestamp")
        .expect("timestamp")
        .parse()
        .expect("unix timestamp");
    assert!((before..=after).contains(&timestamp));
    if let Some(duration_ms) = duration_ms {
        assert_eq!(
            param(&params, "duration").as_deref(),
            Some((duration_ms / 1000).to_string().as_str())
        );
    }
    // The optional mbid is never sent: the Rust catalog does not store
    // MusicBrainz recording IDs yet.
    assert!(param(&params, "mbid").is_none());
    // api_sig signs every parameter except api_sig and format.
    assert_eq!(
        param(&params, "api_sig").as_deref(),
        Some(expected_signature(&params).as_str())
    );
    assert_eq!(param(&params, "format").as_deref(), Some("json"));
    db.drop().await;
}

#[tokio::test]
async fn now_playing_dispatch_sends_the_legacy_parameter_set() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_now_playing").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-now-playing").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7302,
        CatalogFixturePresentation {
            release_title: "Now Playing Release",
            release_artist: "Now Playing Release Artist",
            track_title: "Now Playing Track",
            track_artist: "Now Playing Track Artist",
            release_date: "2026-08-14",
        },
    )
    .await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track");
    store_linked_session(
        &db.pool,
        user.id,
        &LastfmSession {
            key: SecretString::new(SESSION_KEY.to_string()),
            username: None,
        },
    )
    .await
    .expect("link");

    let dispatch = LastfmDispatch::with_api_base_url(db.pool.clone(), &mock.base_url);
    let outcome = dispatch.submit_now_playing(user.id, track_public_id).await;
    assert_eq!(outcome, DispatchOutcome::Submitted);

    let request = captured_requests(mock)
        .into_iter()
        .find(|request| {
            request.body.contains("track.updateNowPlaying")
                && request.body.contains(&format!("sk={SESSION_KEY}"))
        })
        .expect("now-playing request");
    let params = form_decode(&request.body);
    assert_eq!(
        param(&params, "method").as_deref(),
        Some("track.updateNowPlaying")
    );
    assert_eq!(param(&params, "sk").as_deref(), Some(SESSION_KEY));
    assert_eq!(
        param(&params, "track").as_deref(),
        Some("Now Playing Track")
    );
    assert_eq!(
        param(&params, "artist").as_deref(),
        Some("Now Playing Track Artist")
    );
    // Legacy now-playing sends no timestamp, duration, mbid, or
    // chosenByUser (evidence doc §4).
    for absent in ["timestamp", "duration", "mbid", "chosenByUser"] {
        assert!(param(&params, absent).is_none(), "{absent} must be absent");
    }
    assert_eq!(
        param(&params, "api_sig").as_deref(),
        Some(expected_signature(&params).as_str())
    );
    db.drop().await;
}

#[tokio::test]
async fn invalid_session_error_clears_the_link_and_drops_the_scrobble() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_invalid_session").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-invalid-session").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7303,
        CatalogFixturePresentation {
            track_title: "Invalid Session Track",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track");
    store_linked_session(
        &db.pool,
        user.id,
        &LastfmSession {
            key: SecretString::new(STALE_SESSION_KEY.to_string()),
            username: None,
        },
    )
    .await
    .expect("link");

    let dispatch = LastfmDispatch::with_api_base_url(db.pool.clone(), &mock.base_url);
    let outcome = dispatch.submit_scrobble(user.id, track_public_id).await;
    // Legacy ADR-0004: API error 9 clears the stored key; re-linking is the
    // only recovery; the scrobble itself is dropped without retry.
    assert_eq!(outcome, DispatchOutcome::Dropped);
    assert!(
        !LastfmLinkStore::new()
            .exists(&db.pool, user.id)
            .await
            .expect("exists")
    );
    db.drop().await;
}

#[tokio::test]
async fn unconfigured_and_unlinked_dispatches_are_silent_no_ops() {
    let mock = mock().await;
    let db = common::TestDb::create("lastfm_noops").await;
    let user = seed_user(&db, "lastfm-noop").await;
    // A unique track title is the per-test marker in the shared mock's
    // captured traffic (tests run in parallel against one server).
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7304,
        CatalogFixturePresentation {
            track_title: "Silent Noop Track",
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    let track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("track");
    let dispatch = LastfmDispatch::with_api_base_url(db.pool.clone(), &mock.base_url);
    let no_traffic = |mock: &MockHandle| {
        !captured_requests(mock)
            .iter()
            .any(|request| request.body.contains("Silent%20Noop%20Track"))
    };

    // Nothing configured: the legacy "Last.fm scrobbling not configured"
    // success/no-op, with zero HTTP traffic.
    assert_eq!(
        dispatch.submit_scrobble(user.id, track_public_id).await,
        DispatchOutcome::NotConfigured
    );
    assert!(no_traffic(mock));

    // Enabled but the user has no link: the ADR-0004 silent no-op.
    configure_lastfm(&db).await;
    assert_eq!(
        dispatch.submit_scrobble(user.id, track_public_id).await,
        DispatchOutcome::NotLinked
    );
    assert_eq!(
        dispatch.submit_now_playing(user.id, track_public_id).await,
        DispatchOutcome::NotLinked
    );
    assert!(no_traffic(mock));

    // Enabled and linked but the server secret is gone: fail-degraded, no
    // traffic, and the link is left intact for a later, fixed deployment.
    store_linked_session(
        &db.pool,
        user.id,
        &LastfmSession {
            key: SecretString::new(SESSION_KEY.to_string()),
            username: None,
        },
    )
    .await
    .expect("link");
    sqlx::query("DELETE FROM server_settings WHERE key = 'security.secretKey'")
        .execute(&db.pool)
        .await
        .expect("delete secret");
    assert_eq!(
        dispatch.submit_scrobble(user.id, track_public_id).await,
        DispatchOutcome::Dropped
    );
    assert!(no_traffic(mock));
    assert!(
        LastfmLinkStore::new()
            .exists(&db.pool, user.id)
            .await
            .expect("exists")
    );
    db.drop().await;
}

#[tokio::test]
async fn disconnect_is_idempotent_and_clears_the_link() {
    mock().await;
    let db = common::TestDb::create("lastfm_disconnect").await;
    configure_lastfm(&db).await;
    let user = seed_user(&db, "lastfm-disconnect").await;
    let state = native_state(&db);
    store_linked_session(
        &db.pool,
        user.id,
        &LastfmSession {
            key: SecretString::new(SESSION_KEY.to_string()),
            username: None,
        },
    )
    .await
    .expect("link");

    let (status, body) = native_request(
        state.clone(),
        principal(&user),
        "POST",
        "/api/v1/scrobble/lastfm/disconnect",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"message": "Last.fm disconnected"}));
    assert!(
        !LastfmLinkStore::new()
            .exists(&db.pool, user.id)
            .await
            .expect("exists")
    );

    // Idempotent: succeeds when nothing is linked (captured golden).
    let (status, body) = native_request(
        state,
        principal(&user),
        "POST",
        "/api/v1/scrobble/lastfm/disconnect",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"message": "Last.fm disconnected"}));
    db.drop().await;
}

#[tokio::test]
async fn catalog_entry_never_requires_a_provider() {
    // PRD-EXT-002: a structurally valid local Release enters the catalog
    // with zero external providers configured. The structural guarantee is
    // that catalog/import lives in melodee-persistence/melodee-catalog while
    // every provider client lives in melodee-server behind settings gates;
    // this test proves the behavioral guarantee: a full catalog import
    // succeeds with no provider settings rows at all, and no provider cache
    // traffic is produced.
    let db = common::TestDb::create("lastfm_no_provider_required").await;
    let setting_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM server_settings")
        .fetch_one(&db.pool)
        .await
        .expect("settings count");
    assert_eq!(setting_rows, 0, "no providers configured");
    let fixture =
        phase5_common::seed_cataloged_release(&db, 7305, CatalogFixturePresentation::default())
            .await;
    let releases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM releases WHERE id = $1")
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("release count");
    assert_eq!(releases, 1, "release entered the catalog provider-free");
    let cache_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM external_provider_cache")
        .fetch_one(&db.pool)
        .await
        .expect("cache count");
    assert_eq!(cache_rows, 0, "catalog entry never touches providers");
    db.drop().await;
}
