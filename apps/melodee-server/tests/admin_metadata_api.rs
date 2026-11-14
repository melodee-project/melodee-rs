//! Metadata transform integration tests (MEL-8).
//!
//! Exercises preview/apply/revert against a real PostgreSQL database and
//! canonical media root, covering authorization gates, failure injection
//! (missing ffmpeg), exact revert, and catalog consistency. The existing
//! implementation is qualified end to end; code is changed only if a test
//! exposes a defect.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use sha2::{Digest, Sha256};

fn file_sha256(path: &Path) -> String {
    let mut file = fs::File::open(path).expect("open fixture file");
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer).expect("read fixture file");
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_media::{TagRewrite, apply_tag_rewrite};
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-metadata-jwt-key-least-32b";

/// A valid 64-hex placeholder hash used as the initial media asset hash (the
/// database rejects non-hex values such as the literal string "placeholder").
const INITIAL_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
    /// Kept alive so the canonical storage root (and its WAV fixture) persists
    /// for the duration of the test.
    #[expect(dead_code)]
    temp_dir: tempfile::TempDir,
    storage_root: PathBuf,
    release_public: Uuid,
    media_asset_id: Uuid,
    wav_path: PathBuf,
}

async fn seed_users_and_media(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();

    for (username, is_admin, public_id) in [
        ("wp-md-admin", true, admin_public),
        ("wp-md-plain", false, plain_public),
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

    // Canonical storage root with a decodable WAV file.
    let temp_dir = tempfile::tempdir().expect("storage root");
    let storage_root = temp_dir.path().to_path_buf();
    let wav_path = storage_root.join("track01.wav");
    Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-q:a",
            "8",
            "-metadata",
            "title=Fixture Track",
            "-y",
            wav_path.to_str().expect("wav path"),
        ])
        .output()
        .expect("generate wav fixture");

    // Give the WAV an observable ID3 title tag so the transform engine can read
    // the current title when planning the replacement. ffmpeg's WAV muxer does
    // not write an ID3 tag, so we add one via the safe tag-rewrite path.
    let rewrite = TagRewrite {
        title: Some("Fixture Track".to_owned()),
        ..TagRewrite::default()
    };
    apply_tag_rewrite(&wav_path, &rewrite).expect("add title tag to fixture");

    // Seed a release with one track and one media asset pointing at the WAV.
    let release_id = Uuid::now_v7();
    let release_public = Uuid::now_v7();
    let track_id = Uuid::now_v7();
    let track_public = Uuid::now_v7();
    let media_asset_id = Uuid::now_v7();
    let relative_path = "track01.wav";

    sqlx::query(
        "INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,
                 display_artist, release_date, status, revision, created_at, updated_at)
         VALUES ($1, $2, 'Metadata Release', 'metadata release', 'Metadata Release',
                 'Metadata Artist', DATE '2001-01-01', 'active', 1, $3, $3)",
    )
    .bind(release_id)
    .bind(release_public)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert release");

    let audio_identity_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO audio_identities (id, status, created_at, updated_at)
         VALUES ($1, 'provisional', $2, $2)",
    )
    .bind(audio_identity_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert audio identity");
    let mut tx = db.pool.begin().await.expect("begin track transaction");
    sqlx::query(
        "INSERT INTO tracks (id, public_api_id, release_id, playable_media_asset_id,
                 audio_identity_id, ordinal, title, normalized_title, sort_title,
                 display_artist, disc_number, track_number, duration_ms, source_path,
                 revision, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, 1, 'Fixture Track', 'fixture track', 'Fixture Track',
                 NULL, NULL, 1, 1000, $6, 1, $7, $7)",
    )
    .bind(track_id)
    .bind(track_public)
    .bind(release_id)
    .bind(media_asset_id)
    .bind(audio_identity_id)
    .bind(relative_path)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("insert track");
    sqlx::query(
        "INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size,
                 mime_type, container, codec, sample_rate_hz, channels, bitrate_bps,
                 duration_ms, created_at)
         VALUES ($1, $2, $3, $4, 'audio/wav', 'wav', 'pcm_s16le', 44100, 1,
                 1411000, 1000, $5)",
    )
    .bind(media_asset_id)
    .bind(relative_path)
    .bind(INITIAL_HASH)
    .bind(0)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("insert media asset");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
         VALUES ($1, $2, 'playable', 1, $3)",
    )
    .bind(track_id)
    .bind(media_asset_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .expect("insert track media asset");
    tx.commit().await.expect("commit track transaction");

    Fixture {
        admin_public,
        plain_public,
        temp_dir,
        storage_root,
        release_public,
        media_asset_id,
        wav_path,
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

fn app(pool: sqlx::PgPool, storage_root: PathBuf, ffmpeg_path: &Path) -> axum::Router {
    let mut config = melodee_ingestion::IngestionConfig::with_sources(vec![]);
    config.ffmpeg_path = ffmpeg_path.to_path_buf();
    config.ffmpeg_timeout = time::Duration::seconds(300);
    config.storage_root = Some(storage_root);

    melodee_server::admin_metadata::router(config).with_state(AdminAccessState {
        pool: pool.clone(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
        notifications: Arc::new(melodee_server::notifications::NotificationService::noop(
            pool.clone(),
        )),
        scripting: Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
    })
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    bearer: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(path);
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

fn json_body<T: serde::Serialize>(value: T) -> Vec<u8> {
    serde_json::to_vec(&value).expect("serialize")
}

fn parse_json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("parse json")
}

#[tokio::test]
async fn metadata_routes_require_administrator() {
    let db = common::TestDb::create("wp_md_gate").await;
    let fixture = seed_users_and_media(&db).await;
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/usr/bin/ffmpeg"),
    );
    let plain = token_for(fixture.plain_public, "wp-md-plain");
    let body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "old", "to": "new"}
    }));

    for (path, method) in [
        (
            format!(
                "/api/admin/v1/catalog/releases/{}/metadata-transforms/preview",
                fixture.release_public
            ),
            "POST",
        ),
        (
            format!(
                "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
                fixture.release_public
            ),
            "POST",
        ),
        (
            format!(
                "/api/admin/v1/catalog/releases/{}/metadata-transforms",
                fixture.release_public
            ),
            "GET",
        ),
    ] {
        // No token: 401
        let (status, _, _) = call(
            app.clone(),
            method,
            &path,
            Some("application/json"),
            body.clone(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no token {path} {method}");
        // Plain user: 403
        let (status, _, _) = call(
            app.clone(),
            method,
            &path,
            Some("application/json"),
            body.clone(),
            Some(&plain),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "plain user {path} {method}");
    }
    // Revert with no token: 401
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/{}/revert",
            fixture.release_public,
            Uuid::now_v7()
        ),
        None,
        vec![],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revert no token");
    // Revert with plain user: 403
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/{}/revert",
            fixture.release_public,
            Uuid::now_v7()
        ),
        None,
        vec![],
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "revert plain user");

    db.drop().await;
}

#[tokio::test]
async fn unknown_release_returns_404() {
    let db = common::TestDb::create("wp_md_not_found").await;
    let fixture = seed_users_and_media(&db).await;
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/usr/bin/ffmpeg"),
    );
    let admin = token_for(fixture.admin_public, "wp-md-admin");
    let body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "old", "to": "new"}
    }));

    let path = format!(
        "/api/admin/v1/catalog/releases/{}/metadata-transforms/preview",
        Uuid::now_v7()
    );
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &path,
        Some("application/json"),
        body.clone(),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "preview unknown release");

    let path = format!(
        "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
        Uuid::now_v7()
    );
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &path,
        Some("application/json"),
        body.clone(),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "apply unknown release");

    db.drop().await;
}

#[tokio::test]
async fn preview_then_apply_replace_title() {
    let db = common::TestDb::create("wp_md_apply").await;
    let fixture = seed_users_and_media(&db).await;
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/usr/bin/ffmpeg"),
    );
    let admin = token_for(fixture.admin_public, "wp-md-admin");

    let preview_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"}
    }));

    // Preview returns planned deltas.
    let (status, _, bytes) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/preview",
            fixture.release_public
        ),
        Some("application/json"),
        preview_body.clone(),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preview status");
    let preview: serde_json::Value = parse_json(&bytes);
    assert_eq!(preview["operation"], "replace_song_titles");
    assert_eq!(preview["affectedFiles"], 1);
    assert_eq!(preview["deltas"][0]["field"], "title");
    assert_eq!(preview["deltas"][0]["old"].as_str(), Some("Fixture Track"));
    assert_eq!(preview["deltas"][0]["new"].as_str(), Some("Changed Track"));
    let preview_hash = preview["previewHash"]
        .as_str()
        .expect("preview hash")
        .to_string();

    // Apply requires the preview hash.
    let apply_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"},
        "previewHash": preview_hash
    }));
    let (status, _, bytes) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
            fixture.release_public
        ),
        Some("application/json"),
        apply_body,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "apply status");
    let apply: serde_json::Value = parse_json(&bytes);
    assert_eq!(apply["status"], "success");
    let audit_id = apply["auditId"].as_str().expect("audit id").to_string();

    // Verify the media asset hash was updated and the file on disk changed.
    let new_hash: String = sqlx::query_scalar("SELECT byte_hash FROM media_assets WHERE id = $1")
        .bind(fixture.media_asset_id)
        .fetch_one(&db.pool)
        .await
        .expect("new hash");
    assert_ne!(new_hash, INITIAL_HASH);

    let file_bytes = fs::read(&fixture.wav_path).expect("read wav");
    assert_ne!(file_bytes.len(), 0);

    // List audits shows the recorded transformation.
    let (status, _, bytes) = call(
        app.clone(),
        "GET",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms",
            fixture.release_public
        ),
        None,
        vec![],
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list audits status");
    let audits: serde_json::Value = parse_json(&bytes);
    assert!(!audits.as_array().unwrap().is_empty());
    for row in audits.as_array().expect("audit list") {
        for field in ["createdAt", "completedAt"] {
            if !row[field].is_null() {
                let timestamp = row[field].as_str().expect("audit timestamp string");
                time::OffsetDateTime::parse(
                    timestamp,
                    &time::format_description::well_known::Rfc3339,
                )
                .expect("audit timestamp is RFC 3339");
                assert!(timestamp.ends_with('Z'));
            }
        }
    }

    // Verify the audit record contains the audit_id.
    let audit_found = audits
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"].as_str() == Some(&audit_id));
    assert!(audit_found, "audit record found");

    db.drop().await;
}

#[tokio::test]
async fn revert_restores_original() {
    let db = common::TestDb::create("wp_md_revert").await;
    let fixture = seed_users_and_media(&db).await;
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/usr/bin/ffmpeg"),
    );
    let admin = token_for(fixture.admin_public, "wp-md-admin");

    // Apply a transform.
    let preview_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"}
    }));
    let (_, _, preview_bytes) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/preview",
            fixture.release_public
        ),
        Some("application/json"),
        preview_body.clone(),
        Some(&admin),
    )
    .await;
    let preview: serde_json::Value = parse_json(&preview_bytes);
    let preview_hash = preview["previewHash"]
        .as_str()
        .expect("preview hash")
        .to_string();

    // Hash of the fixture as it exists before the transform (it carries the
    // ID3 title tag added by the fixture setup). The revert must restore this.
    let pre_apply_hash = file_sha256(&fixture.wav_path);

    let apply_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"},
        "previewHash": preview_hash
    }));
    let (status, _, apply_bytes) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
            fixture.release_public
        ),
        Some("application/json"),
        apply_body,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "apply status");
    let apply: serde_json::Value = parse_json(&apply_bytes);
    let audit_id = apply["auditId"].as_str().expect("audit id").to_string();

    // Revert the audit.
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/{}/revert",
            fixture.release_public, audit_id
        ),
        None,
        vec![],
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revert status");

    // Verify the file on disk is restored to original.
    let original_bytes = fs::read(&fixture.wav_path).expect("read restored wav");
    assert_ne!(original_bytes.len(), 0);

    // Verify the media asset hash is restored to the pre-transform state.
    let restored_hash: String =
        sqlx::query_scalar("SELECT byte_hash FROM media_assets WHERE id = $1")
            .bind(fixture.media_asset_id)
            .fetch_one(&db.pool)
            .await
            .expect("restored hash");
    assert_eq!(
        restored_hash, pre_apply_hash,
        "hash should be restored to pre-transform hash"
    );

    db.drop().await;
}

#[tokio::test]
async fn missing_ffmpeg_does_not_corrupt_files() {
    let db = common::TestDb::create("wp_md_failure").await;
    let fixture = seed_users_and_media(&db).await;
    let admin = token_for(fixture.admin_public, "wp-md-admin");

    // Point ffmpeg at a non-existent binary.
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/definitely/missing/ffmpeg"),
    );

    let preview_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"}
    }));
    let (_, _, preview_bytes) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/preview",
            fixture.release_public
        ),
        Some("application/json"),
        preview_body.clone(),
        Some(&admin),
    )
    .await;
    let preview: serde_json::Value = parse_json(&preview_bytes);
    let preview_hash = preview["previewHash"]
        .as_str()
        .expect("preview hash")
        .to_string();

    let apply_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"},
        "previewHash": preview_hash
    }));
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
            fixture.release_public
        ),
        Some("application/json"),
        apply_body,
        Some(&admin),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "apply fails with missing ffmpeg"
    );

    // Verify the original file is unchanged.
    let original_bytes = fs::read(&fixture.wav_path).expect("read original wav");
    assert_ne!(original_bytes.len(), 0);

    // Verify no temp artifacts remain.
    let entries: Vec<_> = fs::read_dir(&fixture.storage_root)
        .expect("list storage root")
        .map(|e| e.expect("dir entry"))
        .collect();
    let temp_artifacts: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.file_name()
                .to_str()
                .unwrap_or("")
                .contains("melodee-edit")
        })
        .collect();
    assert!(
        temp_artifacts.is_empty(),
        "no temp artifacts: {:?}",
        temp_artifacts
    );

    // Verify the media asset hash is unchanged.
    let hash: String = sqlx::query_scalar("SELECT byte_hash FROM media_assets WHERE id = $1")
        .bind(fixture.media_asset_id)
        .fetch_one(&db.pool)
        .await
        .expect("hash unchanged");
    assert_eq!(hash, INITIAL_HASH);

    db.drop().await;
}

#[tokio::test]
async fn preview_mismatch_rejected() {
    let db = common::TestDb::create("wp_md_mismatch").await;
    let fixture = seed_users_and_media(&db).await;
    let app = app(
        db.pool.clone(),
        fixture.storage_root.clone(),
        Path::new("/usr/bin/ffmpeg"),
    );
    let admin = token_for(fixture.admin_public, "wp-md-admin");

    // Apply with a wrong preview hash.
    let apply_body = json_body(serde_json::json!({
        "operation": "replace_song_titles",
        "params": {"from": "Fixture", "to": "Changed"},
        "previewHash": "0000000000000000000000000000000000000000000000000000000000000000"
    }));
    let (status, _, _) = call(
        app.clone(),
        "POST",
        &format!(
            "/api/admin/v1/catalog/releases/{}/metadata-transforms/apply",
            fixture.release_public
        ),
        Some("application/json"),
        apply_body,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "preview mismatch rejected");

    db.drop().await;
}
