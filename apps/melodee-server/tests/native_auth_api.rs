//! Native-v1 login/JWT/refresh/replay/logout compatibility evidence.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderValue, Request, StatusCode, header},
    response::IntoResponse,
};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use melodee_auth::{
    NativeAccessClaims, PasswordHashAlgorithm, RefreshTokenDigest, SecretString, issue_access_token,
};
use melodee_persistence::{AuthStore, NativeUserImport, SocialLoginLink};
use melodee_server::native::{
    NativeFuture, NativeState, NativeStreamUrlSigner, PostgresNativeUserStateReader,
    auth::NativeAuthService,
};
use time::OffsetDateTime;
use tokio::sync::Barrier;
use tower::ServiceExt;
use uuid::Uuid;

const GOOGLE_PRIVATE_KEY: &str = include_str!("fixtures/google-test-private.pem");
const GOOGLE_MODULUS: &str = "38l6wihKe_gOmh8-H5h0AHi2epwg6Aae42NG_X5PzYrqAsqJDgfgkYQNvFjNj5q8o0JioEnTPPybXZL8czN8t9bXvWO_w1YH6zGkSsKJUv8W9aFC3p7YIyMSg5RDrawzHCChVeQsVWdFeI7JcfGJQrZtGlAYtj4UKvNzhbhhoJVK4cxNOQJLPaY2JO8IoCVW9g2iEFB6xUOpdGiJXfP9zofpSFwgaHN1wB-937HTK7URwcuw9X8z32MBULsy4EQ1CT0K8CSD2MdRg1iG4q21f45AqeL9Ih8Oje3V-3W_J-pwPI8qYhhF5DPnuHnt4NDNHQ7aYqAy_uo7WyybaNrbOQ";

#[derive(Debug)]
struct TestSigner;

impl NativeStreamUrlSigner for TestSigner {
    fn stream_url(
        &self,
        _track_public_id: Uuid,
        _user_public_id: Uuid,
        _client_binding: &str,
    ) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Err(()) })
    }
}

async fn state(db: &common::TestDb) -> NativeState {
    let now = OffsetDateTime::now_utc();
    let roles = vec!["Administrator".to_string(), "HasStreamRole".to_string()];
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065")
                    .expect("captured public id"),
                username: "phase0-fixture",
                normalized_username: "PHASE0-FIXTURE",
                email: "phase0-fixture@example.test",
                normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                password_hash: &bcrypt::hash("Mel0deeR0cks!", 4).expect("fixture hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
                is_admin: true,
                is_editor: true,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: Some("en-US"),
                preferred_theme: Some("dark"),
                auth_revision: 1,
                roles: &roles,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import user");
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://melodee.test".to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(TestSigner),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            NativeAuthService::new(
                "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
                90,
            )
            .expect("auth config")
            .with_development_reset_tokens_for_tests(),
        ),
    }
}

async fn request(
    state: NativeState,
    path: &str,
    body: serde_json::Value,
    bearer: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let mut request = builder
        .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let response = melodee_server::native::configured_router(state)
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
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("JSON body")
    };
    (status, json)
}

async fn raw_post(
    state: NativeState,
    path: &str,
    body: serde_json::Value,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder
        .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    melodee_server::native::configured_router(state)
        .oneshot(request)
        .await
        .expect("response")
}

async fn raw_authenticated(
    state: NativeState,
    method: &str,
    path: &str,
    body: serde_json::Value,
    bearer: &str,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    melodee_server::native::configured_router(state)
        .oneshot(request)
        .await
        .expect("response")
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn login_unknown_and_wrong_password_share_the_exact_public_error() {
    let db = common::TestDb::create("server_native_auth_errors").await;
    let state = state(&db).await;
    let (wrong_status, wrong) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"wrong"}),
        None,
    )
    .await;
    let (unknown_status, unknown) = request(
        state,
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"does-not-exist","password":"wrong"}),
        None,
    )
    .await;
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong["code"], unknown["code"]);
    assert_eq!(wrong["message"], unknown["message"]);
    assert_eq!(wrong["code"], "UNAUTHORIZED");
    assert_eq!(wrong["message"], "Invalid credentials");
    db.drop().await;
}

#[tokio::test]
async fn invalid_and_expired_refresh_tokens_have_the_captured_distinct_messages() {
    let db = common::TestDb::create("server_native_auth_refresh_errors").await;
    let state = state(&db).await;
    let (invalid_status, invalid) = request(
        state.clone(),
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":"not-a-real-token"}),
        None,
    )
    .await;
    assert_eq!(invalid_status, StatusCode::UNAUTHORIZED);
    assert_eq!(invalid["code"], "refresh_token_invalid");
    assert_eq!(invalid["message"], "Refresh token is invalid.");

    let now = OffsetDateTime::now_utc();
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM native_users LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("user");
    let family_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_refresh_families (
            id,user_id,session_started_at,expires_at,created_at,updated_at
         ) VALUES ($1,$2,$3,$4,$3,$3)",
    )
    .bind(family_id)
    .bind(user_id)
    .bind(now - time::Duration::days(40))
    .bind(now + time::Duration::days(10))
    .execute(&db.pool)
    .await
    .expect("family");
    let expired_raw = "expired-refresh-token";
    let digest = RefreshTokenDigest::from_token(expired_raw.as_bytes());
    sqlx::query(
        "INSERT INTO native_refresh_tokens (
            id,family_id,token_digest,issued_at,expires_at,created_at
         ) VALUES ($1,$2,$3,$4,$5,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(family_id)
    .bind(digest.as_bytes().as_slice())
    .bind(now - time::Duration::days(40))
    .bind(now - time::Duration::days(10))
    .execute(&db.pool)
    .await
    .expect("expired token");
    let (expired_status, expired) = request(
        state,
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":expired_raw}),
        None,
    )
    .await;
    assert_eq!(expired_status, StatusCode::UNAUTHORIZED);
    assert_eq!(expired["code"], "refresh_token_invalid");
    assert_eq!(expired["message"], "Refresh token has expired.");
    db.drop().await;
}

#[tokio::test]
async fn malformed_and_expired_access_tokens_fail_with_an_empty_401() {
    let db = common::TestDb::create("server_native_auth_access_errors").await;
    let state = state(&db).await;
    let (malformed_status, malformed_body) = request(
        state.clone(),
        "/api/v1/auth/refresh",
        serde_json::json!({}),
        Some("not-a-jwt"),
    )
    .await;
    assert_eq!(malformed_status, StatusCode::UNAUTHORIZED);
    assert_eq!(malformed_body, serde_json::Value::Null);

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expired = issue_access_token(
        &SecretString::new("test-native-jwt-key-that-is-at-least-32-bytes".to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email: "phase0-fixture@example.test".to_string(),
            exp: now - 1,
            sid: Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").expect("public id"),
            iat: now - 901,
            iss: "MelodeeApi".to_string(),
            nbf: now - 901,
            role: vec!["Administrator".to_string()],
            unique_name: "phase0-fixture".to_string(),
            theme: "dark".to_string(),
        },
    )
    .expect("issue expired fixture");
    let (expired_status, expired_body) = request(
        state,
        "/api/v1/auth/refresh",
        serde_json::json!({}),
        Some(&expired),
    )
    .await;
    assert_eq!(expired_status, StatusCode::UNAUTHORIZED);
    assert_eq!(expired_body, serde_json::Value::Null);
    db.drop().await;
}

#[tokio::test]
async fn legacy_access_refresh_rejects_every_unavailable_account_state() {
    let db = common::TestDb::create("server_native_auth_refresh_account_state").await;
    let state = state(&db).await;

    for (column, expected_code) in [
        ("enabled=false", "account_disabled"),
        ("locked=true", "USER_LOCKED"),
        ("blacklisted=true", "BLACKLISTED"),
    ] {
        sqlx::query(
            "UPDATE native_users
             SET enabled=true,locked=false,blacklisted=false,auth_revision=auth_revision+1",
        )
        .execute(&db.pool)
        .await
        .expect("reset account state");
        let (_, login) = request(
            state.clone(),
            "/api/v1/auth/authenticate",
            serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
            None,
        )
        .await;
        let access = login["token"].as_str().expect("access token");
        let refresh = login["refreshToken"].as_str().expect("refresh token");
        let statement = format!("UPDATE native_users SET {column},auth_revision=auth_revision+1");
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&db.pool)
            .await
            .expect("set unavailable account state");

        let (status, body) = request(
            state.clone(),
            "/api/v1/auth/refresh",
            serde_json::json!({}),
            Some(access),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{column}");
        assert_eq!(body["code"], expected_code, "{column}");
        assert!(body.get("token").is_none(), "{column}");

        let (status, body) = request(
            state.clone(),
            "/api/v1/auth/refresh-token",
            serde_json::json!({"refreshToken":refresh}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{column}");
        assert_eq!(body["code"], "refresh_token_invalid", "{column}");
        let active_families: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM native_refresh_families WHERE revoked_at IS NULL",
        )
        .fetch_one(&db.pool)
        .await
        .expect("active families");
        assert_eq!(active_families, 0, "{column}");
    }
    db.drop().await;
}

#[tokio::test]
async fn browser_cookie_is_separate_from_bearer_and_cleared_on_sign_out() {
    let db = common::TestDb::create("server_native_cookie_auth").await;
    let state = state(&db).await;
    sqlx::query(
        "INSERT INTO native_themes(id,name,author,version,is_built_in,created_at,updated_at) \
         VALUES('fixture','Fixture','Melodee','1',true,now(),now())",
    )
    .execute(&db.pool)
    .await
    .expect("theme");

    let login = raw_post(
        state.clone(),
        "/api/v1/auth/cookie/sign-in",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
        None,
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("cookie")
        .to_string();
    assert!(set_cookie.starts_with("melodee_auth="));
    assert!(set_cookie.contains("; Path=/; Secure; SameSite=Strict; HttpOnly"));
    let cookie = set_cookie.split(';').next().expect("cookie pair");
    let login_body: serde_json::Value = serde_json::from_slice(
        &login
            .into_body()
            .collect()
            .await
            .expect("cookie login body")
            .to_bytes(),
    )
    .expect("cookie login JSON");
    assert_eq!(login_body, serde_json::json!({"message":"Signed in"}));

    let (_, jwt_login) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
    )
    .await;
    let jwt = jwt_login["token"].as_str().expect("jwt");
    let bearer_only = raw_post(
        state.clone(),
        "/api/v1/themes/me",
        serde_json::json!({"themeId":"fixture"}),
        Some(jwt),
        None,
    )
    .await;
    assert_eq!(bearer_only.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        bearer_only.headers().get(header::LOCATION),
        Some(&HeaderValue::from_static(
            "http://melodee.test/Account/Login?ReturnUrl=%2Fapi%2Fv1%2Fthemes%2Fme"
        ))
    );

    let theme = raw_post(
        state.clone(),
        "/api/v1/themes/me",
        serde_json::json!({"themeId":"fixture"}),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(theme.status(), StatusCode::OK);

    let missing_cookie = raw_post(
        state.clone(),
        "/api/v1/auth/cookie/sign-out",
        serde_json::json!({}),
        None,
        None,
    )
    .await;
    assert_eq!(missing_cookie.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing_cookie.headers().get(header::LOCATION),
        Some(&HeaderValue::from_static(
            "http://melodee.test/Account/Login?ReturnUrl=%2Fapi%2Fv1%2Fauth%2Fcookie%2Fsign-out"
        ))
    );

    let sign_out = raw_post(
        state.clone(),
        "/api/v1/auth/cookie/sign-out",
        serde_json::json!({}),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(sign_out.status(), StatusCode::OK);
    let cleared = sign_out
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("cleared cookie");
    assert!(cleared.starts_with("melodee_auth=; Expires=Thu, 01 Jan 1970"));
    let sign_out_body: serde_json::Value = serde_json::from_slice(
        &sign_out
            .into_body()
            .collect()
            .await
            .expect("sign-out body")
            .to_bytes(),
    )
    .expect("sign-out JSON");
    assert_eq!(sign_out_body, serde_json::json!({"message":"Signed out"}));
    db.drop().await;
}

#[tokio::test]
async fn password_reset_is_non_enumerating_one_use_and_revokes_refresh() {
    let db = common::TestDb::create("server_native_password_reset").await;
    let state = state(&db).await;
    let unknown = raw_post(
        state.clone(),
        "/api/v1/auth/password-reset/request",
        serde_json::json!({"email":"unknown@example.test"}),
        None,
        None,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::OK);
    let unknown_body: serde_json::Value = serde_json::from_slice(
        &unknown
            .into_body()
            .collect()
            .await
            .expect("unknown reset body")
            .to_bytes(),
    )
    .expect("unknown reset JSON");
    assert_eq!(
        unknown_body,
        serde_json::json!({
            "message":"If an account with that email exists, a password reset link has been sent."
        })
    );
    let known = raw_post(
        state.clone(),
        "/api/v1/auth/password-reset/request",
        serde_json::json!({"email":"phase0-fixture@example.test"}),
        None,
        None,
    )
    .await;
    assert_eq!(known.status(), StatusCode::OK);
    let known_json: serde_json::Value =
        serde_json::from_slice(&known.into_body().collect().await.expect("body").to_bytes())
            .expect("reset json");
    let token = known_json["resetToken"]
        .as_str()
        .expect("development token");

    let mut validate = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/auth/password-reset/validate/{token}"))
        .body(Body::empty())
        .expect("validate request");
    validate.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let valid = melodee_server::native::configured_router(state.clone())
        .oneshot(validate)
        .await
        .expect("validate");
    assert_eq!(valid.status(), StatusCode::OK);

    let mut invalid_validate = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/password-reset/validate/phase0-invalid-reset-token")
        .body(Body::empty())
        .expect("invalid validate request");
    invalid_validate.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let invalid_validate = melodee_server::native::configured_router(state.clone())
        .oneshot(invalid_validate)
        .await
        .expect("invalid validate");
    assert_eq!(invalid_validate.status(), StatusCode::BAD_REQUEST);
    let invalid_validate_body: serde_json::Value = serde_json::from_slice(
        &invalid_validate
            .into_body()
            .collect()
            .await
            .expect("invalid validate body")
            .to_bytes(),
    )
    .expect("invalid validate JSON");
    assert_eq!(invalid_validate_body["code"], "BAD_REQUEST");
    assert_eq!(invalid_validate_body["message"], "Invalid token");

    let confirmed = raw_post(
        state.clone(),
        "/api/v1/auth/password-reset/confirm",
        serde_json::json!({"token":token,"newPassword":"replacement-password"}),
        None,
        None,
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let reused = raw_post(
        state.clone(),
        "/api/v1/auth/password-reset/confirm",
        serde_json::json!({"token":token,"newPassword":"another-password"}),
        None,
        None,
    )
    .await;
    assert_eq!(reused.status(), StatusCode::BAD_REQUEST);
    let reused_body: serde_json::Value = serde_json::from_slice(
        &reused
            .into_body()
            .collect()
            .await
            .expect("invalid reset body")
            .to_bytes(),
    )
    .expect("invalid reset JSON");
    assert_eq!(reused_body["code"], "BAD_REQUEST");
    assert_eq!(reused_body["message"], "Invalid token");
    let (login_status, _) = request(
        state,
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"replacement-password"}),
        None,
    )
    .await;
    assert_eq!(login_status, StatusCode::OK);
    db.drop().await;
}

#[tokio::test]
async fn native_auth_rate_limit_is_cluster_backed_and_has_retry_after() {
    let db = common::TestDb::create("server_native_auth_rate_limit").await;
    let state = state(&db).await;
    for ordinal in 0..10 {
        let response = raw_post(
            state.clone(),
            "/api/v1/auth/authenticate",
            serde_json::json!({"userName":"rate-fixture","password":"wrong"}),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{ordinal}");
    }
    let limited = raw_post(
        state,
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"rate-fixture","password":"wrong"}),
        None,
        None,
    )
    .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));
    db.drop().await;
}

#[tokio::test]
async fn auth_configuration_bodies_and_provider_link_rate_are_bounded_fail_closed() {
    assert!(NativeAuthService::new("x".repeat(31), 90).is_err());
    assert!(NativeAuthService::new("x".repeat(4097), 90).is_err());
    assert!(
        NativeAuthService::new("replace-with-at-least-32-random-bytes".to_string(), 90).is_err(),
        "the checked-in example sentinel must never become a signing key"
    );
    assert!(
        melodee_server::native::auth::cors_from_origins("https://user:secret@example.test")
            .is_err()
    );
    assert!(melodee_server::native::auth::cors_from_origins("*").is_err());
    assert!(melodee_server::native::auth::cors_from_origins(&"x".repeat(32 * 1024 + 1)).is_err());
    assert!(
        NativeAuthService::new(
            "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
            90,
        )
        .expect("auth")
        .with_loopback_google_oidc_for_tests("http://127.0.0.1/jwks", "x".repeat(2049), Vec::new(),)
        .is_err()
    );
    let debug_service = NativeAuthService::new(
        "debug-secret-native-jwt-key-that-is-at-least-32-bytes".to_string(),
        90,
    )
    .expect("debug service")
    .with_loopback_google_oidc_for_tests(
        "http://127.0.0.1/jwks",
        "debug-google-client-id-must-be-redacted".to_string(),
        vec!["example.test".to_string()],
    )
    .expect("debug OIDC");
    let debug = format!("{debug_service:?}");
    assert!(!debug.contains("debug-secret-native-jwt-key"));
    assert!(!debug.contains("debug-google-client-id-must-be-redacted"));

    let db = common::TestDb::create("server_native_auth_bounds").await;
    let mut state = state(&db).await;
    state.auth = Arc::new(
        NativeAuthService::new(
            "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
            90,
        )
        .expect("production-safe defaults"),
    );
    let reset = raw_post(
        state.clone(),
        "/api/v1/auth/password-reset/request",
        serde_json::json!({"email":"phase0-fixture@example.test"}),
        None,
        None,
    )
    .await;
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = response_json(reset).await;
    assert!(reset.get("resetToken").is_none());

    let mut oversized = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/authenticate")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(vec![b'x'; 64 * 1024 + 1]))
        .expect("oversized request");
    oversized.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let oversized = melodee_server::native::configured_router(state.clone())
        .oneshot(oversized)
        .await
        .expect("oversized response");
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let (_, login) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
    )
    .await;
    let access = login["token"].as_str().expect("access token");
    for _ in 0..10 {
        let response = raw_authenticated(
            state.clone(),
            "POST",
            "/api/v1/user/me/link/google",
            serde_json::json!({"idToken":"disabled"}),
            access,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let limited = raw_authenticated(
        state,
        "POST",
        "/api/v1/user/me/link/google",
        serde_json::json!({"idToken":"disabled"}),
        access,
    )
    .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));
    db.drop().await;
}

#[tokio::test]
async fn google_oidc_uses_local_jwks_and_immutable_social_subject_mapping() {
    async fn jwks(
        axum::extract::State(requests): axum::extract::State<Arc<AtomicUsize>>,
    ) -> axum::Json<serde_json::Value> {
        let ordinal = requests.fetch_add(1, Ordering::SeqCst);
        let kid = if ordinal == 0 {
            "fixture-key"
        } else {
            "rotated-key"
        };
        axum::Json(serde_json::json!({"keys":[{
            "kty":"RSA","use":"sig","alg":"RS256","kid":kid,
            "n":GOOGLE_MODULUS,"e":"AQAB"
        }]}))
    }
    let jwks_requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("jwks listener");
    let address = listener.local_addr().expect("jwks address");
    let fixture_requests = jwks_requests.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/jwks", axum::routing::get(jwks))
                .with_state(fixture_requests),
        )
        .await
        .expect("jwks server");
    });

    let db = common::TestDb::create("server_native_google_oidc").await;
    let mut state = state(&db).await;
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM native_users LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .expect("user");
    AuthStore::new()
        .link_social_login(
            &db.pool,
            &SocialLoginLink {
                user_id,
                provider: "google",
                subject: "google-subject-fixture",
                verified_email: Some("phase0-fixture@example.test"),
                display_name: Some("Fixture"),
                hosted_domain: Some("example.test"),
                now: OffsetDateTime::now_utc(),
            },
        )
        .await
        .expect("social link");
    state.auth = Arc::new(
        NativeAuthService::new(
            "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
            90,
        )
        .expect("auth")
        .with_loopback_google_oidc_for_tests(
            &format!("http://{address}/jwks"),
            "test-client-id".to_string(),
            vec!["example.test".to_string()],
        )
        .expect("oidc"),
    );
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = serde_json::json!({
        "iss":"https://accounts.google.com","aud":"test-client-id",
        "sub":"google-subject-fixture","email":"phase0-fixture@example.test",
        "email_verified":true,"name":"Fixture","hd":"example.test",
        "iat":now,"nbf":now,"exp":now+300
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture-key".to_string());
    let token = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(GOOGLE_PRIVATE_KEY.as_bytes()).expect("private key"),
    )
    .expect("google token");
    let response = raw_post(
        state.clone(),
        "/api/v1/auth/google",
        serde_json::json!({"idToken":token,"deviceId":"fixture-device"}),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let authenticated = response_json(response).await;
    let access_token = authenticated["token"].as_str().expect("access token");

    let profile = raw_authenticated(
        state.clone(),
        "GET",
        "/api/v1/user/me",
        serde_json::Value::Null,
        access_token,
    )
    .await;
    assert_eq!(profile.status(), StatusCode::OK);
    let profile = response_json(profile).await;
    assert_eq!(profile["username"], "phase0-fixture");
    assert_eq!(profile["id"], "f78448cc-82ec-5864-877c-6c6163b00065");

    let providers = raw_authenticated(
        state.clone(),
        "GET",
        "/api/v1/user/me/linked-providers",
        serde_json::Value::Null,
        access_token,
    )
    .await;
    assert_eq!(providers.status(), StatusCode::OK);
    let providers = response_json(providers).await;
    assert_eq!(providers[0]["provider"], "Google");
    assert_eq!(providers[0]["email"], "phase0-fixture@example.test");

    let unlink = raw_authenticated(
        state.clone(),
        "DELETE",
        "/api/v1/user/me/link/google",
        serde_json::Value::Null,
        access_token,
    )
    .await;
    assert_eq!(unlink.status(), StatusCode::OK);
    assert_eq!(
        response_json(unlink).await["message"],
        "Google account unlinked successfully"
    );

    let missing = raw_authenticated(
        state.clone(),
        "DELETE",
        "/api/v1/user/me/link/google",
        serde_json::Value::Null,
        access_token,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(missing).await["message"],
        "Google account is not linked not found"
    );

    let link = raw_authenticated(
        state.clone(),
        "POST",
        "/api/v1/user/me/link/google",
        serde_json::json!({"idToken":token}),
        access_token,
    )
    .await;
    assert_eq!(link.status(), StatusCode::OK);
    let link = response_json(link).await;
    assert_eq!(link["message"], "Google account linked successfully");
    assert_eq!(link["provider"]["provider"], "Google");

    // A new kid triggers exactly one forced JWKS refresh and then remains
    // cached; this is the provider key-rotation path.
    header.kid = Some("rotated-key".to_string());
    let rotated = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(GOOGLE_PRIVATE_KEY.as_bytes()).expect("private key"),
    )
    .expect("rotated google token");
    let rotated_response = raw_post(
        state.clone(),
        "/api/v1/auth/cookie/google",
        serde_json::json!({"idToken":rotated}),
        None,
        None,
    )
    .await;
    assert_eq!(rotated_response.status(), StatusCode::OK);
    assert!(rotated_response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(jwks_requests.load(Ordering::SeqCst), 2);

    for (field, invalid) in [
        ("iss", serde_json::json!("https://issuer.invalid")),
        ("aud", serde_json::json!("wrong-client")),
        ("nbf", serde_json::json!(now + 600)),
        ("exp", serde_json::json!(now - 1)),
        ("email_verified", serde_json::json!(false)),
        ("hd", serde_json::json!("forbidden.example")),
    ] {
        let mut invalid_claims = claims.clone();
        invalid_claims[field] = invalid;
        let invalid_token = encode(
            &header,
            &invalid_claims,
            &EncodingKey::from_rsa_pem(GOOGLE_PRIVATE_KEY.as_bytes()).expect("private key"),
        )
        .expect("invalid token fixture");
        let invalid_response = raw_post(
            state.clone(),
            "/api/v1/auth/google",
            serde_json::json!({"idToken":invalid_token}),
            None,
            None,
        )
        .await;
        assert_eq!(
            invalid_response.status(),
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    assert_eq!(jwks_requests.load(Ordering::SeqCst), 2);
    db.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_unknown_google_key_ids_share_one_forced_refresh() {
    async fn jwks(
        axum::extract::State(requests): axum::extract::State<Arc<AtomicUsize>>,
    ) -> axum::Json<serde_json::Value> {
        requests.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        axum::Json(serde_json::json!({"keys":[{
            "kty":"RSA","use":"sig","alg":"RS256","kid":"fixture-key",
            "n":GOOGLE_MODULUS,"e":"AQAB"
        }]}))
    }

    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("JWKS listener");
    let address = listener.local_addr().expect("JWKS address");
    let fixture_requests = requests.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/jwks", axum::routing::get(jwks))
                .with_state(fixture_requests),
        )
        .await
        .expect("JWKS server");
    });

    let service = Arc::new(
        NativeAuthService::new(
            "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
            90,
        )
        .expect("auth")
        .with_loopback_google_oidc_for_tests(
            &format!("http://{address}/jwks"),
            "test-client-id".to_string(),
            Vec::new(),
        )
        .expect("OIDC"),
    );
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = serde_json::json!({
        "iss":"https://accounts.google.com","aud":"test-client-id",
        "sub":"unknown-key-subject","email":"unknown@example.test",
        "email_verified":true,"iat":now,"nbf":now,"exp":now+300
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("attacker-controlled-unknown-key".to_string());
    let token = Arc::new(
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(GOOGLE_PRIVATE_KEY.as_bytes()).expect("private key"),
        )
        .expect("unknown-key token"),
    );
    let barrier = Arc::new(Barrier::new(16));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let service = service.clone();
        let barrier = barrier.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            service.validate_google_token_for_tests(&token).await
        }));
    }
    for task in tasks {
        assert!(!task.await.expect("OIDC validation task"));
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "one cold fetch plus one forced refresh must serve the entire burst"
    );
}

#[tokio::test]
async fn google_authentication_is_disabled_by_default_for_jwt_and_cookie_flows() {
    let db = common::TestDb::create("server_native_google_disabled").await;
    let state = state(&db).await;

    for path in ["/api/v1/auth/google", "/api/v1/auth/cookie/google"] {
        let (status, body) = request(
            state.clone(),
            path,
            serde_json::json!({"idToken":"not-a-token"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "BAD_REQUEST");
        assert_eq!(body["message"], "Google authentication is not enabled");
    }
}

#[tokio::test]
async fn google_jwks_rejects_redirects_and_oversized_responses() {
    async fn redirect() -> axum::response::Response {
        (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/jwks")],
        )
            .into_response()
    }
    async fn oversized() -> Vec<u8> {
        vec![b'x'; 256 * 1024 + 1]
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/redirect", axum::routing::get(redirect))
                .route("/oversized", axum::routing::get(oversized)),
        )
        .await
        .expect("fixture server");
    });
    let db = common::TestDb::create("server_native_google_jwks_bounds").await;
    let base = state(&db).await;
    let private =
        EncodingKey::from_rsa_pem(include_str!("fixtures/google-test-private.pem").as_bytes())
            .expect("private key");
    let mut jwt_header = Header::new(Algorithm::RS256);
    jwt_header.kid = Some("fixture-key".to_string());
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let token = encode(
        &jwt_header,
        &serde_json::json!({
            "iss":"https://accounts.google.com","aud":"test-client-id","sub":"subject",
            "email":"fixture@example.test","email_verified":true,"iat":now,"nbf":now,
            "exp":now+300
        }),
        &private,
    )
    .expect("token");
    for endpoint in ["redirect", "oversized"] {
        let mut state = base.clone();
        state.auth = Arc::new(
            NativeAuthService::new(
                "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
                90,
            )
            .expect("auth")
            .with_loopback_google_oidc_for_tests(
                &format!("http://{address}/{endpoint}"),
                "test-client-id".to_string(),
                Vec::new(),
            )
            .expect("oidc"),
        );
        let response = raw_post(
            state,
            "/api/v1/auth/google",
            serde_json::json!({"idToken":token}),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{endpoint}");
    }
    assert!(
        NativeAuthService::new(
            "test-native-jwt-key-that-is-at-least-32-bytes".to_string(),
            90,
        )
        .expect("auth")
        .with_loopback_google_oidc_for_tests(
            "http://not-loopback.example/jwks",
            "client".to_string(),
            Vec::new(),
        )
        .is_err()
    );
    db.drop().await;
}

#[tokio::test]
async fn cors_allows_only_configured_origins_and_required_meloamp_headers() {
    let db = common::TestDb::create("server_native_cors").await;
    let state = state(&db).await;
    let app = melodee_server::native::configured_router(state).layer(
        melodee_server::native::auth::cors_from_origins("http://localhost:3001")
            .expect("strict cors"),
    );
    let mut request = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/auth/authenticate")
        .header(header::ORIGIN, "http://localhost:3001")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type,range",
        )
        .body(Body::empty())
        .expect("preflight");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let response = app.clone().oneshot(request).await.expect("preflight");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("http://localhost:3001"))
    );
    let allow_methods = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_METHODS)
        .and_then(|value| value.to_str().ok())
        .expect("allowed methods");
    assert!(allow_methods.contains("GET"));
    assert!(allow_methods.contains("POST"));
    assert!(allow_methods.contains("PATCH"));
    let allow_headers = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .and_then(|value| value.to_str().ok())
        .expect("allowed headers");
    assert!(allow_headers.contains("authorization"));
    assert!(allow_headers.contains("range"));

    let mut exposed = Request::builder()
        .uri("/api/v1/system/info")
        .header(header::ORIGIN, "http://localhost:3001")
        .body(Body::empty())
        .expect("actual CORS request");
    exposed.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:45678".parse::<SocketAddr>().expect("peer"),
    ));
    let exposed = app.clone().oneshot(exposed).await.expect("CORS response");
    let expose_headers = exposed
        .headers()
        .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .and_then(|value| value.to_str().ok())
        .expect("exposed response headers");
    assert!(expose_headers.contains("content-range"));
    assert!(expose_headers.contains("x-total-count"));

    let denied = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/auth/authenticate")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .body(Body::empty())
        .expect("denied preflight");
    let denied = app.oneshot(denied).await.expect("denied response");
    assert!(
        denied
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
    db.drop().await;
}

#[tokio::test]
async fn login_rotation_replay_and_logout_match_the_captured_lifecycle() {
    let db = common::TestDb::create("server_native_auth_lifecycle").await;
    let state = state(&db).await;
    let (status, login) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let access = login["token"].as_str().expect("access token");
    let refresh = login["refreshToken"].as_str().expect("refresh token");
    assert_eq!(refresh.len(), 88);
    assert_eq!(login["user"]["id"], "f78448cc-82ec-5864-877c-6c6163b00065");

    let (rotate_status, rotated) = request(
        state.clone(),
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":refresh,"deviceId":"phase0-fixture-device"}),
        None,
    )
    .await;
    assert_eq!(rotate_status, StatusCode::OK);
    let successor = rotated["refreshToken"].as_str().expect("successor");

    let (replay_status, replay) = request(
        state.clone(),
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":refresh,"deviceId":"phase0-fixture-device"}),
        None,
    )
    .await;
    assert_eq!(replay_status, StatusCode::UNAUTHORIZED);
    assert_eq!(replay["code"], "refresh_token_replayed");
    let (cascade_status, cascade) = request(
        state.clone(),
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":successor,"deviceId":"phase0-fixture-device"}),
        None,
    )
    .await;
    assert_eq!(cascade_status, StatusCode::UNAUTHORIZED);
    assert_eq!(cascade["code"], "refresh_token_replayed");

    let (relogin_status, relogin) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
    )
    .await;
    assert_eq!(relogin_status, StatusCode::OK);
    let access_after_relogin = relogin["token"].as_str().expect("access");
    let refresh_after_relogin = relogin["refreshToken"].as_str().expect("refresh");
    let (logout_status, _) = request(
        state.clone(),
        "/api/v1/auth/logout",
        serde_json::json!({}),
        Some(access_after_relogin),
    )
    .await;
    assert_eq!(logout_status, StatusCode::OK);

    // Captured access JWTs remain valid after logout; no refresh/session table
    // participates in middleware validation.
    let (legacy_refresh_status, _) = request(
        state.clone(),
        "/api/v1/auth/refresh",
        serde_json::json!({}),
        Some(access_after_relogin),
    )
    .await;
    assert_eq!(legacy_refresh_status, StatusCode::OK);
    let (after_logout_status, after_logout) = request(
        state.clone(),
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":refresh_after_relogin}),
        None,
    )
    .await;
    assert_eq!(after_logout_status, StatusCode::UNAUTHORIZED);
    assert_eq!(after_logout["code"], "refresh_token_replayed");

    let (final_login_status, final_login) = request(
        state.clone(),
        "/api/v1/auth/authenticate",
        serde_json::json!({"userName":"phase0-fixture","password":"Mel0deeR0cks!"}),
        None,
    )
    .await;
    assert_eq!(final_login_status, StatusCode::OK);
    let final_access = final_login["token"].as_str().expect("access");
    let final_refresh = final_login["refreshToken"].as_str().expect("refresh");
    let (revoke_status, revoke) = request(
        state.clone(),
        "/api/v1/auth/revoke",
        serde_json::json!({"refreshToken":final_refresh}),
        Some(final_access),
    )
    .await;
    assert_eq!(revoke_status, StatusCode::OK);
    assert_eq!(revoke["message"], "Token revoked successfully");
    let (revoked_status, revoked) = request(
        state,
        "/api/v1/auth/refresh-token",
        serde_json::json!({"refreshToken":final_refresh}),
        None,
    )
    .await;
    assert_eq!(revoked_status, StatusCode::UNAUTHORIZED);
    assert_eq!(revoked["code"], "refresh_token_replayed");
    assert!(!access.is_empty());
    db.drop().await;
}
