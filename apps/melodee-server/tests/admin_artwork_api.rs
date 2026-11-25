//! ADR-0143 catalog artwork override integration evidence (parity matrix
//! artwork row, checklist 07): the admin PUT/DELETE image endpoints, the
//! native `/images/{kind}_{id}/{size}` precedence change (override served;
//! plan/placeholder restored after removal), upload validation (400 for
//! invalid/truncated/oversized bodies), the 401/403 administrator gate, 404
//! unknown ids, audit attribution, and the proof that no canonical media
//! file is ever deleted.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use melodee_server::admin_catalog::AdminCatalogState;
use melodee_server::native::stream::DatabaseStreamUrlSigner;
use melodee_server::native::{NativeState, PostgresNativeUserStateReader};
use melodee_server::native_stream::ClientAddressResolver;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-artwork-jwt-key-least-32b";
const PUBLIC_ORIGIN: &str = "http://melodee.test";

const PLACEHOLDER_ALBUM: &[u8] = include_bytes!(
    "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin"
);
const PLACEHOLDER_ARTIST: &[u8] = include_bytes!(
    "../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin"
);

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp-art-admin", true, admin_public),
        ("wp-art-plain", false, plain_public),
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
                    roles: &["HasStreamRole".to_string()],
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

fn native_state(pool: sqlx::PgPool) -> NativeState {
    NativeState {
        lastfm_api_base_url: None,
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        search: None,
        public_origin: PUBLIC_ORIGIN.to_string(),
        podcast_config: Arc::new(melodee_server::podcast_worker::PodcastConfig::default()),
        podcast_discovery: melodee_server::native::podcasts::PodcastDiscoveryClient::itunes()
            .expect("podcast discovery"),
        stream_url_signer: Arc::new(DatabaseStreamUrlSigner::new(pool.clone(), PUBLIC_ORIGIN)),
        user_state: Arc::new(PostgresNativeUserStateReader::new(pool.clone())),
        direct_play: None,
        client_addresses: ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
        pool,
    }
}

/// The admin catalog surface and the anonymous native image endpoint in one
/// in-process app, so an upload round trip is served exactly as production
/// serves it.
fn app(pool: sqlx::PgPool) -> axum::Router {
    let admin = melodee_server::admin_catalog::router().with_state(AdminCatalogState::new(
        AdminAccessState {
            notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
                pool.clone(),
            )),
            scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
                pool.clone(),
            )),
            pool: pool.clone(),
            auth: Arc::new(
                melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                    .expect("auth config"),
            ),
        },
        Arc::new(DatabaseStreamUrlSigner::new(pool.clone(), PUBLIC_ORIGIN)),
        ClientAddressResolver::default(),
    ));
    let images = melodee_server::native::images::router().with_state(native_state(pool));
    admin.merge(images)
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    bearer: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder =
        Request::builder()
            .method(method)
            .uri(path)
            .extension(axum::extract::ConnectInfo(SocketAddr::from((
                [127, 0, 0, 1],
                12345,
            ))));
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let response = app
        .oneshot(builder.body(Body::from(body)).expect("request"))
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

/// Deterministic decodable PNG upload (gradient so re-encodes differ from
/// every placeholder).
fn png_upload(width: u32, height: u32) -> Vec<u8> {
    let image =
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, 90, 255])
        }));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .expect("encode upload fixture");
    encoded.into_inner()
}

fn jpeg_dimensions(bytes: &[u8]) -> (u32, u32) {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .expect("served image format")
        .into_dimensions()
        .expect("served image dimensions")
}

/// Seeds one cataloged Release and returns (release, artist, track) public
/// ids plus the fixture (for filesystem assertions).
async fn seed_catalog(
    db: &TestDb,
    ordinal: u32,
) -> (phase5_common::CatalogedPolicyFixture, Uuid, Uuid, Uuid) {
    let fixture = phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: "Artwork Release",
            release_artist: "Artwork Artist",
            track_title: "Artwork Track",
            track_artist: "Artwork Artist",
            release_date: "2007-08-09",
        },
    )
    .await;
    let release_id = fixture.release_id().into_uuid();
    let release_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
            .bind(release_id)
            .fetch_one(&db.pool)
            .await
            .expect("release public id");
    let track_public: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(release_id)
            .fetch_one(&db.pool)
            .await
            .expect("track public id");
    let artist_public: Uuid = sqlx::query_scalar(
        "SELECT a.public_api_id FROM artists a
         JOIN release_artists ra ON ra.artist_id = a.id WHERE ra.release_id = $1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("artist public id");
    (fixture, release_public, artist_public, track_public)
}

#[tokio::test]
async fn artwork_routes_require_administrator() {
    let db = TestDb::create("wp_art_gate").await;
    let fixture = seed_users(&db).await;
    let app = app(db.pool.clone());
    let plain = token_for(fixture.plain_public, "wp-art-plain");
    let body = png_upload(64, 64);
    let id = Uuid::now_v7();
    for path in [
        format!("/api/admin/v1/catalog/artists/{id}/image"),
        format!("/api/admin/v1/catalog/releases/{id}/image"),
    ] {
        let (status, _, _) = call(
            app.clone(),
            "PUT",
            &path,
            Some("image/png"),
            body.clone(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "PUT {path}");
        let (status, _, _) = call(
            app.clone(),
            "PUT",
            &path,
            Some("image/png"),
            body.clone(),
            Some(&plain),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "PUT {path}");
        let (status, _, _) = call(app.clone(), "DELETE", &path, None, vec![], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "DELETE {path}");
        let (status, _, _) = call(app.clone(), "DELETE", &path, None, vec![], Some(&plain)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "DELETE {path}");
    }
    db.drop().await;
}

#[tokio::test]
async fn unknown_entity_ids_return_404_on_all_four_endpoints() {
    let db = TestDb::create("wp_art_not_found").await;
    let fixture = seed_users(&db).await;
    let app = app(db.pool.clone());
    let admin = token_for(fixture.admin_public, "wp-art-admin");
    let body = png_upload(64, 64);
    for kind in ["artists", "releases"] {
        let path = format!("/api/admin/v1/catalog/{kind}/{}/image", Uuid::now_v7());
        // Entity resolution precedes body validation, so even a valid upload
        // to an unknown id is 404.
        let (status, _, _) = call(
            app.clone(),
            "PUT",
            &path,
            Some("image/png"),
            body.clone(),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "PUT {path}");
        let (status, _, _) = call(app.clone(), "DELETE", &path, None, vec![], Some(&admin)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "DELETE {path}");
    }
    db.drop().await;
}

#[tokio::test]
async fn upload_serves_override_and_delete_restores_placeholder() {
    let db = TestDb::create("wp_art_round_trip").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp-art-admin");
    // Ordinal 7 gives this test a fixture-unique canonical path: the other
    // tests share ordinal 1 and the fixtures assert their canonical paths do
    // not exist, so the sentinel below must live on its own path.
    let (catalog, release_public, _artist_public, track_public) = seed_catalog(&db, 7).await;
    let app = app(db.pool.clone());

    // Baseline: no override → the captured placeholder bytes (frozen branch).
    let album_path = format!("/images/album_{release_public}/80");
    let (status, headers, bytes) = call(app.clone(), "GET", &album_path, None, vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, PLACEHOLDER_ALBUM, "baseline must be the placeholder");
    let placeholder_etag = headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("placeholder etag")
        .to_string();

    // Canonical sentinel: the deletion contract must never remove it.
    let canonical_path = std::path::PathBuf::from(&catalog.published.canonical_asset_path);
    let _ = std::fs::remove_file(&canonical_path); // tolerate a previous failed run
    std::fs::create_dir_all(canonical_path.parent().expect("canonical parent")).expect("mkdir");
    std::fs::write(
        &canonical_path,
        b"canonical bytes the override must never touch",
    )
    .expect("write canonical sentinel");

    // Square upload: the serving pipeline's aspect-preserving resize then
    // yields exactly the requested size.
    let upload = png_upload(320, 320);
    let path = format!("/api/admin/v1/catalog/releases/{release_public}/image");
    let (status, _, body) = call(
        app.clone(),
        "PUT",
        &path,
        Some("image/png"),
        upload.clone(),
        Some(&admin),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PUT: {}",
        String::from_utf8_lossy(&body)
    );
    let body: serde_json::Value = serde_json::from_slice(&body).expect("mutation body");
    assert_eq!(body["imageId"], format!("album_{release_public}"));

    // Override precedence through the frozen endpoint, for both the Release
    // (album_) and its Track (song_) kinds.
    for image_path in [&album_path, &format!("/images/song_{track_public}/80")] {
        let (status, headers, bytes) =
            call(app.clone(), "GET", image_path, None, vec![], None).await;
        assert_eq!(status, StatusCode::OK, "GET {image_path}");
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/jpeg")
        );
        assert_eq!(jpeg_dimensions(&bytes), (80, 80), "override is resized");
        assert_ne!(bytes, PLACEHOLDER_ALBUM, "override must be served");
        let etag = headers
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .expect("override etag")
            .to_string();
        assert_ne!(etag, placeholder_etag, "ETag is content-derived");
        // Conditional revalidation against the override bytes.
        let mut builder = Request::builder().uri(image_path);
        builder = builder.header(header::IF_NONE_MATCH, &etag);
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    // Idempotent removal restores the fallback branch byte-for-byte.
    let (status, _, _) = call(app.clone(), "DELETE", &path, None, vec![], Some(&admin)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, bytes) = call(app.clone(), "GET", &album_path, None, vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        bytes, PLACEHOLDER_ALBUM,
        "removal must restore the placeholder branch"
    );
    let (status, _, _) = call(app.clone(), "DELETE", &path, None, vec![], Some(&admin)).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "deleting an absent override is idempotent (ADR-0143 §4)"
    );

    // The canonical file and the publication plan survived every mutation.
    assert_eq!(
        std::fs::read(&canonical_path).expect("canonical sentinel"),
        b"canonical bytes the override must never touch"
    );
    let plan_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM release_publication_plans plan
         JOIN catalog_imports ci ON ci.work_item_id = plan.work_item_id
         WHERE ci.release_id = $1",
    )
    .bind(catalog.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("plan count");
    assert_eq!(plan_count, 1, "publication plan must be untouched");
    let asset_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM track_media_assets link
         JOIN tracks t ON t.id = link.track_id WHERE t.release_id = $1",
    )
    .bind(catalog.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("asset count");
    assert_eq!(asset_count, 1, "media asset links must be untouched");
    std::fs::remove_file(&canonical_path).expect("cleanup sentinel");
    let _ = std::fs::remove_dir(canonical_path.parent().expect("canonical parent"));

    db.drop().await;
}

#[tokio::test]
async fn artist_override_is_served_and_audit_attributes_the_admin() {
    let db = TestDb::create("wp_art_artist").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp-art-admin");
    let (_catalog, _release_public, artist_public, _track) = seed_catalog(&db, 1).await;
    let app = app(db.pool.clone());

    let artist_path = format!("/images/artist_{artist_public}/160");
    let (_, _, bytes) = call(app.clone(), "GET", &artist_path, None, vec![], None).await;
    assert_eq!(bytes, PLACEHOLDER_ARTIST, "baseline placeholder");

    let path = format!("/api/admin/v1/catalog/artists/{artist_public}/image");
    let (status, _, _) = call(
        app.clone(),
        "PUT",
        &path,
        Some("image/png"),
        png_upload(500, 500),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, bytes) = call(app.clone(), "GET", &artist_path, None, vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jpeg_dimensions(&bytes), (160, 160), "override is resized");
    assert_ne!(bytes, PLACEHOLDER_ARTIST);

    let (status, _, _) = call(
        app.clone(),
        "DELETE",
        &path,
        Some("application/json"),
        serde_json::to_vec(&serde_json::json!({"reason": "wrong artist photo"}))
            .expect("reason body"),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, bytes) = call(app.clone(), "GET", &artist_path, None, vec![], None).await;
    assert_eq!(bytes, PLACEHOLDER_ARTIST, "fallback restored");

    let events: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event_type, actor, reason FROM catalog_audit_events
         WHERE entity_type = 'artist'
           AND entity_id = (SELECT id FROM artists WHERE public_api_id = $1)
           AND event_type LIKE 'image_%'
         ORDER BY created_at, event_id",
    )
    .bind(artist_public)
    .fetch_all(&db.pool)
    .await
    .expect("audit events");
    assert_eq!(
        events,
        vec![
            (
                "image_override_set".to_string(),
                fixture.admin_public.to_string(),
                "operator artwork upload".to_string(),
            ),
            (
                "image_override_removed".to_string(),
                fixture.admin_public.to_string(),
                "wrong artist photo".to_string(),
            ),
        ],
        "every mutation is audit-logged with the acting administrator id"
    );

    db.drop().await;
}

#[tokio::test]
async fn webp_replaces_primary_artwork_for_artist_and_release() {
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

    let db = TestDb::create("art_webp").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp-art-admin");
    let (_, release, artist, track) = seed_catalog(&db, 13).await;
    let app = app(db.pool.clone());
    // Half transparent (with hidden red), half blue, non-square to catch resizing.
    let source = RgbaImage::from_fn(80, 48, |x, _| {
        if x < 40 {
            Rgba([255, 0, 0, 0])
        } else {
            Rgba([0, 0, 255, 255])
        }
    });
    let mut webp = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(source)
        .write_to(&mut webp, ImageFormat::WebP)
        .expect("encode WebP");
    let webp = webp.into_inner();
    for (collection, id, prefix) in [
        ("artists", artist, "artist"),
        ("releases", release, "album"),
    ] {
        let detail = format!("/api/admin/v1/catalog/{collection}/{id}");
        let upload = format!("{detail}/image");
        let primary = format!("/images/{prefix}_{id}/original");
        let (status, _, _) = call(
            app.clone(),
            "PUT",
            &upload,
            Some("image/png"),
            png_upload(64, 64),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, old_headers, _) = call(app.clone(), "GET", &primary, None, vec![], None).await;

        let (status, _, body) = call(
            app.clone(),
            "PUT",
            &upload,
            Some("image/webp"),
            webp.clone(),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let mutation: serde_json::Value = serde_json::from_slice(&body).expect("mutation");
        let (status, _, body) = call(app.clone(), "GET", &detail, None, vec![], Some(&admin)).await;
        assert_eq!(status, StatusCode::OK);
        let detail_body: serde_json::Value = serde_json::from_slice(&body).expect("detail");
        assert!(mutation["updatedAt"].is_string());
        assert_eq!(detail_body["imageUpdatedAt"], mutation["updatedAt"]);

        let (status, headers, bytes) = call(app.clone(), "GET", &primary, None, vec![], None).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(headers[header::ETAG], old_headers[header::ETAG]);
        assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            image::guess_format(&bytes).expect("stored format"),
            ImageFormat::Jpeg
        );
        let pixels = image::load_from_memory(&bytes)
            .expect("decode JPEG")
            .into_rgb8();
        assert_eq!(pixels.dimensions(), (80, 48));
        assert!(pixels.get_pixel(4, 24).0.iter().all(|&v| v >= 245));
        let [r, g, b] = pixels.get_pixel(70, 24).0;
        assert!(r < 10 && g < 10 && b > 245);
        if collection == "releases" {
            let (_, _, inherited) = call(
                app.clone(),
                "GET",
                &format!("/images/song_{track}/original"),
                None,
                vec![],
                None,
            )
            .await;
            assert_eq!(inherited, bytes);
        }

        for (mime, invalid) in [
            ("image/webp", png_upload(4, 4)),
            ("image/png", webp.clone()),
            ("image/webp", webp[..14].to_vec()),
        ] {
            let (status, _, _) = call(
                app.clone(),
                "PUT",
                &upload,
                Some(mime),
                invalid,
                Some(&admin),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (_, _, retained) = call(app.clone(), "GET", &primary, None, vec![], None).await;
        assert_eq!(
            retained, bytes,
            "rejected replacement must preserve the current primary"
        );
        let (status, _, _) = call(app.clone(), "DELETE", &upload, None, vec![], Some(&admin)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, _, body) = call(app.clone(), "GET", &detail, None, vec![], Some(&admin)).await;
        let detail_body: serde_json::Value = serde_json::from_slice(&body).expect("detail");
        assert_eq!(
            detail_body.get("imageUpdatedAt"),
            Some(&serde_json::Value::Null)
        );
    }
    db.drop().await;
}

#[tokio::test]
async fn invalid_truncated_and_oversized_uploads_are_400() {
    let db = TestDb::create("wp_art_invalid").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp-art-admin");
    let (_catalog, release_public, _artist, _track) = seed_catalog(&db, 1).await;
    let app = app(db.pool.clone());
    let path = format!("/api/admin/v1/catalog/releases/{release_public}/image");

    let valid = png_upload(64, 64);
    let dimension_bomb = png_upload(4097, 1);
    let mut oversized = valid.clone();
    oversized.resize(10 * 1024 * 1024 + 1, 0);
    let cases: [(&str, Option<&str>, Vec<u8>); 7] = [
        (
            "non-image bytes",
            Some("image/png"),
            b"not an image".to_vec(),
        ),
        (
            "svg payload",
            Some("image/png"),
            b"<svg onload=alert(1)>".to_vec(),
        ),
        ("empty body", Some("image/png"), vec![]),
        (
            "declared content type mismatch",
            Some("image/jpeg"),
            valid.clone(),
        ),
        ("missing content type", None, valid.clone()),
        ("truncated png", Some("image/png"), valid[..24].to_vec()),
        ("dimension bomb", Some("image/png"), dimension_bomb),
    ];
    for (case, content_type, body) in cases {
        let (status, _, _) =
            call(app.clone(), "PUT", &path, content_type, body, Some(&admin)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
    }
    // Beyond the 10 MiB input bound but inside the route body-limit slack:
    // the pipeline's contract answer is 400, not the limiter's 413.
    let (status, _, _) = call(
        app.clone(),
        "PUT",
        &path,
        Some("image/png"),
        oversized,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "oversized upload");

    // None of the rejections stored an override.
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM catalog_entity_images")
        .fetch_one(&db.pool)
        .await
        .expect("override count");
    assert_eq!(stored, 0);

    db.drop().await;
}

#[tokio::test]
async fn canonical_artwork_with_corrupted_storage_falls_back_to_the_captured_placeholder() {
    let db = TestDb::create("wp_art_hash_fence").await;
    let (catalog, release_public, _artist_public, _track_public) = seed_catalog(&db, 11).await;

    // The policy fixture publishes a media-only plan; append an immutable
    // artwork asset carrying the REAL blake3 hash of its bytes (the same
    // hash the publication pipeline records).
    let artwork = {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(512, 512, |x, y| {
            image::Rgba([(x % 253) as u8, (y % 239) as u8, 42, 255])
        }));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Jpeg)
            .expect("encode canonical artwork");
        encoded.into_inner()
    };
    let canonical_hash = blake3::hash(&artwork).to_hex().to_string();
    let canonical_path = format!("artwork/{canonical_hash}.jpg");
    sqlx::query(
        "UPDATE release_publication_plans
         SET plan_payload = jsonb_set(
             plan_payload,
             '{assets}',
             (plan_payload->'assets') || jsonb_build_array(jsonb_build_object(
                 'kind', 'artwork',
                 'canonical_path', $2,
                 'size_bytes', $3::bigint,
                 'byte_hash', $4
             ))
         )
         WHERE work_item_id = $1",
    )
    .bind(catalog.published.work_item_id)
    .bind(&canonical_path)
    .bind(i64::try_from(artwork.len()).expect("artwork size"))
    .bind(&canonical_hash)
    .execute(&db.pool)
    .await
    .expect("add canonical artwork fixture");

    let root = tempfile::tempdir().expect("canonical root");
    let on_disk = root
        .path()
        .join(&catalog.published.plan.target_relative_path)
        .join(&canonical_path);
    std::fs::create_dir_all(on_disk.parent().expect("artwork parent")).expect("mkdir");
    std::fs::write(&on_disk, &artwork).expect("write canonical artwork");

    let mut state = native_state(db.pool.clone());
    state.direct_play = Some(
        melodee_server::native_stream::DirectPlayService::new(
            melodee_server::native_stream::CanonicalAssetRoot::open(root.path())
                .expect("open canonical root"),
            4,
        )
        .expect("direct play"),
    );
    let images = melodee_server::native::images::router().with_state(state);

    // Intact storage: the canonical artwork is served (processed to size).
    let album_path = format!("/images/album_{release_public}/80");
    let (status, headers, bytes) =
        call(images.clone(), "GET", &album_path, None, vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    assert_eq!(jpeg_dimensions(&bytes), (80, 80));
    assert_ne!(bytes, PLACEHOLDER_ALBUM, "canonical artwork must serve");

    // Same-length corruption: the recorded-size check alone would pass, so
    // only the publication hash fence can catch the swapped bytes. The
    // captured contract answer for unavailable artwork is the placeholder.
    let corrupted: Vec<u8> = artwork.iter().map(|byte| byte ^ 0x5A).collect();
    std::fs::write(&on_disk, &corrupted).expect("corrupt canonical artwork");
    let (status, _headers, bytes) = call(images, "GET", &album_path, None, vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        bytes, PLACEHOLDER_ALBUM,
        "hash-mismatched storage must fall back to the captured placeholder"
    );

    db.drop().await;
}
