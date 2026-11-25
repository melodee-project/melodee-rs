//! Administrator and editor hard-deletion API (ADR-0211).
//!
//! The confirmation dialog is the only step: a confirmed request removes the
//! selected rows, their dependent evidence, the originating work item, and the
//! files derived from durable paths. No reason, scope, or workflow is required.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use common::TestDb;
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthStore, NativeUserImport};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-catalog-delete-key-32";

struct Fixture {
    admin_public: Uuid,
    editor_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let editor_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, is_editor, public_id) in [
        ("del-admin", true, false, admin_public),
        ("del-editor", false, true, editor_public),
        ("del-plain", false, false, plain_public),
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
                    is_editor,
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
        editor_public,
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

fn app(pool: sqlx::PgPool, canonical_root: Option<std::path::PathBuf>) -> axum::Router {
    let state = melodee_server::admin_access::AdminAccessState {
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
    };
    axum::Router::new().merge(melodee_server::admin_catalog_delete::router().with_state(
        melodee_server::admin_catalog_delete::AdminArtistDeleteState::new(
            state,
            canonical_root,
            None,
        ),
    ))
}

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
    let response = app
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

async fn artist_public_id(db: &TestDb, canonical_name: &str) -> Uuid {
    sqlx::query_scalar("SELECT public_api_id FROM artists WHERE canonical_name=$1")
        .bind(canonical_name)
        .fetch_one(&db.pool)
        .await
        .expect("read Artist public id")
}

async fn release_public_id(db: &TestDb, internal: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(internal)
        .fetch_one(&db.pool)
        .await
        .expect("read Release public id")
}

#[tokio::test]
async fn deletion_routes_reject_a_non_operator_and_allow_an_editor() {
    let db = TestDb::create("catalog_delete_gate").await;
    let fixture = seed_users(&db).await;
    let plain = token_for(fixture.plain_public, "del-plain");
    let editor = token_for(fixture.editor_public, "del-editor");
    let router = app(db.pool.clone(), None);
    let (status, body) = call(
        router.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [] })),
        Some(&plain),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, status, "{body}");

    let (status, body) = call(
        router,
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [] })),
        Some(&editor),
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status, "{body}");
    db.drop().await;
}

#[tokio::test]
async fn artist_delete_needs_no_reason_and_fails_closed_without_storage() {
    let db = TestDb::create("catalog_delete_artist_api").await;
    let fixture = seed_users(&db).await;
    let editor = token_for(fixture.editor_public, "del-editor");
    let release = phase5_common::seed_cataloged_release(
        &db,
        401,
        CatalogFixturePresentation {
            release_title: "Deletable Artist Release",
            release_artist: "Deletable Artist",
            track_title: "Deleted Track",
            track_artist: "Deletable Artist",
            release_date: "2001-01-01",
        },
    )
    .await;
    let artist = artist_public_id(&db, "Deletable Artist").await;
    let router = app(db.pool.clone(), None);

    let (status, preview) = call(
        router.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [artist] })),
        Some(&editor),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(1, preview["impact"]["artists"]);
    assert_eq!(1, preview["impact"]["releases"]);
    assert_eq!(1, preview["impact"]["tracks"]);
    assert_eq!(1, preview["impact"]["mediaFiles"]);
    assert_eq!(false, preview["storageConfigured"]);

    // The owned media cannot be removed without a configured canonical root;
    // nothing is changed.
    let (status, error) = call(
        router,
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({ "artistIds": [artist] })),
        Some(&editor),
    )
    .await;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, status, "{error}");
    assert_eq!("delete_storage_unavailable", error["code"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artists WHERE public_api_id=$1")
            .bind(artist)
            .fetch_one(&db.pool)
            .await
            .expect("count Artist"),
        1
    );

    let root = tempfile::tempdir().expect("canonical root");
    let configured = app(db.pool.clone(), Some(root.path().to_path_buf()));
    let (status, deleted) = call(
        configured,
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({ "artistIds": [artist] })),
        Some(&editor),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(
        1,
        deleted["deletedArtistIds"].as_array().expect("ids").len()
    );
    assert_eq!(
        1,
        deleted["deletedReleaseIds"].as_array().expect("ids").len()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artists WHERE public_api_id=$1")
            .bind(artist)
            .fetch_one(&db.pool)
            .await
            .expect("count Artist"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM releases WHERE id=$1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count Release"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count Tracks"),
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn release_bulk_delete_removes_the_selection_and_keeps_the_rest() {
    let db = TestDb::create("catalog_delete_release_api").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "del-admin");
    let first = phase5_common::seed_cataloged_release(
        &db,
        421,
        CatalogFixturePresentation {
            release_title: "Bulk Delete One",
            release_artist: "Bulk Artist One",
            track_title: "Bulk Track One",
            track_artist: "Bulk Artist One",
            release_date: "2003-03-03",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        422,
        CatalogFixturePresentation {
            release_title: "Bulk Delete Two",
            release_artist: "Bulk Artist Two",
            track_title: "Bulk Track Two",
            track_artist: "Bulk Artist Two",
            release_date: "2004-04-04",
        },
    )
    .await;
    let kept = phase5_common::seed_cataloged_release(
        &db,
        423,
        CatalogFixturePresentation {
            release_title: "Bulk Keep Three",
            release_artist: "Bulk Artist Three",
            track_title: "Bulk Track Three",
            track_artist: "Bulk Artist Three",
            release_date: "2005-05-05",
        },
    )
    .await;
    let first_public = release_public_id(&db, first.release_id().into_uuid()).await;
    let second_public = release_public_id(&db, second.release_id().into_uuid()).await;
    let kept_public = release_public_id(&db, kept.release_id().into_uuid()).await;
    let root = tempfile::tempdir().expect("canonical root");
    let app = app(db.pool.clone(), Some(root.path().to_path_buf()));

    let (status, preview) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/delete/preview",
        Some(serde_json::json!({ "releaseIds": [first_public, second_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(2, preview["impact"]["releases"]);
    assert_eq!(2, preview["impact"]["tracks"]);
    assert_eq!(2, preview["impact"]["mediaFiles"]);

    let (status, deleted) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/delete",
        Some(serde_json::json!({ "releaseIds": [first_public, second_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(
        2,
        deleted["deletedReleaseIds"].as_array().expect("ids").len()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM releases WHERE public_api_id = ANY($1)")
            .bind([first_public, second_public])
            .fetch_one(&db.pool)
            .await
            .expect("count deleted Releases"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM releases WHERE public_api_id=$1")
            .bind(kept_public)
            .fetch_one(&db.pool)
            .await
            .expect("count kept Release"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM release_work_items WHERE id IN ($1,$2)")
            .bind(first.work_item_id())
            .bind(second.work_item_id())
            .fetch_one(&db.pool)
            .await
            .expect("count deleted work items"),
        0
    );

    // Repeating the same selection is a clean not-found, not a tombstone.
    let (status, error) = call(
        app,
        "POST",
        "/api/admin/v1/catalog/releases/delete",
        Some(serde_json::json!({ "releaseIds": [first_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, status);
    assert_eq!("not_found", error["code"]);

    db.drop().await;
}

#[tokio::test]
async fn track_delete_removes_the_track_and_its_file_but_keeps_the_release() {
    let db = TestDb::create("catalog_delete_track_api").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "del-admin");
    let release = phase5_common::seed_cataloged_release(
        &db,
        431,
        CatalogFixturePresentation {
            release_title: "Track Keep Release",
            release_artist: "Track Keep Artist",
            track_title: "Deleted Track",
            track_artist: "Track Keep Artist",
            release_date: "2006-06-06",
        },
    )
    .await;
    let track_public: Uuid = sqlx::query_scalar(
        "SELECT public_api_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(release.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read Track public id");
    let root = tempfile::tempdir().expect("canonical root");
    let app = app(db.pool.clone(), Some(root.path().to_path_buf()));

    let (status, preview) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/tracks/delete/preview",
        Some(serde_json::json!({ "trackIds": [track_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(1, preview["impact"]["tracks"]);
    assert_eq!(1, preview["impact"]["mediaFiles"]);

    let (status, deleted) = call(
        app,
        "POST",
        "/api/admin/v1/catalog/tracks/delete",
        Some(serde_json::json!({ "trackIds": [track_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(1, deleted["deletedTrackIds"].as_array().expect("ids").len());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks WHERE public_api_id=$1")
            .bind(track_public)
            .fetch_one(&db.pool)
            .await
            .expect("count Track"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM releases WHERE id=$1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count Release"),
        1
    );
    db.drop().await;
}
