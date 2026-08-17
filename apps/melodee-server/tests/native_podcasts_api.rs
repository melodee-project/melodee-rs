//! Phase 7 native-v1 podcast route, tenancy, and bounded-input evidence.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::ConnectInfo,
    extract::Query,
    http::header,
    http::{Method, Request, StatusCode},
    routing::get,
};
use http_body_util::BodyExt;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport, StoredNativeUser};
use melodee_server::native::{
    NativeClientBinding, NativeFuture, NativePrincipal, NativeState, NativeStreamUrlSigner,
    PostgresNativeUserStateReader,
};
use time::OffsetDateTime;
use tokio::sync::{Mutex, Notify, oneshot};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug)]
struct Signer;
impl NativeStreamUrlSigner for Signer {
    fn stream_url(&self, _: Uuid, _: Uuid, _: &str) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Ok(String::new()) })
    }
}

async fn user(db: &common::TestDb, name: &str, role: bool) -> StoredNativeUser {
    let now = OffsetDateTime::now_utc();
    let roles = if role {
        vec!["HasPodcastRole".to_owned()]
    } else {
        vec!["user".to_owned()]
    };
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: name,
                normalized_username: &name.to_ascii_uppercase(),
                email: &format!("{name}@example.test"),
                normalized_email: &format!("{}@EXAMPLE.TEST", name.to_ascii_uppercase()),
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "podcast-test-key",
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
        .expect("user")
}
fn principal(u: &StoredNativeUser) -> NativePrincipal {
    NativePrincipal {
        user_id: u.id,
        public_api_id: u.public_api_id,
        is_admin: u.is_admin,
        is_editor: u.is_editor,
    }
}
fn state(db: &common::TestDb, discovery_endpoint: &str) -> NativeState {
    let podcast_config = melodee_server::podcast_worker::PodcastConfig {
        allow_loopback_http: true,
        ..melodee_server::podcast_worker::PodcastConfig::default()
    };
    NativeState {
        lastfm_api_base_url: None,
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: "http://melodee.test".to_owned(),
        podcast_config: Arc::new(podcast_config),
        podcast_discovery:
            melodee_server::native::podcasts::PodcastDiscoveryClient::new_for_loopback_fixture(
                discovery_endpoint,
            )
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(Signer),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: None,
        client_addresses: melodee_server::native_stream::ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "test-only-jwt-signing-key-that-is-long".to_owned(),
                90,
            )
            .expect("auth"),
        ),
    }
}

async fn discovery_fixture() -> String {
    async fn search(
        Query(params): Query<std::collections::HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        let term = params.get("term").cloned().unwrap_or_default();
        Json(serde_json::json!({
            "resultCount": 1,
            "results": [{
                "trackName": term,
                "artistName": "Fixture Artist",
                "feedUrl": "https://example.test/feed.xml",
                "artworkUrl600": "https://example.test/art.png",
                "primaryGenreName": "Technology",
                "trackCount": 42,
                "releaseDate": "2026-08-12T00:00:00Z",
                "trackViewUrl": "https://example.test/show",
                "collectionId": 12345
            }]
        }))
    }
    async fn lookup(
        Query(params): Query<std::collections::HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        if params.get("id").is_some_and(|id| id == "0") {
            Json(serde_json::json!({"resultCount":0,"results":[]}))
        } else {
            Json(serde_json::json!({
                "resultCount":1,
                "results":[{
                    "trackName":"Fixture Podcast",
                    "collectionId":12345
                }]
            }))
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let feed = format!(
        "<?xml version=\"1.0\"?><rss><channel><title>Fixture Channel</title>\
         <description>Fixture description</description><item><guid>fixture-from-feed</guid>\
         <title>Fetched Episode</title><description>Fetched details</description>\
         <enclosure url=\"http://{address}/episode.mp3\"/></item></channel></rss>"
    );
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/search", get(search))
                .route("/lookup", get(lookup))
                .route(
                    "/feed.xml",
                    get(move || {
                        let feed = feed.clone();
                        async move { ([(header::CONTENT_TYPE, "application/rss+xml")], feed) }
                    }),
                )
                .route(
                    "/episode.mp3",
                    get(|| async { b"fixture podcast media".to_vec() }),
                ),
        )
        .await
        .expect("fixture server");
    });
    format!("http://{address}/")
}

async fn blocked_media_fixture() -> (String, oneshot::Receiver<()>, Arc<Notify>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("blocked fixture listener");
    let address = listener.local_addr().expect("blocked fixture address");
    let (arrival_sender, arrival_receiver) = oneshot::channel();
    let arrival_sender = Arc::new(Mutex::new(Some(arrival_sender)));
    let release = Arc::new(Notify::new());
    let route_sender = Arc::clone(&arrival_sender);
    let route_release = Arc::clone(&release);
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/blocked.mp3",
                get(move || {
                    let sender = Arc::clone(&route_sender);
                    let release = Arc::clone(&route_release);
                    async move {
                        if let Some(sender) = sender.lock().await.take() {
                            let _ = sender.send(());
                            release.notified().await;
                        }
                        b"lease-fenced podcast media".to_vec()
                    }
                }),
            ),
        )
        .await
        .expect("blocked fixture server");
    });
    (
        format!("http://{address}/blocked.mp3"),
        arrival_receiver,
        release,
    )
}

async fn call(
    state: NativeState,
    p: Option<NativePrincipal>,
    method: Method,
    uri: &str,
    body: Vec<u8>,
    content_type: &str,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", content_type)
        .body(Body::from(body))
        .expect("request");
    if let Some(p) = p {
        req.extensions_mut().insert(p);
        req.extensions_mut()
            .insert(NativeClientBinding("127.0.0.1".to_owned()));
    }
    let res = melodee_server::native::router()
        .with_state(state)
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = res
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn configured_router_exposes_podcasts_only_after_real_bearer_validation() {
    let db = common::TestDb::create("server_native_podcast_composition").await;
    let allowed = user(&db, "podcast-bearer", true).await;
    let discovery_endpoint = discovery_fixture().await;
    let state = state(&db, &discovery_endpoint);
    let password = "password";
    let hash = bcrypt::hash(password, 4).expect("hash");
    sqlx::query("UPDATE native_users SET password_hash=$2 WHERE id=$1")
        .bind(allowed.id)
        .bind(hash)
        .execute(&db.pool)
        .await
        .expect("password");
    let mut login = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/authenticate")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json(serde_json::json!({
            "userName":"podcast-bearer",
            "password":password
        }))))
        .expect("login");
    login.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41001".parse::<SocketAddr>().expect("peer"),
    ));
    let response = melodee_server::native::configured_router(state.clone())
        .oneshot(login)
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("login body")
        .to_bytes();
    let token = serde_json::from_slice::<serde_json::Value>(&body).expect("login json")["token"]
        .as_str()
        .expect("access token")
        .to_owned();
    let mut request = Request::builder()
        .uri("/api/v1/podcasts/channels?page=1&pageSize=50")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41001".parse::<SocketAddr>().expect("peer"),
    ));
    let response = melodee_server::native::configured_router(state)
        .oneshot(request)
        .await
        .expect("podcast response");
    assert_eq!(response.status(), StatusCode::OK);
    db.drop().await;
}
fn json(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).expect("json")
}

#[tokio::test]
async fn all_native_podcast_routes_are_tenant_scoped_bounded_and_authorized() {
    let db = common::TestDb::create("server_native_podcasts").await;
    let allowed = user(&db, "podcast-allowed", true).await;
    let denied = user(&db, "podcast-denied", false).await;
    let p = principal(&allowed);
    let discovery_endpoint = discovery_fixture().await;
    let state = state(&db, &discovery_endpoint);
    let empty_channels = call(
        state.clone(),
        Some(p),
        Method::GET,
        "/api/v1/podcasts/channels?page=1&pageSize=50",
        vec![],
        "application/json",
    )
    .await;
    assert_eq!(empty_channels.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&empty_channels.1).expect("channel envelope"),
        serde_json::json!({
            "currentPage":0,
            "totalCount":0,
            "totalPages":0,
            "additionalClientData":null,
            "errors":null,
            "data":[],
            "isSuccess":true,
            "type":200,
            "messages":null,
            "operationTime":null
        })
    );
    let captured_missing = [
        (
            Method::POST,
            "/api/v1/podcasts/channels",
            json(serde_json::json!({"url":"not-a-feed-url"})),
            StatusCode::BAD_REQUEST,
            "Invalid URL format",
        ),
        (
            Method::PATCH,
            "/api/v1/podcasts/channels/999999",
            json(serde_json::json!({"autoDownloadEnabled":true})),
            StatusCode::NOT_FOUND,
            "Channel not found",
        ),
        (
            Method::DELETE,
            "/api/v1/podcasts/channels/999999?softDelete=true",
            vec![],
            StatusCode::BAD_REQUEST,
            "Channel not found",
        ),
        (
            Method::GET,
            "/api/v1/podcasts/channels/999999/episodes?page=1&pageSize=50",
            vec![],
            StatusCode::NOT_FOUND,
            "Channel not found",
        ),
        (
            Method::POST,
            "/api/v1/podcasts/channels/999999/refresh",
            vec![],
            StatusCode::NOT_FOUND,
            "Channel not found",
        ),
        (
            Method::DELETE,
            "/api/v1/podcasts/episodes/999999",
            vec![],
            StatusCode::BAD_REQUEST,
            "Episode not found",
        ),
        (
            Method::POST,
            "/api/v1/podcasts/episodes/999999/download",
            vec![],
            StatusCode::BAD_REQUEST,
            "Episode not found",
        ),
        (
            Method::POST,
            "/api/v1/podcasts/episodes/999999/play?secondsPlayed=10",
            vec![],
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred",
        ),
        (
            Method::GET,
            "/api/v1/podcasts/episodes/999999/bookmark",
            vec![],
            StatusCode::BAD_REQUEST,
            "Error retrieving bookmark",
        ),
        (
            Method::PUT,
            "/api/v1/podcasts/episodes/999999/bookmark",
            json(serde_json::json!({"positionSeconds":10,"comment":"resume"})),
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred",
        ),
        (
            Method::DELETE,
            "/api/v1/podcasts/episodes/999999/bookmark",
            vec![],
            StatusCode::BAD_REQUEST,
            "Bookmark not found",
        ),
        (
            Method::POST,
            "/api/v1/podcasts/opml/import",
            b"<opml version=\"2.0\"><body></body></opml>".to_vec(),
            StatusCode::BAD_REQUEST,
            "No podcast feeds found in OPML file",
        ),
    ];
    for (method, uri, body, expected_status, expected_message) in captured_missing {
        let response = call(
            state.clone(),
            Some(p),
            method,
            uri,
            body,
            if uri.ends_with("/import") {
                "application/xml"
            } else {
                "application/json"
            },
        )
        .await;
        assert_eq!(response.0, expected_status, "{uri}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.1).expect("captured error")["message"],
            expected_message,
            "{uri}"
        );
    }
    // The OPML import route carries an explicit body limit at the documented
    // 2 MiB contract bound: an oversized body is rejected by the limiter
    // (413), never buffered for the handler.
    let oversized_opml = vec![b' '; 2 * 1024 * 1024 + 1];
    let oversized = call(
        state.clone(),
        Some(p),
        Method::POST,
        "/api/v1/podcasts/opml/import",
        oversized_opml,
        "application/xml",
    )
    .await;
    assert_eq!(
        oversized.0,
        StatusCode::PAYLOAD_TOO_LARGE,
        "opml import enforces its explicit 2 MiB body limit"
    );
    let empty_episodes = call(
        state.clone(),
        Some(p),
        Method::GET,
        "/api/v1/podcasts/episodes/search?query=phase7-no-match&page=1&pageSize=50",
        vec![],
        "application/json",
    )
    .await;
    assert_eq!(empty_episodes.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&empty_episodes.1).expect("episode envelope"),
        serde_json::json!({
            "currentPage":0,
            "totalCount":0,
            "totalPages":0,
            "additionalClientData":null,
            "errors":null,
            "data":[],
            "isSuccess":true,
            "type":200,
            "messages":null,
            "operationTime":null
        })
    );
    let missing_history = call(
        state.clone(),
        Some(p),
        Method::GET,
        "/api/v1/podcasts/episodes/999999/history?limit=50&offset=0",
        vec![],
        "application/json",
    )
    .await;
    assert_eq!(missing_history.0, StatusCode::OK);
    assert_eq!(missing_history.1, b"[]");
    assert_eq!(
        call(
            state.clone(),
            None,
            Method::GET,
            "/api/v1/podcasts/channels",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            state.clone(),
            Some(principal(&denied)),
            Method::GET,
            "/api/v1/podcasts/channels",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            "/api/v1/podcasts/channels?page=1&pageSize=501",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::POST,
            "/api/v1/podcasts/channels",
            json(serde_json::json!({"url":format!("{discovery_endpoint}feed.xml")})),
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    let channel: i64 =
        sqlx::query_scalar("SELECT id FROM native_podcast_channels WHERE owner_user_id=$1")
            .bind(allowed.id)
            .fetch_one(&db.pool)
            .await
            .expect("channel");
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::PATCH,
            &format!("/api/v1/podcasts/channels/{channel}"),
            json(serde_json::json!({"autoDownloadEnabled":true,"refreshIntervalHours":12})),
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::POST,
            &format!("/api/v1/podcasts/channels/{channel}/refresh"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    let episode:i64=sqlx::query_scalar("INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,description,duration_ms,enclosure_url,created_at,updated_at) VALUES($1,$2,'fixture-episode','Episode One','searchable details',60000,'https://example.test/episode.mp3',now(),now()) RETURNING id").bind(Uuid::now_v7()).bind(channel).fetch_one(&db.pool).await.expect("episode");
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            &format!("/api/v1/podcasts/channels/{channel}/episodes?page=1&pageSize=50"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::POST,
            &format!("/api/v1/podcasts/episodes/{episode}/play?secondsPlayed=16"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            "/api/v1/podcasts/episodes/search?query=searchable&page=1&pageSize=50",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::POST,
            &format!("/api/v1/podcasts/episodes/{episode}/download"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::POST,
            &format!("/api/v1/podcasts/episodes/{episode}/play?secondsPlayed=15"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, Option<i32>)>(
            "SELECT count(*),max(seconds_played) FROM native_podcast_play_history
             WHERE user_id=$1 AND episode_id=$2 AND is_now_playing",
        )
        .bind(allowed.id)
        .bind(episode)
        .fetch_one(&db.pool)
        .await
        .expect("heartbeat state"),
        (1, Some(15)),
        "a repeated heartbeat updates one active playback row"
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::PUT,
            &format!("/api/v1/podcasts/episodes/{episode}/bookmark"),
            json(serde_json::json!({"positionSeconds":15,"comment":"resume"})),
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            &format!("/api/v1/podcasts/episodes/{episode}/bookmark"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            &format!("/api/v1/podcasts/episodes/{episode}/history?limit=50&offset=0"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::DELETE,
            &format!("/api/v1/podcasts/episodes/{episode}/bookmark"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    let exported = call(
        state.clone(),
        Some(p),
        Method::GET,
        "/api/v1/podcasts/opml/export",
        vec![],
        "application/xml",
    )
    .await;
    assert_eq!(exported.0, StatusCode::OK);
    assert!(
        String::from_utf8(exported.1)
            .expect("opml")
            .contains(&format!("{discovery_endpoint}feed.xml"))
    );
    let imported = call(
        state.clone(),
        Some(p),
        Method::POST,
        "/api/v1/podcasts/opml/import",
        b"<opml><body><outline xmlUrl=\"https://example.test/second.xml\"/></body></opml>".to_vec(),
        "application/xml",
    )
    .await;
    assert_eq!(imported.0, StatusCode::OK);
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            "/api/v1/podcasts/discover/search?query=rust%20%26%20music&limit=25&country=US",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            "/api/v1/podcasts/discover/trending?limit=25&country=US",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::GET,
            "/api/v1/podcasts/discover/lookup/12345",
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    let missing_lookup = call(
        state.clone(),
        Some(p),
        Method::GET,
        "/api/v1/podcasts/discover/lookup/0",
        vec![],
        "application/json",
    )
    .await;
    assert_eq!(missing_lookup.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&missing_lookup.1)
            .expect("captured lookup error")["message"],
        "Podcast not found"
    );
    let fetched: (i64, Uuid) = sqlx::query_as(
        "SELECT id,public_api_id FROM native_podcast_episodes WHERE channel_id=$1 AND feed_item_id='fixture-from-feed'",
    )
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("refreshed episode");
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloading',download_lease_owner=$2,\
         download_lease_expires_at=now()-interval '1 second' WHERE id=$1",
    )
    .bind(fetched.0)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await
    .expect("expired lease");
    let storage = tempfile::tempdir().expect("podcast storage");
    std::fs::write(
        storage.path().join(".melodee-podcast-root"),
        b"melodee-podcast-root-v1\n",
    )
    .expect("ownership marker");
    let worker_config = melodee_server::podcast_worker::PodcastConfig {
        worker_enabled: true,
        storage_root: Some(storage.path().to_path_buf()),
        allow_loopback_http: true,
        max_attempts: 1,
        ..melodee_server::podcast_worker::PodcastConfig::default()
    };
    assert_eq!(
        melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
            .await
            .expect("download pass"),
        1,
        "an expired lease is reclaimed and completed"
    );
    let final_path = storage.path().join(format!("{}.media", fetched.1));
    assert_eq!(
        std::fs::read(&final_path).expect("downloaded enclosure"),
        b"fixture podcast media"
    );
    let orphan = storage
        .path()
        .join(format!("{}.{}.tmp", Uuid::now_v7(), Uuid::now_v7()));
    std::fs::write(&orphan, b"partial").expect("orphan");
    assert_eq!(
        melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
            .await
            .expect("idempotent pass"),
        0
    );
    assert!(
        !orphan.exists(),
        "owned orphan temporary files are recovered"
    );

    let (blocked_url, arrival, release_download) = blocked_media_fixture().await;
    let fenced_public_id = Uuid::now_v7();
    let fenced_episode: i64 = sqlx::query_scalar(
        "INSERT INTO native_podcast_episodes (
             public_api_id, channel_id, feed_item_id, title, enclosure_url,
             download_status, download_available_at, created_at, updated_at
         ) VALUES ($1, $2, 'lease-fenced-episode', 'Lease Fenced Episode', $3,
                   'Queued', now(), now(), now()) RETURNING id",
    )
    .bind(fenced_public_id)
    .bind(channel)
    .bind(blocked_url)
    .fetch_one(&db.pool)
    .await
    .expect("lease-fenced episode");
    let worker_pool = db.pool.clone();
    let worker_pass_config = worker_config.clone();
    let old_worker = tokio::spawn(async move {
        melodee_server::podcast_worker::run_podcast_once(&worker_pool, &worker_pass_config).await
    });
    tokio::time::timeout(Duration::from_secs(5), arrival)
        .await
        .expect("worker reached enclosure fixture")
        .expect("enclosure arrival signal");
    let stolen_worker = Uuid::now_v7();
    assert_eq!(
        sqlx::query(
            "UPDATE native_podcast_episodes
             SET download_lease_owner=$2, download_lease_expires_at=now()-interval '1 second'
             WHERE id=$1 AND download_status='Downloading'",
        )
        .bind(fenced_episode)
        .bind(stolen_worker)
        .execute(&db.pool)
        .await
        .expect("steal expired downloader publication lease")
        .rows_affected(),
        1
    );
    assert_eq!(
        melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
            .await
            .expect("successor recovery download pass"),
        1,
        "an expired claim can be completed while its stale predecessor is still unwinding"
    );
    let fenced_path = storage.path().join(format!("{fenced_public_id}.media"));
    assert_eq!(
        std::fs::read(&fenced_path).expect("successor enclosure"),
        b"lease-fenced podcast media"
    );
    release_download.notify_waiters();
    assert_eq!(
        old_worker
            .await
            .expect("old worker task")
            .expect("old worker pass"),
        0,
        "a stale downloader cannot publish after its lease is stolen"
    );
    assert_eq!(
        std::fs::read(&fenced_path).expect("successor enclosure survives stale cleanup"),
        b"lease-fenced podcast media",
        "the stale downloader cannot delete its successor's publication"
    );
    let fenced_state: (String, Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT download_status, download_lease_owner, local_path
         FROM native_podcast_episodes WHERE id=$1",
    )
    .bind(fenced_episode)
    .fetch_one(&db.pool)
    .await
    .expect("lease-fenced state");
    assert_eq!(
        fenced_state,
        (
            "Downloaded".to_owned(),
            None,
            Some(fenced_path.to_string_lossy().into_owned())
        )
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::DELETE,
            &format!("/api/v1/podcasts/episodes/{fenced_episode}"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
        .await
        .expect("deleted-download recovery pass");
    assert!(
        !fenced_path.exists(),
        "soft deletion cancels download authority and orphan recovery removes its enclosure"
    );
    assert_eq!(
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT download_status,local_path FROM native_podcast_episodes WHERE id=$1",
        )
        .bind(fenced_episode)
        .fetch_one(&db.pool)
        .await
        .expect("deleted episode state"),
        ("NotDownloaded".to_owned(), None)
    );

    sqlx::query("UPDATE native_podcast_channels SET max_downloaded_episodes=0 WHERE id=$1")
        .bind(channel)
        .execute(&db.pool)
        .await
        .expect("retention policy");
    sqlx::query(
        "INSERT INTO native_podcast_bookmarks (
             user_id,episode_id,position_seconds,comment,created_at,updated_at
         ) VALUES ($1,$2,12,'retain this download',now(),now())",
    )
    .bind(allowed.id)
    .bind(fetched.0)
    .execute(&db.pool)
    .await
    .expect("retention bookmark");
    melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
        .await
        .expect("retention pass");
    assert!(
        final_path.exists(),
        "retention never removes a bookmarked episode"
    );
    sqlx::query("DELETE FROM native_podcast_bookmarks WHERE user_id=$1 AND episode_id=$2")
        .bind(allowed.id)
        .bind(fetched.0)
        .execute(&db.pool)
        .await
        .expect("release retention bookmark");
    melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
        .await
        .expect("unbookmarked retention pass");
    assert!(
        !final_path.exists(),
        "retention removes only the owned enclosure"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT download_status FROM native_podcast_episodes WHERE id=$1",
        )
        .bind(fetched.0)
        .fetch_one(&db.pool)
        .await
        .expect("retention status"),
        "NotDownloaded"
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::DELETE,
            &format!("/api/v1/podcasts/episodes/{episode}"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            Some(p),
            Method::DELETE,
            &format!("/api/v1/podcasts/channels/{channel}?softDelete=true"),
            vec![],
            "application/json"
        )
        .await
        .0,
        StatusCode::OK
    );
    db.drop().await;
}

#[tokio::test]
async fn podcast_orphan_reconciliation_continues_after_first_bounded_batch() {
    let db = common::TestDb::create("phase7_podcast_orphan_batches").await;
    let storage = tempfile::tempdir().expect("podcast storage");
    std::fs::write(
        storage.path().join(".melodee-podcast-root"),
        b"melodee-podcast-root-v1\n",
    )
    .expect("ownership marker");
    let mut orphans = Vec::new();
    for _ in 0..300 {
        let path = storage
            .path()
            .join(format!("{}.{}.tmp", Uuid::now_v7(), Uuid::now_v7()));
        std::fs::write(&path, b"abandoned partial download").expect("orphan fixture");
        orphans.push(path);
    }
    let config = melodee_server::podcast_worker::PodcastConfig {
        worker_enabled: true,
        storage_root: Some(storage.path().to_path_buf()),
        ..melodee_server::podcast_worker::PodcastConfig::default()
    };

    assert_eq!(
        melodee_server::podcast_worker::run_podcast_once(&db.pool, &config)
            .await
            .expect("reconciliation pass"),
        0
    );
    assert!(
        orphans.iter().all(|path| !path.exists()),
        "every bounded set is reconciled; entries after the first 256 cannot starve"
    );
    db.drop().await;
}
