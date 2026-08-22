//! OpenSubsonic system/auth/negative compatibility evidence.
//! Bodies and headers asserted here mirror the frozen golden captures in
//! `compatibility/opensubsonic/golden/{system,auth,negative}` with only the
//! scenario-declared volatile fields differing.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::opensubsonic::{MusicFolder, OsState};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const FIXTURE_SECRET: &str = "fixture-os-secret-NOT-A-REAL-SECRET";
const SERVER_VERSION: &str = "0.0.0-test";
const ENVELOPE_JSON_PREFIX: &str = "{\"subsonic-response\":{\"status\":\"";
const XML_PING_OK: &str = "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"ok\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"0.0.0-test\" openSubsonic=\"true\"></subsonic-response>";

fn fixture_user_id() -> Uuid {
    Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").expect("fixture id")
}

fn music_folders() -> Vec<std::sync::Arc<MusicFolder>> {
    vec![
        std::sync::Arc::new(MusicFolder {
            id: Uuid::parse_str("c598e196-db6f-582e-b110-1726c138bd4f").expect("folder id"),
            name: "Phase0 Fixture Library".to_string(),
        }),
        std::sync::Arc::new(MusicFolder {
            id: Uuid::parse_str("f63a6428-55d5-847b-3d09-3fa3b69b66ae").expect("folder id"),
            name: "Storage".to_string(),
        }),
    ]
}

async fn state(db: &common::TestDb) -> OsState {
    let now = OffsetDateTime::now_utc();
    let roles = vec!["Administrator".to_string(), "HasStreamRole".to_string()];
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: fixture_user_id(),
                username: "phase0-fixture",
                normalized_username: "PHASE0-FIXTURE",
                email: "phase0-fixture@example.test",
                normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("fixture hash"),
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
    let internal_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM native_users WHERE normalized_username='PHASE0-FIXTURE'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("user id");
    sqlx::query("INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at) VALUES ($1, $2, now(), now())")
        .bind(internal_id)
        .bind(FIXTURE_SECRET)
        .execute(&db.pool)
        .await
        .expect("seed secret");
    OsState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        server_version: SERVER_VERSION.to_string(),
        public_origin: "http://melodee.test".to_string(),
        base_url_host: Some("capture-fixture.example.test".to_string()),
        music_folders: music_folders(),
        podcasts_enabled: true,
        downloads_enabled: true,
        jukebox_enabled: false,
        direct_play: None,
        transcoding: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        search: None,
    }
}

fn token_for(salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(FIXTURE_SECRET.as_bytes());
    hasher.update(salt.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn respond(
    state: OsState,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = melodee_server::opensubsonic::configured_router(state)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .expect("request")
}

fn enforced_get(path: &str) -> Request<Body> {
    // Mirrors the enforced-auth capture: a synthetic public client
    // address defeats the localhost bypass without touching the server.
    let mut request = get(path);
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
    request
}

fn json_body(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("json body")
}

#[tokio::test]
async fn ping_matches_the_captured_envelope_in_all_formats() {
    let db = common::TestDb::create("os_ping_formats").await;
    let state = state(&db).await;

    let (status, headers, body) =
        respond(state.clone(), get("/rest/ping.view?f=json&v=1.16.1&c=t")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/json; charset=utf-8"
    );
    assert_eq!(headers.get("x-total-count").unwrap(), "0");
    assert_eq!(
        json_body(&body),
        serde_json::json!({
            "subsonic-response": {
                "status": "ok",
                "version": "1.16.1",
                "type": "Melodee",
                "serverVersion": SERVER_VERSION,
                "openSubsonic": true
            }
        })
    );

    let (status, headers, body) = respond(state.clone(), get("/rest/ping.view?v=1.16.1&c=t")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    assert_eq!(String::from_utf8(body).unwrap(), XML_PING_OK);

    let (_, headers, body) = respond(
        state.clone(),
        get("/rest/ping.view?f=jsonp&callback=phase0Callback&v=1.16.1&c=t"),
    )
    .await;
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/javascript; charset=utf-8"
    );
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "phase0Callback({\"subsonic-response\":{\"status\":\"ok\",\"version\":\"1.16.1\",\"type\":\"Melodee\",\"serverVersion\":\"0.0.0-test\",\"openSubsonic\":true}})"
    );

    // f=jsonp without callback falls back to XML.
    let (_, headers, body) =
        respond(state.clone(), get("/rest/ping.view?f=jsonp&v=1.16.1&c=t")).await;
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/xml"
    );
    assert_eq!(String::from_utf8(body).unwrap(), XML_PING_OK);

    // Plain route and the server-root alias answer identically.
    for path in ["/rest/ping?f=json", "/ping?f=json"] {
        let (status, _, body) = respond(state.clone(), get(path)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(
            String::from_utf8(body)
                .unwrap()
                .starts_with(ENVELOPE_JSON_PREFIX),
            "{path}"
        );
    }
}

#[tokio::test]
async fn license_and_extensions_preserve_captured_quirks() {
    let db = common::TestDb::create("os_system_quirks").await;
    let state = state(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        get("/rest/getOpenSubsonicExtensions.view?f=json"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body)["subsonic-response"]["openSubsonicExtensions"],
        serde_json::json!([
            {"name": "melodeeExtensions", "versions": [1]},
            {"name": "apiKeyAuthentication", "versions": [1]},
            {"name": "formPost", "versions": [1]},
            {"name": "songLyrics", "versions": [1]},
            {"name": "transcodeOffset", "versions": [1]}
        ])
    );

    let (_, _, body) = respond(state.clone(), get("/rest/getOpenSubsonicExtensions.view")).await;
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"ok\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"0.0.0-test\" openSubsonic=\"true\"><openSubsonicExtensions>name=\"melodeeExtensions\" versions=\"1 \"/>name=\"apiKeyAuthentication\" versions=\"1 \"/>name=\"formPost\" versions=\"1 \"/>name=\"songLyrics\" versions=\"1 \"/>name=\"transcodeOffset\" versions=\"1 \"/></openSubsonicExtensions></subsonic-response>"
    );

    // getLicense: JSON keeps the `trailExpires` typo; XML keeps the nested
    // element with `trialExpires` and its irregular spacing.
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getLicense.view?f=json&u=phase0-fixture&t={}&s=abc",
            token_for("abc")
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let license = &json_body(&body)["subsonic-response"]["license"];
    assert_eq!(license["valid"], true);
    assert!(license.get("trailExpires").is_some());
    assert!(license.get("trialExpires").is_none());

    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getLicense.view?u=phase0-fixture&t={}&s=abc",
            token_for("abc")
        )),
    )
    .await;
    let xml = String::from_utf8(body).unwrap();
    assert!(
        xml.contains("<license><license valid=\"true\" email="),
        "{xml}"
    );
    assert!(xml.contains(" trialExpires=\""), "{xml}");
    assert!(xml.contains("licenseExpires=\""), "{xml}");
    assert!(xml.contains("\"  trialExpires="), "{xml}");
}

#[tokio::test]
async fn music_folders_match_the_captured_shape() {
    let db = common::TestDb::create("os_music_folders").await;
    let state = state(&db).await;

    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getMusicFolders.view?f=json&u=phase0-fixture&t={}&s=xyz",
            token_for("xyz")
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body)["subsonic-response"]["musicFolders"],
        serde_json::json!({
            "musicFolder": [
                {"id": "library_c598e196-db6f-582e-b110-1726c138bd4f", "name": "Phase0 Fixture Library"},
                {"id": "library_f63a6428-55d5-847b-3d09-3fa3b69b66ae", "name": "Storage"}
            ]
        })
    );

    let (_, _, body) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getMusicFolders.view?u=phase0-fixture&t={}&s=xyz",
            token_for("xyz")
        )),
    )
    .await;
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "<subsonic-response xmlns=\"https://subsonic.org/restapi\" status=\"ok\" type=\"melodee\" version=\"1.16.1\" serverVersion=\"0.0.0-test\" openSubsonic=\"true\"><musicFolders><musicFolder id=\"library_c598e196-db6f-582e-b110-1726c138bd4f\" name=\"Phase0 Fixture Library\"/><musicFolder id=\"library_f63a6428-55d5-847b-3d09-3fa3b69b66ae\" name=\"Storage\"/></musicFolders></subsonic-response>"
    );
}

#[tokio::test]
async fn token_auth_accepts_case_insensitive_hex_and_form_post() {
    let db = common::TestDb::create("os_token_forms").await;
    let state = state(&db).await;

    let (status, _, _) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getMusicFolders.view?f=json&u=phase0-fixture&t={}&s=q",
            token_for("q").to_uppercase()
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let body = format!("u=phase0-fixture&t={}&s=form&f=json", token_for("form"));
    let mut request = Request::builder()
        .method("POST")
        .uri("/rest/getMusicFolders.view")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        9000,
    )));
    let (status, _, bytes) = respond(state.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["subsonic-response"]["status"], "ok");
}

#[tokio::test]
async fn password_auth_accepts_plain_and_enc_hex_forms() {
    let db = common::TestDb::create("os_password_forms").await;
    let state = state(&db).await;

    let (status, _, _) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getMusicFolders.view?f=json&u=phase0-fixture&p={FIXTURE_PASSWORD}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let enc: String = FIXTURE_PASSWORD
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (status, _, _) = respond(
        state.clone(),
        enforced_get(&format!(
            "/rest/getMusicFolders.view?f=json&u=phase0-fixture&p=enc:{enc}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn auth_failures_share_the_captured_error_40_envelope() {
    let db = common::TestDb::create("os_auth_failures").await;
    let state = state(&db).await;
    let cases = [
        "/rest/getMusicFolders.view?f=json",
        "/rest/getMusicFolders.view?f=json&u=nobody&t=abc&s=def",
        "/rest/getMusicFolders.view?f=json&u=phase0-fixture&t=00000000000000000000000000000000&s=def",
        "/rest/getMusicFolders.view?f=json&u=phase0-fixture&t=nothexatall!!!&s=def",
        "/rest/getMusicFolders.view?f=json&u=phase0-fixture&t=00000000000000000000000000000000",
        // apiKey only: advertised but never consulted (ADR-0130 decision 2).
        "/rest/getMusicFolders.view?f=json&apiKey=whatever",
        // jwt only: the guard requires u + (p or t); the branch is never
        // reached and, per ADR-0130 decision 1, no JWT branch exists at all.
        "/rest/getMusicFolders.view?f=json&jwt=aaa.bbb.ccc",
    ];
    for path in cases {
        let (status, _, body) = respond(state.clone(), enforced_get(path)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            json_body(&body),
            serde_json::json!({
                "subsonic-response": {
                    "status": "error",
                    "version": "1.16.1",
                    "type": "Melodee",
                    "serverVersion": SERVER_VERSION,
                    "openSubsonic": true,
                    "error": {"code": 40, "message": "Wrong username or password."}
                }
            }),
            "{path}"
        );
    }
}

#[tokio::test]
async fn forged_unsigned_jwt_with_wrong_password_is_rejected() {
    // ADR-0113 / ADR-0130 regression: the legacy server authenticated this
    // attack; the Rust adapter must not. The forged payload claims the
    // fixture user via sub/sid/name claims with no signature.
    let db = common::TestDb::create("os_jwt_rejection").await;
    let state = state(&db).await;
    let forged =
        "eyJhbGciOiJub25lIn0.eyJzdWIiOiJwaGFzZTAtZml4dHVyZSIsIm5hbWUiOiJwaGFzZTAtZml4dHVyZSJ9.";
    let path = format!(
        "/rest/getMusicFolders.view?f=json&u=phase0-fixture&p=definitely-wrong&jwt={forged}"
    );
    let (status, _, body) = respond(state.clone(), enforced_get(&path)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&body)["subsonic-response"]["error"]["code"], 40);
}

#[tokio::test]
async fn podcast_auth_failure_uses_http_403() {
    let db = common::TestDb::create("os_podcast_403").await;
    let state = state(&db).await;
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(
            "/rest/getPodcasts.view?f=json&u=phase0-fixture&t=00000000000000000000000000000000&s=x",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json_body(&body)["subsonic-response"]["error"]["code"], 40);
}

#[tokio::test]
async fn bypass_needs_no_credentials_and_synthesizes_no_user() {
    let db = common::TestDb::create("os_bypass").await;
    let state = state(&db).await;

    // Local requests (ConnectInfo loopback) bypass auth.
    let mut local = get("/rest/getMusicFolders.view?f=json");
    local.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        8000,
    )));
    let (status, _, body) = respond(state.clone(), local).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&body)["subsonic-response"]["status"], "ok");

    // Requests whose Host equals the configured base URL bypass auth.
    let mut host_match = enforced_get("/rest/getMusicFolders.view?f=json");
    host_match.headers_mut().insert(
        header::HOST,
        "capture-fixture.example.test".parse().expect("host"),
    );
    let (status, _, _) = respond(state.clone(), host_match).await;
    assert_eq!(status, StatusCode::OK);

    // Paths containing /images/ bypass auth. The route itself is owned by the
    // native surface (not mounted in this harness), so the request falls
    // through to 404 — the assertion that matters is that the OS auth
    // middleware does NOT challenge it (no 403/error-40). The native
    // unknown-id 200 empty class is covered by the native image tests.
    let (status, _, _) = respond(
        state.clone(),
        enforced_get("/images/song_00000000-0000-0000-0000-000000000000/80"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn podcasts_enabled_and_disabled_match_the_captured_contract() {
    let db = common::TestDb::create("os_podcasts_conditional").await;
    let state = state(&db).await;
    let auth = format!("u=phase0-fixture&t={}&s=p", token_for("p"));

    // Enabled: captured empty lists.
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&format!("/rest/getPodcasts.view?f=json&{auth}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body)["subsonic-response"]["podcasts"],
        serde_json::json!({"channel": []})
    );
    let (status, _, body) = respond(
        state.clone(),
        enforced_get(&format!("/rest/getNewestPodcasts.view?f=json&{auth}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_body(&body)["subsonic-response"]["newestPodcasts"],
        serde_json::json!({"episode": []})
    );

    // Disabled: HTTP 400 with the raw ResponseData body, NOT the subsonic
    // envelope, and no x-total-count header.
    let mut disabled = state.clone();
    disabled.podcasts_enabled = false;
    let (status, headers, body) = respond(
        disabled,
        enforced_get(&format!("/rest/getPodcasts.view?f=json&{auth}")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(headers.get("x-total-count").is_none());
    let parsed = json_body(&body);
    assert_eq!(parsed["isSuccess"], true);
    assert_eq!(parsed["subsonic-response"]["isSuccess"], false);
    assert_eq!(
        parsed["subsonic-response"]["error"]["message"],
        "Podcasts are currently disabled."
    );
    assert_eq!(parsed["totalCount"], 0);
}

#[tokio::test]
async fn negative_matrix_preserves_exact_statuses_and_headers() {
    let db = common::TestDb::create("os_negative_matrix").await;
    let state = state(&db).await;
    let auth = format!("u=phase0-fixture&t={}&s=n", token_for("n"));
    let gone = [
        "getChatMessages",
        "addChatMessage",
        "hls",
        "getCaptions",
        "getVideoInfo",
        "getVideos",
        "jukeboxControl",
    ];
    for name in gone {
        for suffix in ["", ".view"] {
            let (status, headers, body) = respond(
                state.clone(),
                enforced_get(&format!("/rest/{name}{suffix}?f=json&{auth}")),
            )
            .await;
            assert_eq!(status, StatusCode::GONE, "{name}{suffix}");
            assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
            assert!(body.is_empty(), "{name}{suffix}");
        }
    }
    let not_implemented = ["getUsers", "updateUser", "deleteUser", "changePassword"];
    for name in not_implemented {
        for suffix in ["", ".view"] {
            let (status, headers, body) = respond(
                state.clone(),
                enforced_get(&format!("/rest/{name}{suffix}?f=json&{auth}")),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{name}{suffix}");
            assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
            assert!(body.is_empty(), "{name}{suffix}");
        }
    }
}
