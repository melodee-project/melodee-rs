//! catalog browsing + detail API integration tests: the
//! administrator gate, the paginated Artists/Releases/Tracks browsing
//! envelopes with filters and whitelist sorts, and the bounded
//! Artist/Release/Track detail payloads (identity, aliases, stats, releases,
//! contributions, relationships, media, provenance, classification, history).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use common::{TestDb, test_config};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_domain::{Disposition, IngestionState, PolicyRuleSetId};
use melodee_persistence::{AuthStore, NativeUserImport, PolicyStore, WorkerId};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_catalog::AdminCatalogState;
use melodee_server::native::stream::DatabaseStreamUrlSigner;
use melodee_server::native::{NativeState, PostgresNativeUserStateReader};
use melodee_server::native_stream::{CanonicalAssetRoot, ClientAddressResolver, DirectPlayService};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-catalog-jwt-key-least-32b";
/// The signer embeds this origin; tests strip it before replaying the URL
/// against the in-process native router.
const PUBLIC_ORIGIN: &str = "http://melodee.test";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
    /// Administrator without the streaming role: the signer fails closed and
    /// preview URLs must be absent (PRD-UI-007).
    no_stream_admin_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    let no_stream_admin_public = Uuid::now_v7();
    for (username, is_admin, public_id, roles) in [
        (
            "wp15-admin",
            true,
            admin_public,
            vec!["HasStreamRole".to_string()],
        ),
        (
            "wp15-plain",
            false,
            plain_public,
            vec!["HasStreamRole".to_string()],
        ),
        ("wp15-no-stream-admin", true, no_stream_admin_public, vec![]),
    ] {
        let email = format!("{username}@example.test");
        AuthStore::new()
            .import_user(
                &db.pool,
                &NativeUserImport {
                    id: Uuid::now_v7(),
                    public_api_id: public_id,
                    username,
                    normalized_username: &username.to_uppercase(),
                    email: &email,
                    normalized_email: &email.to_uppercase(),
                    password_hash: &bcrypt::hash("Mel0deeR0cks!", 4).expect("fixture hash"),
                    password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                    stream_signing_key: "test-stream-signing-key",
                    is_admin,
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
            .expect("import user");
    }
    Fixture {
        admin_public,
        plain_public,
        no_stream_admin_public,
    }
}

fn token_for(public_api_id: Uuid, username: &str) -> String {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    issue_access_token(
        &SecretString::new(JWT_KEY.to_string()),
        &NativeAccessClaims {
            aud: "MelodeeClient".to_string(),
            email: format!("{username}@example.test"),
            exp: now + 900,
            sid: public_api_id,
            iat: now,
            iss: "MelodeeApi".to_string(),
            nbf: now,
            role: vec![],
            unique_name: username.to_string(),
            theme: String::new(),
        },
    )
    .expect("issue token")
}

fn admin_state(pool: sqlx::PgPool) -> AdminAccessState {
    AdminAccessState {
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool,
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
    }
}

fn app(pool: sqlx::PgPool) -> axum::Router {
    melodee_server::admin_catalog::router().with_state(AdminCatalogState::new(
        admin_state(pool.clone()),
        Arc::new(DatabaseStreamUrlSigner::new(pool, PUBLIC_ORIGIN)),
        ClientAddressResolver::default(),
    ))
}

/// The detail handlers resolve the token client binding from ConnectInfo,
/// exactly as the production server does for every request.
const TEST_PEER: &str = "127.0.0.1:41000";

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize"))
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(TEST_PEER.parse::<SocketAddr>().expect("peer")));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, json)
}

/// Seeds one cataloged Release and returns the fixture.
async fn seed_release(
    db: &TestDb,
    ordinal: u32,
    release_title: &'static str,
    artist: &'static str,
    track_title: &'static str,
) -> phase5_common::CatalogedPolicyFixture {
    phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title,
            release_artist: artist,
            track_title,
            track_artist: artist,
            release_date: "2007-08-09",
        },
    )
    .await
}

/// Resolves the public ids of the seeded Release, its billed (release-scoped)
/// Artist, and its Track.
async fn public_ids(db: &TestDb, release_id: Uuid) -> (Uuid, Uuid, Uuid) {
    let (release_public, track_public): (Uuid, Uuid) = sqlx::query_as(
        "SELECT r.public_api_id, t.public_api_id FROM releases r
         JOIN tracks t ON t.release_id = r.id WHERE r.id = $1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("release/track public ids");
    let artist_public: Uuid = sqlx::query_scalar(
        "SELECT a.public_api_id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id
         WHERE ra.release_id = $1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("artist public id");
    (release_public, artist_public, track_public)
}

#[tokio::test]
async fn catalog_routes_require_administrator() {
    let db = TestDb::create("wp15_catalog_gate").await;
    let fixture = seed_users(&db).await;
    let app = app(db.pool.clone());
    let plain = token_for(fixture.plain_public, "wp15-plain");
    let admin = token_for(fixture.admin_public, "wp15-admin");

    for path in [
        "/api/admin/v1/catalog/artists",
        "/api/admin/v1/catalog/releases",
        "/api/admin/v1/catalog/tracks",
        &format!("/api/admin/v1/catalog/artists/{}", Uuid::now_v7()),
        &format!("/api/admin/v1/catalog/releases/{}", Uuid::now_v7()),
        &format!("/api/admin/v1/catalog/tracks/{}", Uuid::now_v7()),
    ] {
        let (status, _) = call(app.clone(), "GET", path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        let (status, _) = call(app.clone(), "GET", path, None, Some(&plain)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        let (status, _) = call(app.clone(), "GET", path, None, Some(&admin)).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::NOT_FOUND,
            "{path}: {status}"
        );
    }

    db.drop().await;
}

#[tokio::test]
async fn artist_browsing_envelope_filters_and_whitelist_sort() {
    let db = TestDb::create("wp15_artist_browse").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(&db, 1, "Browse Release", "Browse Artist", "Browse Track").await;
    let (_, artist_public, _) = public_ids(&db, release.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/artists",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // One release-scoped and one track-scoped provisional Artist.
    assert_eq!(body["total"], 2);
    assert_eq!(body["limit"], 50);
    assert_eq!(body["offset"], 0);
    let item = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["id"].as_str() == Some(artist_public.to_string().as_str()))
        .expect("billed artist row");
    assert_eq!(item["name"], "Browse Artist");
    assert_eq!(item["sortName"], "Browse Artist");
    assert_eq!(item["artistType"], "unknown");
    assert_eq!(item["status"], "provisional");
    assert_eq!(item["releaseCount"], 1);
    assert_eq!(item["trackCount"], 1);
    assert_eq!(
        item["imageId"].as_str().expect("imageId"),
        format!("artist_{artist_public}")
    );

    // Substring filter, then the strict sort whitelist.
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/artists?query=nothing-matches-this",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/artists?sort=bogus",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_sort");
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/artists?query={}", "x".repeat(201)),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_query");

    // The limit clamps at the server maximum and reports the clamp honestly.
    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/catalog/artists?limit=10000",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["limit"], 500);

    db.drop().await;
}

#[tokio::test]
async fn release_browsing_filters_by_query_and_artist() {
    let db = TestDb::create("wp15_release_browse").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let first = seed_release(&db, 2, "Browse Alpha", "Browse Artist A", "Track A").await;
    let _second = seed_release(&db, 3, "Browse Beta", "Browse Artist B", "Track B").await;
    let (release_public, artist_public, _) = public_ids(&db, first.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/releases?sort=title:asc",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    let item = &body["items"][0];
    assert_eq!(item["title"], "Browse Alpha");
    assert_eq!(item["displayArtist"], "Browse Artist A");
    assert_eq!(item["releaseDate"], "2007-08-09");
    assert_eq!(item["status"], "active");
    assert_eq!(item["trackCount"], 1);
    assert_eq!(item["durationMs"], 180_000);
    assert_eq!(
        item["imageId"].as_str().expect("imageId"),
        format!("album_{release_public}")
    );

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/releases?query=beta",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["title"], "Browse Beta");

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/releases?artistId={artist_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["title"], "Browse Alpha");

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/catalog/releases?sort=title:sideways",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_sort");

    db.drop().await;
}

#[tokio::test]
async fn track_browsing_filters_by_release_and_query() {
    let db = TestDb::create("wp15_track_browse").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let first = seed_release(&db, 4, "Track Browse A", "Track Artist A", "First Title").await;
    let _second = seed_release(&db, 5, "Track Browse B", "Track Artist B", "Second Title").await;
    let (release_public, _, track_public) = public_ids(&db, first.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/tracks",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    let item = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["id"].as_str() == Some(track_public.to_string().as_str()))
        .expect("track row");
    assert_eq!(item["title"], "First Title");
    assert_eq!(
        item["releaseId"].as_str(),
        Some(release_public.to_string().as_str())
    );
    assert_eq!(item["releaseTitle"], "Track Browse A");
    assert_eq!(item["durationMs"], 180_000);
    assert_eq!(
        item["imageId"].as_str().expect("imageId"),
        format!("song_{track_public}")
    );

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/tracks?releaseId={release_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/tracks?query=second",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/catalog/tracks?sort=album:asc",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_sort");

    db.drop().await;
}

#[tokio::test]
async fn artist_detail_payload_and_not_found() {
    let db = TestDb::create("wp15_artist_detail").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(&db, 6, "Detail Release", "Detail Artist", "Detail Track").await;
    let (_, artist_public, _) = public_ids(&db, release.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/artists/{artist_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Detail Artist");
    assert_eq!(body["status"], "provisional");
    assert_eq!(body["artistType"], "unknown");
    assert_eq!(body["stats"]["releaseCount"], 1);
    assert_eq!(body["stats"]["performanceTrackCount"], 1);
    assert_eq!(body["aliases"]["total"], 0);
    assert_eq!(body["externalIds"]["total"], 0);
    assert_eq!(body["releases"]["total"], 1);
    assert_eq!(body["releases"]["items"][0]["title"], "Detail Release");
    assert_eq!(body["releases"]["items"][0]["role"], "primary");
    assert_eq!(body["relationships"]["total"], 0);
    assert_eq!(body["memberships"]["total"], 0);
    assert_eq!(body["history"]["total"], 0);
    assert_eq!(
        body["imageId"].as_str().expect("imageId"),
        format!("artist_{artist_public}")
    );

    let (status, body) = call(
        app,
        "GET",
        &format!("/api/admin/v1/catalog/artists/{}", Uuid::now_v7()),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    db.drop().await;
}

#[tokio::test]
async fn release_detail_payload_includes_classification_provenance_history() {
    let db = TestDb::create("wp15_release_detail").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(&db, 7, "Detail Release", "Detail Artist", "Detail Track").await;
    let (release_public, artist_public, track_public) =
        public_ids(&db, release.release_id().into_uuid()).await;

    // Drive the cataloged fixture through policy evaluation so the durable
    // classification exists.
    let store = PolicyStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let mut claims = store
        .claim_cataloged(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim cataloged fixture");
    let claim = claims.pop().expect("one claim");
    let context = store
        .load_context(&db.pool, &claim)
        .await
        .expect("load context");
    let rule_set = phase5_common::policy_rule_set(PolicyRuleSetId::new(), now);
    store
        .ensure_rule_set(&db.pool, &rule_set, now)
        .await
        .expect("persist rules");
    let committed = store
        .commit_evaluation(
            &db.pool,
            &worker,
            &phase5_common::unknown_review_commit(&context, rule_set.id, now),
        )
        .await
        .expect("commit evaluation");
    assert_eq!(committed.item.state, IngestionState::NeedsReview);
    assert_eq!(committed.decision.disposition, Disposition::Review);

    let app = app(db.pool.clone());
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/releases/{release_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Detail Release");
    assert_eq!(body["releaseDate"], "2007-08-09");
    assert_eq!(body["status"], "active");
    assert_eq!(body["trackCount"], 1);
    assert!(body["mediaBytes"].as_i64().expect("mediaBytes") > 0);
    assert_eq!(body["durationMs"], 180_000);

    let tracks = body["tracks"].as_object().expect("tracks section");
    assert_eq!(tracks["total"], 1);
    let track = &tracks["items"][0];
    assert_eq!(
        track["id"].as_str(),
        Some(track_public.to_string().as_str())
    );
    assert_eq!(track["title"], "Detail Track");
    assert_eq!(track["ordinal"], 1);
    assert_eq!(track["container"], "mp3");
    assert_eq!(track["codec"], "mp3");
    assert_eq!(track["durationMs"], 180_000);

    let artists = body["artists"].as_array().expect("artists");
    assert_eq!(artists.len(), 1);
    assert_eq!(
        artists[0]["artistId"].as_str(),
        Some(artist_public.to_string().as_str())
    );
    assert_eq!(artists[0]["role"], "primary");

    let media = body["mediaAssets"].as_object().expect("media section");
    assert_eq!(media["total"], 1);
    assert_eq!(media["items"][0]["mimeType"], "audio/mpeg");
    assert_eq!(media["items"][0]["container"], "mp3");

    let classification = &body["classification"];
    assert_eq!(
        classification["traits"].as_array().expect("traits"),
        &vec![Value::String("unknown".to_string())]
    );
    assert_eq!(
        classification["classifierVersion"],
        "melodee.policy.classifier-v1"
    );

    let provenance = &body["provenance"];
    assert_eq!(
        provenance["workItemId"].as_str(),
        Some(release.work_item_id().to_string().as_str())
    );
    assert_eq!(provenance["manifestSchemaVersion"], 3);

    let history = body["history"].as_object().expect("history section");
    assert!(
        history["items"]
            .as_array()
            .expect("history items")
            .iter()
            .any(|event| event["eventType"] == "manifest_v3_imported")
    );

    let (status, body) = call(
        app,
        "GET",
        &format!("/api/admin/v1/catalog/releases/{}", Uuid::now_v7()),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    db.drop().await;
}

#[tokio::test]
async fn track_detail_payload_and_not_found() {
    let db = TestDb::create("wp15_track_detail").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(
        &db,
        8,
        "Track Detail Release",
        "Track Artist",
        "Track Title",
    )
    .await;
    let (release_public, _artist_public, track_public) =
        public_ids(&db, release.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/tracks/{track_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Track Title");
    assert_eq!(
        body["releaseId"].as_str(),
        Some(release_public.to_string().as_str())
    );
    assert_eq!(body["releaseTitle"], "Track Detail Release");
    assert_eq!(body["durationMs"], 180_000);
    assert_eq!(
        body["imageId"].as_str().expect("imageId"),
        format!("song_{track_public}")
    );
    assert!(body["audioIdentityId"].is_string());

    // The manifest observed a track-scoped artist, so the track-level billing
    // is a distinct provisional Artist from the release-level one.
    let artists = body["artists"].as_array().expect("artists");
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0]["name"], "Track Artist");
    assert_eq!(artists[0]["role"], "primary");

    let media = body["mediaAssets"].as_array().expect("media assets");
    assert_eq!(media.len(), 1);
    assert_eq!(media[0]["purpose"], "playable");
    assert_eq!(media[0]["container"], "mp3");
    assert_eq!(media[0]["sampleRateHz"], 48_000);

    let (status, body) = call(
        app,
        "GET",
        &format!("/api/admin/v1/catalog/tracks/{}", Uuid::now_v7()),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    db.drop().await;
}

/// Native router state for replaying signed stream URLs: the real direct-play
/// service over a temporary canonical root holding the fixture's bytes.
fn native_streaming_state(pool: &sqlx::PgPool, root: &std::path::Path) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        pool: pool.clone(),
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        search: None,
        public_origin: PUBLIC_ORIGIN.to_owned(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast client"),
        stream_url_signer: Arc::new(DatabaseStreamUrlSigner::new(pool.clone(), PUBLIC_ORIGIN)),
        user_state: Arc::new(PostgresNativeUserStateReader::new(pool.clone())),
        direct_play: DirectPlayService::new(
            CanonicalAssetRoot::open(root).expect("canonical root"),
            2,
        ),
        client_addresses: ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth"),
        ),
    }
}

async fn replay_stream_url(
    app: axum::Router,
    stream_url: &str,
    range: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let path = stream_url
        .strip_prefix(PUBLIC_ORIGIN)
        .expect("signed URL carries the configured public origin");
    let mut builder = Request::builder().uri(path);
    if let Some(range) = range {
        builder = builder.header(header::RANGE, range);
    }
    let mut request = builder.body(Body::empty()).expect("request");
    // The same client address the admin detail request was signed for.
    request
        .extensions_mut()
        .insert(ConnectInfo(TEST_PEER.parse::<SocketAddr>().expect("peer")));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, body)
}

#[tokio::test]
async fn track_detail_stream_url_is_signed_for_the_admin_and_actually_streams() {
    let db = TestDb::create("wp15_track_stream_url").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(&db, 9, "Stream Release", "Stream Artist", "Stream Track").await;
    let (_, _, track_public) = public_ids(&db, release.release_id().into_uuid()).await;

    let (status, body) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/catalog/tracks/{track_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stream_url = body["streamUrl"].as_str().expect("signed streamUrl");
    let prefix = format!(
        "{PUBLIC_ORIGIN}/song/stream/{track_public}/{}/",
        fixture.admin_public
    );
    assert!(
        stream_url.starts_with(&prefix),
        "streamUrl uses the frozen native stream route: {stream_url}"
    );

    // Replay the signed URL against the native stream route backed by the
    // fixture's canonical bytes: it must stream (200 full, 206 ranged). The
    // file lives at the catalog-recorded canonical relative path.
    let canonical_relative_path: String = sqlx::query_scalar(
        "SELECT m.canonical_relative_path FROM tracks t
         JOIN media_assets m ON m.id = t.playable_media_asset_id
         WHERE t.public_api_id = $1",
    )
    .bind(track_public)
    .fetch_one(&db.pool)
    .await
    .expect("canonical media path");
    let temp = tempfile::tempdir().expect("stream root");
    let media_path = temp.path().join(&canonical_relative_path);
    std::fs::create_dir_all(media_path.parent().expect("media parent")).expect("media directory");
    let media_bytes = [0x4d_u8; 42];
    std::fs::write(&media_path, media_bytes).expect("media bytes");
    let stream_app =
        melodee_server::native::router().with_state(native_streaming_state(&db.pool, temp.path()));

    let (status, body) = replay_stream_url(stream_app.clone(), stream_url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, media_bytes);

    let (status, body) = replay_stream_url(stream_app, stream_url, Some("bytes=0-9")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, &media_bytes[..10]);

    db.drop().await;
}

#[tokio::test]
async fn release_detail_track_rows_carry_signed_preview_urls() {
    let db = TestDb::create("wp15_release_stream_url").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp15-admin");
    let release = seed_release(
        &db,
        10,
        "Stream List Release",
        "Stream List Artist",
        "Stream List Track",
    )
    .await;
    let (release_public, _, track_public) = public_ids(&db, release.release_id().into_uuid()).await;

    let (status, body) = call(
        app(db.pool.clone()),
        "GET",
        &format!("/api/admin/v1/catalog/releases/{release_public}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let track = &body["tracks"]["items"][0];
    assert_eq!(
        track["id"].as_str(),
        Some(track_public.to_string().as_str())
    );
    let prefix = format!(
        "{PUBLIC_ORIGIN}/song/stream/{track_public}/{}/",
        fixture.admin_public
    );
    let stream_url = track["streamUrl"].as_str().expect("signed streamUrl");
    assert!(
        stream_url.starts_with(&prefix),
        "track row streamUrl uses the frozen native stream route: {stream_url}"
    );

    db.drop().await;
}

#[tokio::test]
async fn stream_urls_are_absent_when_the_admin_is_not_stream_authorized() {
    let db = TestDb::create("wp15_stream_url_fail_closed").await;
    let fixture = seed_users(&db).await;
    // Administrator gate passes (is_admin), but without HasStreamRole the
    // signer fails closed and both DTOs must honestly carry null.
    let no_stream = token_for(fixture.no_stream_admin_public, "wp15-no-stream-admin");
    let release = seed_release(
        &db,
        11,
        "No Stream Release",
        "No Stream Artist",
        "No Stream Track",
    )
    .await;
    let (release_public, _, track_public) = public_ids(&db, release.release_id().into_uuid()).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/catalog/tracks/{track_public}"),
        None,
        Some(&no_stream),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["streamUrl"], Value::Null);

    let (status, body) = call(
        app,
        "GET",
        &format!("/api/admin/v1/catalog/releases/{release_public}"),
        None,
        Some(&no_stream),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tracks"]["items"][0]["streamUrl"], Value::Null);

    db.drop().await;
}
