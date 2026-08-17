//! Jellyfin core compatibility evidence (Phase 8 WP4): routing middleware,
//! AuthenticateByName, token transports, logout revocation, discovery/system.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use futures_util::future::join_all;
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::jellyfin::{JfState, SecretPepper};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const PEPPER: &[u8] = b"melodee-jellyfin-fixture-pepper-NOT-A-REAL-SECRET-32b!";

type HmacSha256 = Hmac<sha2::Sha256>;

async fn state(db: &common::TestDb) -> JfState {
    let now = OffsetDateTime::now_utc();
    let roles = vec!["Administrator".to_string(), "HasStreamRole".to_string()];
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").expect("id"),
                username: "phase0-fixture",
                normalized_username: "PHASE0-FIXTURE",
                email: "phase0-fixture@example.test",
                normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("hash"),
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
    JfState {
        pool: db.pool.clone(),
        server_name: "Melodee".to_string(),
        server_version: "0.0.0-test".to_string(),
        server_id: "93cd3cc3a120127ccfda124620290cae".to_string(),
        local_address: "http://melodee.test".to_string(),
        public_origin: "http://melodee.test".to_string(),
        token_pepper: SecretPepper::new(PEPPER.to_vec()).expect("valid fixture pepper"),
        token_expiry_hours: 168,
        max_active_tokens: 10,
        direct_play: None,
        transcoding: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
        libraries: Vec::new(),
    }
}

async fn respond(state: JfState, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = melodee_server::jellyfin::configured_router(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn json_body(bytes: &[u8]) -> serde_json::Value {
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(bytes).expect("json")
    }
}

fn post_json(path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn get_with_token(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("X-Emby-Token", token)
        .body(Body::empty())
        .expect("request")
}

async fn fixture_user_id(db: &common::TestDb) -> Uuid {
    sqlx::query_scalar("SELECT id FROM native_users WHERE normalized_username = 'PHASE0-FIXTURE'")
        .fetch_one(&db.pool)
        .await
        .expect("fixture user id")
}

async fn insert_token(db: &common::TestDb, user_id: Uuid, token: &str, salt: [u8; 16]) {
    let mut mac = HmacSha256::new_from_slice(PEPPER).expect("fixture HMAC key");
    mac.update(&salt);
    mac.update(token.as_bytes());
    let digest = mac.finalize().into_bytes().to_vec();
    let issued_at = OffsetDateTime::now_utc();
    let expires_at = issued_at + time::Duration::hours(1);
    sqlx::query("INSERT INTO jellyfin_access_tokens (id, user_id, token_prefix, token_digest, token_salt, issued_at, expires_at, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $6, $6)")
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(&token[..8])
        .bind(digest)
        .bind(salt.as_slice())
        .bind(issued_at)
        .bind(expires_at)
        .execute(&db.pool)
        .await
        .expect("insert token");
}

#[tokio::test]
async fn authenticate_by_name_returns_user_session_and_access_token() {
    let db = common::TestDb::create("jf_auth_login").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (status, bytes) = respond(state, post_json("/Users/AuthenticateByName", body)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["User"]["Name"], "phase0-fixture");
    assert_eq!(json["User"]["Id"], "f78448cc82ec5864877c6c6163b00065");
    assert_eq!(json["User"]["Policy"]["IsAdministrator"], true);
    assert_eq!(json["ServerId"], "93cd3cc3a120127ccfda124620290cae");
    let token = json["AccessToken"].as_str().expect("access token");
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn authenticate_by_name_password_field_not_read_returns_400() {
    let db = common::TestDb::create("jf_auth_password_field").await;
    let state = state(&db).await;
    // A body with `Password` and no `Pw` -> captured 400 (the DTO reads ONLY Pw).
    // The frozen payload is the ASP.NET problem-details shape
    // (`jf-auth-negative/password-field-not-read`).
    let body = r#"{"Username":"phase0-fixture","Password":"Mel0deeR0cks!"}"#;
    let (status, bytes) = respond(state, post_json("/Users/AuthenticateByName", body)).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["title"], "One or more validation errors occurred.");
    assert_eq!(json["status"], 400);
    assert!(
        json["errors"]["$"][0]
            .as_str()
            .unwrap_or_default()
            .contains("missing required properties including: 'Pw'")
    );
}

#[tokio::test]
async fn authenticate_by_name_wrong_password_returns_401() {
    let db = common::TestDb::create("jf_auth_wrong_pw").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"wrong"}"#;
    let (status, bytes) = respond(state, post_json("/Users/AuthenticateByName", body)).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["detail"], "Invalid username or password.");
}

#[tokio::test]
async fn token_transports_and_logout_revocation() {
    let db = common::TestDb::create("jf_token_transports").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
    let token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();

    // X-Emby-Token transport
    let (status, _) = respond(state.clone(), get_with_token("/Users/Me", &token)).await;
    assert_eq!(status, StatusCode::OK);

    // Authorization MediaBrowser transport
    let request = Request::builder()
        .method("GET")
        .uri("/Users/Me")
        .header(
            header::AUTHORIZATION,
            format!("MediaBrowser Token=\"{token}\""),
        )
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Name"], "phase0-fixture");

    // api_key query transport
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Users/Me?api_key={token}"))
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Name"], "phase0-fixture");

    // Logout revokes the token (204, idempotent)
    let logout = Request::builder()
        .method("POST")
        .uri("/Sessions/Logout")
        .header("X-Emby-Token", &token)
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), logout).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(bytes.is_empty());

    // After logout the token is invalid (401, indistinguishable from garbage).
    let (status, _) = respond(state.clone(), get_with_token("/Users/Me", &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Logout with a garbage token is still 204 (idempotent).
    let logout = Request::builder()
        .method("POST")
        .uri("/Sessions/Logout")
        .header("X-Emby-Token", "garbage")
        .body(Body::empty())
        .expect("request");
    let (status, _) = respond(state.clone(), logout).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn prefix_collisions_authenticate_independently_and_logout_revokes_only_presented_token() {
    let db = common::TestDb::create("jf_token_prefix_collision").await;
    let state = state(&db).await;
    let user_id = fixture_user_id(&db).await;
    let first = format!("deadbeef{}", "1".repeat(56));
    let second = format!("deadbeef{}", "2".repeat(56));
    insert_token(&db, user_id, &first, [1; 16]).await;
    insert_token(&db, user_id, &second, [2; 16]).await;

    for token in [&first, &second] {
        let (status, _) = respond(state.clone(), get_with_token("/Users/Me", token)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "valid colliding token must authenticate"
        );
    }

    let logout = Request::builder()
        .method("POST")
        .uri("/Sessions/Logout")
        .header("X-Emby-Token", &first)
        .body(Body::empty())
        .expect("logout request");
    let (status, _) = respond(state.clone(), logout).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = respond(state.clone(), get_with_token("/Users/Me", &first)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = respond(state, get_with_token("/Users/Me", &second)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "logout must not revoke a sibling token"
    );
}

#[tokio::test]
async fn issued_token_stops_authorizing_when_account_becomes_ineligible() {
    let db = common::TestDb::create("jf_token_account_state").await;
    let state = state(&db).await;
    let user_id = fixture_user_id(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;

    for update in [
        "UPDATE native_users SET enabled = FALSE WHERE id = $1",
        "UPDATE native_users SET locked = TRUE WHERE id = $1",
        "UPDATE native_users SET blacklisted = TRUE WHERE id = $1",
    ] {
        let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
        let token = json_body(&bytes)["AccessToken"]
            .as_str()
            .expect("issued token")
            .to_string();
        sqlx::query(update)
            .bind(user_id)
            .execute(&db.pool)
            .await
            .expect("make account ineligible");
        let (status, _) = respond(state.clone(), get_with_token("/Users/Me", &token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        sqlx::query("UPDATE native_users SET enabled = TRUE, locked = FALSE, blacklisted = FALSE WHERE id = $1")
            .bind(user_id)
            .execute(&db.pool)
            .await
            .expect("restore fixture account");
    }
}

#[tokio::test]
async fn concurrent_logins_never_exceed_the_configured_active_token_cap() {
    let db = common::TestDb::create("jf_token_cap_concurrent").await;
    let state = state(&db).await;
    let requests = (0..16).map(|_| {
        respond(
            state.clone(),
            post_json(
                "/Users/AuthenticateByName",
                r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#,
            ),
        )
    });
    for (status, _) in join_all(requests).await {
        assert_eq!(status, StatusCode::OK);
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jellyfin_access_tokens WHERE revoked_at IS NULL AND expires_at > now()",
    )
    .fetch_one(&db.pool)
    .await
    .expect("active token count");
    assert_eq!(active, 10);
}

#[tokio::test]
async fn users_me_without_token_returns_401() {
    let db = common::TestDb::create("jf_me_no_token").await;
    let state = state(&db).await;
    let request = Request::builder()
        .method("GET")
        .uri("/Users/Me")
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state, request).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["detail"], "Missing or invalid authentication token.");
}

#[tokio::test]
async fn routing_middleware_rewrites_marker_requests_to_canonical_paths() {
    let db = common::TestDb::create("jf_routing").await;
    let state = state(&db).await;
    // Allowlisted pre-auth path: /System/Info/Public rewrites without markers.
    let request = Request::builder()
        .method("GET")
        .uri("/System/Info/Public")
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), request).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ServerName"], "Melodee");
    assert_eq!(json["Id"], "93cd3cc3a120127ccfda124620290cae");

    // System/Ping also allowlisted.
    let request = Request::builder()
        .method("GET")
        .uri("/System/Ping")
        .body(Body::empty())
        .expect("request");
    let (status, _) = respond(state.clone(), request).await;
    // Captured: System/Ping returns 204 No Content.
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Non-allowlisted path without a marker falls through (not rewritten -> 404
    // from the outer server's fallback, which axum returns as 404).
    let request = Request::builder()
        .method("GET")
        .uri("/System/Info")
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), request).await;
    // /System/Info is a registered Jellyfin route (accessible without a
    // marker; the handler serves the response).
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["ServerName"], "Melodee");

    // With a marker header, /System/Info also serves (marker is not required
    // for registered routes; it's only needed for the server root /).
    let request = Request::builder()
        .method("GET")
        .uri("/System/Info")
        .header("X-Emby-Token", "any-marker-value")
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state.clone(), request).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ServerName"], "Melodee");
}

#[tokio::test]
async fn users_public_returns_empty_public_user_list() {
    let db = common::TestDb::create("jf_users_public").await;
    let state = state(&db).await;
    let request = Request::builder()
        .method("GET")
        .uri("/Users/Public")
        .body(Body::empty())
        .expect("request");
    let (status, bytes) = respond(state, request).await;
    let json = json_body(&bytes);
    assert_eq!(status, StatusCode::OK);
    // Captured: no local users are public (empty array).
    let users = json.as_array().expect("array");
    assert!(users.is_empty());
}

#[tokio::test]
async fn canonical_root_probe_returns_empty_200() {
    let db = common::TestDb::create("jf_root_probe").await;
    let state = state(&db).await;
    for path in ["/api/jf", "/api/jf/"] {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .expect("request");
        let (status, bytes) = respond(state.clone(), request).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(bytes.is_empty(), "{path}");
    }
}

#[tokio::test]
async fn system_ping_post_is_204() {
    let db = common::TestDb::create("jf_ping_post").await;
    let state = state(&db).await;
    let request = Request::builder()
        .method("POST")
        .uri("/System/Ping")
        .body(Body::empty())
        .expect("request");
    let (status, _) = respond(state, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn authenticate_by_name_unknown_user_returns_401() {
    let db = common::TestDb::create("jf_auth_unknown_user").await;
    let state = state(&db).await;
    let body = r#"{"Username":"nobody","Pw":"whatever"}"#;
    let (status, bytes) = respond(state, post_json("/Users/AuthenticateByName", body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(&bytes)["detail"], "Invalid username or password.");
}

#[tokio::test]
async fn users_list_and_by_id_scoping() {
    let db = common::TestDb::create("jf_users_scoping").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
    let token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();
    let self_id = "f78448cc82ec5864877c6c6163b00065";

    // Admin list contains the fixture user.
    let (status, bytes) = respond(state.clone(), get_with_token("/Users", &token)).await;
    assert_eq!(status, StatusCode::OK);
    let users = json_body(&bytes);
    let users = users.as_array().expect("users array");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["Id"], self_id);
    assert!(
        users[0]["Policy"]["IsAdministrator"]
            .as_bool()
            .expect("bool")
    );

    // By-id: self fetch works.
    let (status, bytes) = respond(
        state.clone(),
        get_with_token(&format!("/Users/{self_id}"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Name"], "phase0-fixture");

    // Unknown user -> captured 404.
    let (status, bytes) = respond(
        state.clone(),
        get_with_token(&format!("/Users/{}", Uuid::now_v7().simple()), &token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "User not found.");

    // Invalid id -> captured 400.
    let (status, bytes) = respond(state.clone(), get_with_token("/Users/junk", &token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Invalid user ID format.");

    // A non-admin cannot fetch other users (captured 403) and lists only self.
    let now = OffsetDateTime::now_utc();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "jf-plain",
                normalized_username: "JF-PLAIN",
                email: "jf-plain@example.test",
                normalized_email: "JF-PLAIN@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasStreamRole".to_string()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import plain user");
    let body = r#"{"Username":"jf-plain","Pw":"Mel0deeR0cks!"}"#;
    let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
    let plain_token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();
    let (status, bytes) = respond(
        state.clone(),
        get_with_token(&format!("/Users/{self_id}"), &plain_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json_body(&bytes)["detail"], "Cannot access other users.");
    let (status, bytes) = respond(state.clone(), get_with_token("/Users", &plain_token)).await;
    assert_eq!(status, StatusCode::OK);
    let users = json_body(&bytes);
    assert_eq!(users.as_array().expect("array").len(), 1);
}

/// Wraps the adapter router with the outer case-rewriting fallback exactly
/// the way `main.rs` does, so case-insensitive routing is testable without
/// booting the full server.
async fn respond_with_case_fallback(
    state: JfState,
    request: Request<Body>,
) -> (StatusCode, Vec<u8>) {
    let app = melodee_server::jellyfin::configured_router(state);
    let replay = app.clone();
    let response = app
        .fallback_service(melodee_server::jellyfin::case_rewriting_fallback(replay))
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn lowercase_jellyfin_routes_match_case_insensitively() {
    let db = common::TestDb::create("jf_case_routing").await;
    let state = state(&db).await;
    // Legacy ASP.NET matched route templates case-insensitively; real clients
    // (Feishin) call lowercase paths (Phase 8 real-client smoke finding).
    let (status, _) =
        respond_with_case_fallback(state.clone(), get_with_token("/system/info/public", "")).await;
    assert_eq!(status, StatusCode::OK);
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (status, bytes) =
        respond_with_case_fallback(state.clone(), post_json("/users/authenticatebyname", body))
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json_body(&bytes)["AccessToken"].as_str().is_some());
    // Dynamic segment values keep their original casing/value.
    let token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();
    let (status, bytes) = respond_with_case_fallback(
        state.clone(),
        get_with_token("/users/f78448cc82ec5864877c6c6163b00065", &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Name"], "phase0-fixture");
    // Non-Jellyfin paths keep the captured empty 404.
    let (status, bytes) =
        respond_with_case_fallback(state, get_with_token("/nonsense/path", "")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn users_items_redirect_matches_query_keys_case_insensitively() {
    let db = common::TestDb::create("jf_redirect_case").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
    let token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();
    // Real clients (Finamp, Feishin) send PascalCase query keys; legacy
    // `[FromQuery]` binding matched them case-insensitively and preserved
    // them in the redirect reconstruction.
    let (status, headers) = {
        let request = Request::builder()
            .method("GET")
            .uri("/Users/f78448cc82ec5864877c6c6163b00065/Items?IncludeItemTypes=MusicAlbum&Recursive=true&SortBy=SortName")
            .header("X-Emby-Token", token.as_str())
            .body(Body::empty())
            .expect("request");
        let response = melodee_server::jellyfin::configured_router(state)
            .oneshot(request)
            .await
            .expect("response");
        (
            response.status(),
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
        )
    };
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        headers.as_deref(),
        Some(
            "/Items?includeItemTypes=MusicAlbum&recursive=True&sortBy=SortName&userId=f78448cc82ec5864877c6c6163b00065"
        )
    );
}

#[tokio::test]
async fn apikey_query_param_authenticates_alongside_api_key() {
    let db = common::TestDb::create("jf_apikey_query").await;
    let state = state(&db).await;
    let body = r#"{"Username":"phase0-fixture","Pw":"Mel0deeR0cks!"}"#;
    let (_, bytes) = respond(state.clone(), post_json("/Users/AuthenticateByName", body)).await;
    let token = json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("token")
        .to_string();
    // Real Jellyfin reads `ApiKey` as the primary query token; real clients
    // build media URLs with it (Feishin /Download stream URLs). /Users is a
    // captured token-gated route.
    let bare_get = |path: &str| {
        Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .expect("request")
    };
    for key in ["apiKey", "ApiKey", "api_key"] {
        let (status, _) = respond(state.clone(), bare_get(&format!("/Users?{key}={token}"))).await;
        assert_eq!(status, StatusCode::OK, "query key {key}");
    }
    // Garbage tokens still reject through the same params.
    let (status, _) = respond(state, bare_get("/Users?apiKey=deadbeef")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
