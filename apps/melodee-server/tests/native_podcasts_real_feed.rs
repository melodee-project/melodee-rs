//! real-feed exercise: closes the limitation "real podcast
//! feeds unexercised". A local, self-contained HTTP fixture serves a
//! realistic iTunes-namespaced RSS 2.0 feed and three binary enclosures; the
//! actual refresh + leased download pipeline runs end-to-end and the stored
//! bytes, episode metadata, and playback history/progress rows are verified.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
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
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug)]
struct Signer;
impl NativeStreamUrlSigner for Signer {
    fn stream_url(&self, _: Uuid, _: Uuid, _: &str) -> NativeFuture<'_, Result<String, ()>> {
        Box::pin(async { Ok(String::new()) })
    }
}

fn json(v: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&v).expect("json")
}

/// Deterministic pseudo-media: an ID3v2-style header followed by an LCG fill,
/// so each enclosure is binary, distinct, and byte-comparable after storage.
fn enclosure_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(b"ID3\x04\x00\x00\x00\x00\x00\x00");
    let mut state = seed | 1;
    while bytes.len() < len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

struct FeedFixture {
    endpoint: String,
    payloads: [Vec<u8>; 3],
}

async fn real_feed_fixture() -> FeedFixture {
    let payloads = [
        enclosure_payload(0x9e37_79b9, 64_517),
        enclosure_payload(0x243f_6a88, 128_009),
        enclosure_payload(0x4528_21e6, 96_301),
    ];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("feed fixture listener");
    let address = listener.local_addr().expect("feed fixture address");
    let feed = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:atom="http://www.w3.org/2005/Atom" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title><![CDATA[The Fixture Signal]]></title>
    <link>https://fixture-signal.test/</link>
    <language>en-us</language>
    <copyright>Copyright 2026 Fixture Network</copyright>
    <itunes:author>Fixture Network</itunes:author>
    <description><![CDATA[A realistic fixture feed & friends <b>about</b> parsing]]></description>
    <itunes:summary>Channel-level iTunes summary</itunes:summary>
    <itunes:owner>
      <itunes:name>Fixture Network</itunes:name>
      <itunes:email>podcasts@fixture-signal.test</itunes:email>
    </itunes:owner>
    <itunes:image href="https://fixture-signal.test/cover-3000.jpg"/>
    <image>
      <url>https://fixture-signal.test/cover-144.jpg</url>
      <title>The Fixture Signal</title>
      <link>https://fixture-signal.test/</link>
    </image>
    <itunes:category text="Technology">
      <itunes:category text="Podcasting"/>
    </itunes:category>
    <itunes:explicit>false</itunes:explicit>
    <atom:link href="http://{address}/feed.xml" rel="self" type="application/rss+xml"/>
    <item>
      <guid isPermaLink="false">urn:uuid:11111111-2222-3333-4444-555555555503</guid>
      <title><![CDATA[Episode 3: Parsing Feeds & Friends]]></title>
      <link>https://fixture-signal.test/episodes/3</link>
      <description><![CDATA[Show <i>notes</i> with <a href="https://fixture-signal.test/episodes/3">links</a> & entities]]></description>
      <content:encoded><![CDATA[<p>Full HTML show notes are ignored in favor of description</p>]]></content:encoded>
      <pubDate>Wed, 12 Aug 2026 15:04:05 +0000</pubDate>
      <itunes:duration>01:02:03</itunes:duration>
      <itunes:episode>3</itunes:episode>
      <itunes:episodeType>full</itunes:episodeType>
      <enclosure url="http://{address}/media/ep3.mp3" length="96301" type="audio/mpeg"/>
    </item>
    <item>
      <guid isPermaLink="false">urn:uuid:11111111-2222-3333-4444-555555555502</guid>
      <title>Episode 2: Lease Fencing</title>
      <link>https://fixture-signal.test/episodes/2</link>
      <description>Plain description for episode two</description>
      <pubDate>Tue, 04 Aug 2026 09:30:00 +0000</pubDate>
      <itunes:duration>42:17</itunes:duration>
      <itunes:episode>2</itunes:episode>
      <itunes:episodeType>full</itunes:episodeType>
      <enclosure url="http://{address}/media/ep2.mp3" length="128009" type="audio/mpeg"/>
    </item>
    <item>
      <guid isPermaLink="false">urn:uuid:11111111-2222-3333-4444-555555555501</guid>
      <title>Episode 1: Origins</title>
      <link>https://fixture-signal.test/episodes/1</link>
      <itunes:summary><![CDATA[Episode one summarized via itunes:summary]]></itunes:summary>
      <pubDate>Tue, 28 Jul 2026 18:00:00 +0000</pubDate>
      <itunes:duration>1835</itunes:duration>
      <itunes:episode>1</itunes:episode>
      <itunes:episodeType>trailer</itunes:episodeType>
      <enclosure url="http://{address}/media/ep1.mp3" length="64517" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#
    );
    let served = payloads.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/feed.xml",
                    get(move || {
                        let feed = feed.clone();
                        async move { ([(header::CONTENT_TYPE, "application/rss+xml")], feed) }
                    }),
                )
                .route(
                    "/media/ep1.mp3",
                    get({
                        let served = served.clone();
                        move || {
                            let body = served[0].clone();
                            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], body) }
                        }
                    }),
                )
                .route(
                    "/media/ep2.mp3",
                    get({
                        let served = served.clone();
                        move || {
                            let body = served[1].clone();
                            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], body) }
                        }
                    }),
                )
                .route(
                    "/media/ep3.mp3",
                    get({
                        move || {
                            let body = served[2].clone();
                            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], body) }
                        }
                    }),
                ),
        )
        .await
        .expect("feed fixture server");
    });
    FeedFixture {
        endpoint: format!("http://{address}/"),
        payloads,
    }
}

async fn user(db: &common::TestDb, name: &str) -> StoredNativeUser {
    let now = OffsetDateTime::now_utc();
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
                stream_signing_key: "podcast-real-feed-key",
                is_admin: false,
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: None,
                preferred_theme: None,
                auth_revision: 1,
                roles: &["HasPodcastRole".to_owned()],
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("user")
}

fn state(db: &common::TestDb) -> NativeState {
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
                "http://127.0.0.1:9/",
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

async fn call(
    state: NativeState,
    p: NativePrincipal,
    method: Method,
    uri: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request");
    req.extensions_mut().insert(p);
    req.extensions_mut()
        .insert(NativeClientBinding("127.0.0.1".to_owned()));
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
async fn real_itunes_feed_drives_refresh_download_and_playback_state_end_to_end() {
    let db = common::TestDb::create("phase9_real_feed_exercise").await;
    let owner = user(&db, "real-feed-listener").await;
    let p = NativePrincipal {
        user_id: owner.id,
        public_api_id: owner.public_api_id,
        is_admin: owner.is_admin,
        is_editor: owner.is_editor,
    };
    let fixture = real_feed_fixture().await;
    let state = state(&db);

    // Subscribe, enable auto-download, then run the real refresh path.
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::POST,
            "/api/v1/podcasts/channels",
            json(serde_json::json!({"url": format!("{}feed.xml", fixture.endpoint)})),
        )
        .await
        .0,
        StatusCode::OK
    );
    let channel: i64 =
        sqlx::query_scalar("SELECT id FROM native_podcast_channels WHERE owner_user_id=$1")
            .bind(owner.id)
            .fetch_one(&db.pool)
            .await
            .expect("channel");
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::PATCH,
            &format!("/api/v1/podcasts/channels/{channel}"),
            json(serde_json::json!({"autoDownloadEnabled": true})),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::POST,
            &format!("/api/v1/podcasts/channels/{channel}/refresh"),
            vec![],
        )
        .await
        .0,
        StatusCode::OK
    );

    // Channel metadata came from the iTunes-namespaced feed.
    let channel_row: (String, String, String, String, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT title,description,website,image_url,last_sync_at FROM native_podcast_channels WHERE id=$1",
    )
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("channel row");
    assert_eq!(channel_row.0, "The Fixture Signal");
    assert!(
        channel_row.1.contains("realistic fixture feed & friends"),
        "CDATA description is decoded: {}",
        channel_row.1
    );
    assert_eq!(channel_row.2, "https://fixture-signal.test/");
    assert_eq!(channel_row.3, "https://fixture-signal.test/cover-144.jpg");
    assert!(channel_row.4.is_some(), "refresh recorded a sync time");

    // All three episodes parsed with identity, duration, and publish time;
    // auto-download queued each enclosure.
    let episodes: Vec<(i64, Uuid, String, String, Option<i64>, String)> = sqlx::query_as(
        "SELECT id,public_api_id,feed_item_id,download_status,duration_ms,title \
         FROM native_podcast_episodes WHERE channel_id=$1 ORDER BY published_at DESC,id",
    )
    .bind(channel)
    .fetch_all(&db.pool)
    .await
    .expect("episodes");
    assert_eq!(episodes.len(), 3);
    let expected = [
        (
            "urn:uuid:11111111-2222-3333-4444-555555555503",
            3_723_000_i64,
            "Episode 3: Parsing Feeds & Friends",
        ),
        (
            "urn:uuid:11111111-2222-3333-4444-555555555502",
            2_537_000,
            "Episode 2: Lease Fencing",
        ),
        (
            "urn:uuid:11111111-2222-3333-4444-555555555501",
            1_835_000,
            "Episode 1: Origins",
        ),
    ];
    for (row, (guid, duration_ms, title)) in episodes.iter().zip(expected.iter()) {
        assert_eq!(&row.2, guid);
        assert_eq!(row.4, Some(*duration_ms), "duration for {title}");
        assert_eq!(&row.5, title);
        assert_eq!(row.3, "Queued", "auto-download queued {title}");
    }
    let summary_description: String = sqlx::query_scalar(
        "SELECT description FROM native_podcast_episodes \
         WHERE channel_id=$1 AND feed_item_id='urn:uuid:11111111-2222-3333-4444-555555555501'",
    )
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("itunes:summary episode");
    assert_eq!(
        summary_description,
        "Episode one summarized via itunes:summary"
    );
    let published_exact: bool = sqlx::query_scalar(
        "SELECT published_at='2026-08-12T15:04:05Z'::timestamptz FROM native_podcast_episodes \
         WHERE channel_id=$1 AND feed_item_id='urn:uuid:11111111-2222-3333-4444-555555555503'",
    )
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("published_at");
    assert!(published_exact, "RFC 2822 pubDate parsed exactly");

    // Run the real leased download pipeline against the fixture enclosures.
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
        ..melodee_server::podcast_worker::PodcastConfig::default()
    };
    assert_eq!(
        melodee_server::podcast_worker::run_podcast_once(&db.pool, &worker_config)
            .await
            .expect("download pass"),
        3,
        "all three queued enclosures download in one bounded pass"
    );

    // Byte-identical storage and durable download rows per episode.
    for (index, (episode, public_id, ..)) in episodes.iter().enumerate() {
        let payload = &fixture.payloads[2 - index];
        let path = storage.path().join(format!("{public_id}.media"));
        assert_eq!(
            std::fs::read(&path).expect("stored enclosure"),
            *payload,
            "stored enclosure is byte-identical to the served payload"
        );
        let row: (String, Option<String>, Option<i64>, Option<OffsetDateTime>) = sqlx::query_as(
            "SELECT download_status,local_path,downloaded_bytes,downloaded_at \
             FROM native_podcast_episodes WHERE id=$1",
        )
        .bind(episode)
        .fetch_one(&db.pool)
        .await
        .expect("download row");
        assert_eq!(row.0, "Downloaded");
        assert_eq!(row.1.as_deref(), Some(path.to_string_lossy().as_ref()));
        assert_eq!(
            row.2,
            Some(i64::try_from(payload.len()).expect("length")),
            "recorded byte count matches the served payload"
        );
        assert!(row.3.is_some(), "download completion is recorded");
        let lease: (Option<Uuid>, Option<OffsetDateTime>) = sqlx::query_as(
            "SELECT download_lease_owner,download_lease_expires_at \
             FROM native_podcast_episodes WHERE id=$1",
        )
        .bind(episode)
        .fetch_one(&db.pool)
        .await
        .expect("lease columns");
        assert_eq!(lease, (None, None), "completed downloads release the lease");
    }

    // A repeated refresh of the unchanged feed preserves download state.
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::POST,
            &format!("/api/v1/podcasts/channels/{channel}/refresh"),
            vec![],
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM native_podcast_episodes WHERE channel_id=$1 AND download_status='Downloaded'",
        )
        .bind(channel)
        .fetch_one(&db.pool)
        .await
        .expect("downloaded count"),
        3,
        "refresh upsert never resets download state"
    );

    // Playback history/progress rows attach to the real downloaded episodes.
    let ep3 = episodes[0].0;
    let ep2 = episodes[1].0;
    let ep1 = episodes[2].0;
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::POST,
            &format!("/api/v1/podcasts/episodes/{ep3}/play?secondsPlayed=87"),
            vec![],
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            state.clone(),
            p,
            Method::POST,
            &format!("/api/v1/podcasts/episodes/{ep2}/play?secondsPlayed=12"),
            vec![],
        )
        .await
        .0,
        StatusCode::OK
    );
    let now_playing: Vec<(i64, Option<i32>)> = sqlx::query_as(
        "SELECT episode_id,seconds_played FROM native_podcast_play_history \
         WHERE user_id=$1 AND is_now_playing",
    )
    .bind(owner.id)
    .fetch_all(&db.pool)
    .await
    .expect("now playing rows");
    assert_eq!(
        now_playing,
        vec![(ep2, Some(12))],
        "exactly one now-playing row per user; starting episode 2 demoted episode 3"
    );
    let ep3_history: (String, Option<i32>, bool) = sqlx::query_as(
        "SELECT client,seconds_played,is_now_playing FROM native_podcast_play_history \
         WHERE user_id=$1 AND episode_id=$2",
    )
    .bind(owner.id)
    .bind(ep3)
    .fetch_one(&db.pool)
    .await
    .expect("episode 3 history row");
    assert_eq!(
        ep3_history,
        ("Melodee.Blazor".to_owned(), Some(87), false),
        "progress heartbeat row is durable and no longer now-playing"
    );

    assert_eq!(
        call(
            state.clone(),
            p,
            Method::PUT,
            &format!("/api/v1/podcasts/episodes/{ep1}/bookmark"),
            json(serde_json::json!({"positionSeconds": 300, "comment": "chapter mark"})),
        )
        .await
        .0,
        StatusCode::OK
    );
    let bookmark: (i32, Option<String>) = sqlx::query_as(
        "SELECT position_seconds,comment FROM native_podcast_bookmarks WHERE user_id=$1 AND episode_id=$2",
    )
    .bind(owner.id)
    .bind(ep1)
    .fetch_one(&db.pool)
    .await
    .expect("bookmark row");
    assert_eq!(bookmark, (300, Some("chapter mark".to_owned())));

    let history = call(
        state.clone(),
        p,
        Method::GET,
        &format!("/api/v1/podcasts/episodes/{ep3}/history?limit=50&offset=0"),
        vec![],
    )
    .await;
    assert_eq!(history.0, StatusCode::OK);
    let history: serde_json::Value = serde_json::from_slice(&history.1).expect("history json");
    assert_eq!(
        history[0]["secondsPlayed"],
        serde_json::json!(87),
        "history endpoint reports the recorded progress: {history}"
    );

    db.drop().await;
}
