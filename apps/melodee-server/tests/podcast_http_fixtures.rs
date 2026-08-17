//! R2.6 podcast ingestion evidence (PRD-POD-001): deterministic per-test
//! HTTP fixtures model the wire behaviors a real feed/enclosure server
//! exhibits — redirects, error statuses, conditional-request answers,
//! partial (206) responses, malformed feeds, and mid-body enclosure
//! interruption — and pin the worker's fail-closed handling of each.
//!
//! The fixture is a raw TCP HTTP/1.1 server (no framework) bound to an
//! ephemeral loopback port per test. Responses are scripted per path and
//! per request ordinal, so a retry sees a different answer than the first
//! attempt. Every reply carries `Connection: close`; each test's server is
//! aborted on drop, so parallel tests never share state.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use melodee_server::podcast_worker::{PodcastConfig, refresh_channel, run_podcast_once};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

/// One scripted answer for a path. Selected by request ordinal; once the
/// script is exhausted the last entry repeats.
#[derive(Clone)]
enum Script {
    /// Full response with the given status and body.
    Respond {
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    },
    /// 3xx with a Location header and no body.
    Redirect { status: u16, location: String },
    /// Declares `declared_len` in Content-Length but sends only `body`
    /// and then closes — a mid-body interruption.
    Truncate { declared_len: usize, body: Vec<u8> },
}

struct FixtureServer {
    base_url: String,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    task: tokio::task::AbortHandle,
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FixtureServer {
    fn start(scripts: Vec<(&'static str, Vec<Script>)>) -> Self {
        let scripts: Arc<HashMap<&'static str, Vec<Script>>> =
            Arc::new(scripts.into_iter().collect());
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let port = listener.local_addr().expect("fixture address").port();
        let hits_clone = Arc::clone(&hits);
        let task = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            while let Ok((stream, _)) = listener.accept().await {
                let scripts = Arc::clone(&scripts);
                let hits = Arc::clone(&hits_clone);
                tokio::spawn(async move {
                    let _ = serve_one(stream, &scripts, &hits).await;
                });
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            hits,
            task: task.abort_handle(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn hits(&self, path: &str) -> usize {
        self.hits
            .lock()
            .expect("hits")
            .get(path)
            .copied()
            .unwrap_or(0)
    }
}

async fn serve_one(
    stream: tokio::net::TcpStream,
    scripts: &HashMap<&'static str, Vec<Script>>,
    hits: &Mutex<HashMap<String, usize>>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let request_line = lines.next_line().await?.unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    // Drain the request headers; the worker never sends a request body.
    loop {
        match lines.next_line().await? {
            Some(line) if !line.is_empty() => {}
            _ => break,
        }
    }
    let ordinal = {
        let mut hits = hits.lock().expect("hits");
        let entry = hits.entry(path.clone()).or_insert(0);
        *entry += 1;
        *entry
    };
    let script = scripts
        .get(path.as_str())
        .and_then(|responses| responses.get((ordinal - 1).min(responses.len() - 1)));
    match script {
        Some(Script::Respond {
            status,
            content_type,
            body,
        }) => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
                reason(*status),
                body.len()
            );
            writer.write_all(head.as_bytes()).await?;
            writer.write_all(body).await?;
        }
        Some(Script::Redirect { status, location }) => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                reason(*status)
            );
            writer.write_all(head.as_bytes()).await?;
        }
        Some(Script::Truncate { declared_len, body }) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {declared_len}\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n"
            );
            writer.write_all(head.as_bytes()).await?;
            writer.write_all(body).await?;
            writer.shutdown().await?;
        }
        None => {
            writer
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
        }
    }
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn xml(body: &str) -> Script {
    Script::Respond {
        status: 200,
        content_type: "application/rss+xml",
        body: body.as_bytes().to_vec(),
    }
}

/// A well-formed RSS document whose single enclosure URL is interpolated.
fn feed_with_enclosure(enclosure_url: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>Fixture Show</title>
<description>Deterministic fixture feed</description>
<link>https://example.test/show</link>
<item><guid>episode-1</guid><title>Episode One</title>
<description>First episode</description>
<pubDate>Mon, 17 Aug 2026 12:00:00 +0000</pubDate>
<enclosure url="{enclosure_url}" type="audio/mpeg" length="1234"/></item>
</channel></rss>"#
    )
}

async fn seed_owner(db: &common::TestDb, name: &str) -> Uuid {
    let owner = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,normalized_email,\
         password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at) \
         VALUES($1,$2,$3,upper($3),$4,upper($4),$5,'bcrypt','podcast-fixture-key',now(),now())",
    )
    .bind(owner)
    .bind(Uuid::now_v7())
    .bind(format!("{name}-owner"))
    .bind(format!("{name}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .execute(&db.pool)
    .await
    .expect("owner");
    owner
}

async fn seed_channel(
    db: &common::TestDb,
    owner: Uuid,
    feed_url: &str,
    title: &str,
    auto_download: bool,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO native_podcast_channels(public_api_id,owner_user_id,feed_url,title,auto_download_enabled,created_at,updated_at) \
         VALUES($1,$2,$3,$4,$5,now(),now()) RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(owner)
    .bind(feed_url)
    .bind(title)
    .bind(auto_download)
    .fetch_one(&db.pool)
    .await
    .expect("channel")
}

async fn seed_queued_episode(db: &common::TestDb, channel: i64, enclosure_url: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,\
         enclosure_url,download_status,created_at,updated_at) \
         VALUES($1,$2,$3,$4,$5,'Queued',now(),now()) RETURNING public_api_id",
    )
    .bind(Uuid::now_v7())
    .bind(channel)
    .bind(format!("fixture-item-{}", Uuid::now_v7()))
    .bind("Fixture Episode")
    .bind(enclosure_url)
    .fetch_one(&db.pool)
    .await
    .expect("episode")
}

fn worker_config(root: &Path, max_attempts: i32) -> PodcastConfig {
    PodcastConfig {
        enabled: true,
        worker_enabled: true,
        storage_root: Some(root.to_path_buf()),
        claim_batch_size: 4,
        lease_duration: Duration::from_secs(30),
        max_attempts,
        max_download_bytes: 4 * 1024 * 1024,
        allow_loopback_http: true,
        ..PodcastConfig::default()
    }
}

async fn episode_row(
    db: &common::TestDb,
    public_api_id: Uuid,
) -> (String, i32, Option<String>, Option<String>, i64) {
    sqlx::query_as::<_, (String, i32, Option<String>, Option<String>, Option<i64>)>(
        "SELECT download_status,download_attempt_count,download_error,local_path,downloaded_bytes \
         FROM native_podcast_episodes WHERE public_api_id=$1",
    )
    .bind(public_api_id)
    .fetch_one(&db.pool)
    .await
    .map(|(status, attempts, error, path, bytes)| {
        (status, attempts, error, path, bytes.unwrap_or(-1))
    })
    .expect("episode row")
}

fn tmp_files(root: &Path) -> usize {
    std::fs::read_dir(root)
        .expect("root")
        .filter(|entry| {
            entry
                .as_ref()
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        })
        .count()
}

// ---- Feed refresh fixtures -------------------------------------------------

#[tokio::test]
async fn feed_redirect_is_fail_closed_and_surfaced() {
    let server = FixtureServer::start(vec![(
        "/feed.xml",
        vec![Script::Redirect {
            status: 301,
            location: "https://feeds.example.test/moved.xml".to_string(),
        }],
    )]);
    let db = common::TestDb::create("podcast_feed_redirect").await;
    let owner = seed_owner(&db, "feed-redirect").await;
    let channel = seed_channel(&db, owner, &server.url("/feed.xml"), "Redirect Show", false).await;

    // Redirect policy is `none`: a 3xx answer is a surfaced refresh error,
    // never a silent follow across an unvalidated origin.
    let result = refresh_channel(&db.pool, owner, channel, true).await;
    assert!(result.is_err(), "a redirecting feed must fail closed");
    let last_sync: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_sync_at FROM native_podcast_channels WHERE id=$1")
            .bind(channel)
            .fetch_one(&db.pool)
            .await
            .expect("channel row");
    assert!(
        last_sync.is_none(),
        "a failed refresh never stamps a successful sync"
    );
    db.drop().await;
}

#[tokio::test]
async fn feed_error_statuses_are_fail_closed() {
    for status in [404_u16, 500] {
        let server = FixtureServer::start(vec![(
            "/feed.xml",
            vec![Script::Respond {
                status,
                content_type: "text/plain",
                body: b"no feed here".to_vec(),
            }],
        )]);
        let db = common::TestDb::create(&format!("podcast_feed_{status}")).await;
        let owner = seed_owner(&db, &format!("feed-{status}")).await;
        let channel = seed_channel(&db, owner, &server.url("/feed.xml"), "Error Show", false).await;
        assert!(
            refresh_channel(&db.pool, owner, channel, true)
                .await
                .is_err(),
            "status {status} must fail the refresh"
        );
        db.drop().await;
    }
}

#[tokio::test]
async fn feed_not_modified_is_fail_closed_not_silent_stale() {
    // The worker sends no conditional headers (no ETag/Last-Modified
    // caching); a 304 answer is therefore a protocol anomaly. It fails the
    // refresh honestly instead of being treated as silent success.
    let server = FixtureServer::start(vec![(
        "/feed.xml",
        vec![Script::Respond {
            status: 304,
            content_type: "text/plain",
            body: Vec::new(),
        }],
    )]);
    let db = common::TestDb::create("podcast_feed_304").await;
    let owner = seed_owner(&db, "feed-304").await;
    let channel = seed_channel(&db, owner, &server.url("/feed.xml"), "304 Show", false).await;
    assert!(
        refresh_channel(&db.pool, owner, channel, true)
            .await
            .is_err(),
        "304 without a conditional request must fail closed"
    );
    db.drop().await;
}

#[tokio::test]
async fn feed_partial_content_is_fail_closed() {
    let server = FixtureServer::start(vec![(
        "/feed.xml",
        vec![Script::Respond {
            status: 206,
            content_type: "application/rss+xml",
            body: feed_with_enclosure("https://media.example.test/e.mp3").into_bytes(),
        }],
    )]);
    let db = common::TestDb::create("podcast_feed_206").await;
    let owner = seed_owner(&db, "feed-206").await;
    let channel = seed_channel(&db, owner, &server.url("/feed.xml"), "206 Show", false).await;
    assert!(
        refresh_channel(&db.pool, owner, channel, true)
            .await
            .is_err(),
        "206 without a sent Range must never be parsed as a complete feed"
    );
    db.drop().await;
}

#[tokio::test]
async fn malformed_feeds_write_no_episode_rows() {
    for (name, body) in [
        // quick-xml `check_end_names` (default on) rejects the crossed close.
        (
            "mismatched-tags",
            "<rss><channel><item><title>x</title></channel></rss>",
        ),
        // An unterminated attribute is ill-formed at the tokenizer level.
        (
            "unterminated-attribute",
            "<rss version=\"2.0><channel><title>x</title></channel></rss>",
        ),
        // Worker-level validation: an item without a title is not a podcast
        // episode and rejects the whole refresh.
        (
            "missing-item-title",
            "<rss><channel><title>Show</title><item><guid>e1</guid></item></channel></rss>",
        ),
    ] {
        let server = FixtureServer::start(vec![("/feed.xml", vec![xml(body)])]);
        let db = common::TestDb::create(&format!("podcast_bad_{name}")).await;
        let owner = seed_owner(&db, name).await;
        let channel = seed_channel(
            &db,
            owner,
            &server.url("/feed.xml"),
            "Malformed Show",
            false,
        )
        .await;
        assert!(
            refresh_channel(&db.pool, owner, channel, true)
                .await
                .is_err(),
            "{name} must fail the refresh"
        );
        let episodes: i64 =
            sqlx::query_scalar("SELECT count(*) FROM native_podcast_episodes WHERE channel_id=$1")
                .bind(channel)
                .fetch_one(&db.pool)
                .await
                .expect("count");
        assert_eq!(episodes, 0, "{name} wrote episode rows");
        db.drop().await;
    }
}

/// Pinned leniency boundary: a syntactically readable but content-free
/// document (e.g. an HTML error page from a captive portal) parses as an
/// empty feed. That is a non-destructive no-op — no episode rows are
/// written and the stored channel title survives — recorded here as the
/// accepted boundary so a future strictness change is a deliberate,
/// evidenced decision rather than drift.
#[tokio::test]
async fn content_free_document_is_a_non_destructive_noop() {
    let server = FixtureServer::start(vec![(
        "/feed.xml",
        vec![xml("<html><body>gateway error page</body></html>")],
    )]);
    let db = common::TestDb::create("podcast_content_free").await;
    let owner = seed_owner(&db, "content-free").await;
    let channel = seed_channel(
        &db,
        owner,
        &server.url("/feed.xml"),
        "Original Title",
        false,
    )
    .await;
    refresh_channel(&db.pool, owner, channel, true)
        .await
        .expect("content-free documents parse as empty feeds");
    let title: String = sqlx::query_scalar("SELECT title FROM native_podcast_channels WHERE id=$1")
        .bind(channel)
        .fetch_one(&db.pool)
        .await
        .expect("channel row");
    assert_eq!(title, "Original Title", "an empty feed overwrites nothing");
    let episodes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_podcast_episodes WHERE channel_id=$1")
            .bind(channel)
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(episodes, 0);
    db.drop().await;
}

#[tokio::test]
async fn episode_without_enclosure_stays_not_downloaded() {
    let server = FixtureServer::start(vec![(
        "/feed.xml",
        vec![xml(
            r#"<?xml version="1.0"?><rss><channel><title>No Enclosure Show</title>
               <item><guid>text-only</guid><title>Text Only Episode</title></item>
               </channel></rss>"#,
        )],
    )]);
    let db = common::TestDb::create("podcast_no_enclosure").await;
    let owner = seed_owner(&db, "no-enclosure").await;
    // Auto-download enabled: only the missing enclosure may keep the row
    // out of the download queue.
    let channel = seed_channel(&db, owner, &server.url("/feed.xml"), "No Enclosure", true).await;
    refresh_channel(&db.pool, owner, channel, true)
        .await
        .expect("well-formed feed refreshes");
    let (status, enclosure): (String, Option<String>) = sqlx::query_as(
        "SELECT download_status,enclosure_url FROM native_podcast_episodes WHERE channel_id=$1",
    )
    .bind(channel)
    .fetch_one(&db.pool)
    .await
    .expect("episode");
    assert_eq!(status, "NotDownloaded");
    assert!(enclosure.is_none());
    db.drop().await;
}

// ---- Enclosure download fixtures -------------------------------------------

#[tokio::test]
async fn enclosure_redirect_and_errors_fail_the_episode_with_backoff() {
    for status in [302_u16, 404, 500] {
        let script = if status == 302 {
            Script::Redirect {
                status,
                location: "https://cdn.example.test/moved.mp3".to_string(),
            }
        } else {
            Script::Respond {
                status,
                content_type: "text/plain",
                body: b"nope".to_vec(),
            }
        };
        let server = FixtureServer::start(vec![("/media.mp3", vec![script])]);
        let db = common::TestDb::create(&format!("podcast_enclosure_{status}")).await;
        let owner = seed_owner(&db, &format!("enclosure-{status}")).await;
        let channel = seed_channel(
            &db,
            owner,
            "https://feeds.example.test/unused",
            "Show",
            false,
        )
        .await;
        let episode = seed_queued_episode(&db, channel, &server.url("/media.mp3")).await;
        let root = tempfile::tempdir().expect("storage root");

        let completed = run_podcast_once(&db.pool, &worker_config(root.path(), 3))
            .await
            .expect("pass");
        assert_eq!(completed, 0, "status {status} cannot complete a download");
        let (status_row, attempts, error, local_path, _) = episode_row(&db, episode).await;
        assert_eq!(status_row, "Queued", "retryable below max_attempts");
        assert_eq!(attempts, 1);
        assert!(
            error.as_deref().is_some_and(|value| !value.is_empty()),
            "the failure reason is recorded for operators"
        );
        assert!(local_path.is_none(), "no bytes were published");
        assert_eq!(tmp_files(root.path()), 0, "no temporary bytes leak");
        db.drop().await;
    }
}

#[tokio::test]
async fn enclosure_partial_content_is_fail_closed() {
    let server = FixtureServer::start(vec![(
        "/media.mp3",
        vec![Script::Respond {
            status: 206,
            content_type: "audio/mpeg",
            body: b"ID3-partial".to_vec(),
        }],
    )]);
    let db = common::TestDb::create("podcast_enclosure_206").await;
    let owner = seed_owner(&db, "enclosure-206").await;
    let channel = seed_channel(
        &db,
        owner,
        "https://feeds.example.test/unused",
        "Show",
        false,
    )
    .await;
    let episode = seed_queued_episode(&db, channel, &server.url("/media.mp3")).await;
    let root = tempfile::tempdir().expect("storage root");

    let completed = run_podcast_once(&db.pool, &worker_config(root.path(), 3))
        .await
        .expect("pass");
    assert_eq!(completed, 0, "a 206 enclosure must never publish");
    let (status, attempts, _, local_path, _) = episode_row(&db, episode).await;
    assert_eq!(status, "Queued");
    assert_eq!(attempts, 1);
    assert!(local_path.is_none());
    assert_eq!(tmp_files(root.path()), 0);
    db.drop().await;
}

#[tokio::test]
async fn mid_body_interruption_retries_to_byte_identical_success() {
    let payload: Vec<u8> = (0..64 * 1024_u32).flat_map(u32::to_le_bytes).collect();
    let partial = payload[..payload.len() / 4].to_vec();
    let server = FixtureServer::start(vec![(
        "/media.mp3",
        vec![
            Script::Truncate {
                declared_len: payload.len(),
                body: partial,
            },
            Script::Respond {
                status: 200,
                content_type: "audio/mpeg",
                body: payload.clone(),
            },
        ],
    )]);
    let db = common::TestDb::create("podcast_interrupted").await;
    let owner = seed_owner(&db, "interrupted").await;
    let channel = seed_channel(
        &db,
        owner,
        "https://feeds.example.test/unused",
        "Show",
        false,
    )
    .await;
    let episode = seed_queued_episode(&db, channel, &server.url("/media.mp3")).await;
    let root = tempfile::tempdir().expect("storage root");
    let config = worker_config(root.path(), 3);

    // First pass: the connection drops after a quarter of the declared body.
    let completed = run_podcast_once(&db.pool, &config).await.expect("pass one");
    assert_eq!(completed, 0, "an interrupted body never publishes");
    let (status, attempts, error, local_path, _) = episode_row(&db, episode).await;
    assert_eq!(status, "Queued", "the episode is scheduled for retry");
    assert_eq!(attempts, 1);
    assert!(error.is_some(), "the interruption reason is recorded");
    assert!(local_path.is_none());
    assert_eq!(
        tmp_files(root.path()),
        0,
        "orphan reconciliation removes the interrupted temporary file"
    );

    // Skip the exponential backoff so the retry is claimed this test run.
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_available_at=now() WHERE public_api_id=$1",
    )
    .bind(episode)
    .execute(&db.pool)
    .await
    .expect("retry now");
    let completed = run_podcast_once(&db.pool, &config).await.expect("pass two");
    assert_eq!(completed, 1, "the retry completes");
    assert_eq!(server.hits("/media.mp3"), 2, "exactly two wire attempts");
    let (status, attempts, error, local_path, bytes) = episode_row(&db, episode).await;
    assert_eq!(status, "Downloaded");
    assert_eq!(attempts, 1, "a success does not increment attempts");
    assert!(error.is_none(), "success clears the recorded failure");
    assert_eq!(bytes, payload.len() as i64);
    let local_path = local_path.expect("published path");
    assert_eq!(
        std::fs::read(&local_path).expect("published bytes"),
        payload,
        "published bytes are byte-identical to the enclosure"
    );
    db.drop().await;
}

#[tokio::test]
async fn failure_at_max_attempts_is_terminal_and_diagnosable() {
    let server = FixtureServer::start(vec![(
        "/media.mp3",
        vec![Script::Respond {
            status: 500,
            content_type: "text/plain",
            body: b"permanent".to_vec(),
        }],
    )]);
    let db = common::TestDb::create("podcast_max_attempts").await;
    let owner = seed_owner(&db, "max-attempts").await;
    let channel = seed_channel(
        &db,
        owner,
        "https://feeds.example.test/unused",
        "Show",
        false,
    )
    .await;
    let episode = seed_queued_episode(&db, channel, &server.url("/media.mp3")).await;
    let root = tempfile::tempdir().expect("storage root");

    let completed = run_podcast_once(&db.pool, &worker_config(root.path(), 1))
        .await
        .expect("pass");
    assert_eq!(completed, 0);
    let (status, attempts, error, _, _) = episode_row(&db, episode).await;
    assert_eq!(
        status, "Failed",
        "max_attempts=1 makes the first failure terminal"
    );
    assert_eq!(attempts, 1);
    assert!(error.is_some(), "a terminal failure keeps its diagnosis");

    // A terminal row is never reclaimed by later passes.
    let completed = run_podcast_once(&db.pool, &worker_config(root.path(), 1))
        .await
        .expect("second pass");
    assert_eq!(completed, 0);
    assert_eq!(
        server.hits("/media.mp3"),
        1,
        "no retry traffic after the terminal failure"
    );
    db.drop().await;
}
