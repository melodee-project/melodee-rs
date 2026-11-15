//! Administrative Artist and Release deletion API (ADR-0199).
//!
//! The two catalog list bulk actions share one destructive surface: a read-only
//! impact preview, then an audited tombstone commit with either catalog-only or
//! releases-and-files scope. Artists that another Artist also bills as primary
//! are never retired by an Artist deletion.

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
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("del-admin", true, admin_public),
        ("del-plain", false, plain_public),
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

#[tokio::test]
async fn deletion_routes_require_an_administrator() {
    let db = TestDb::create("catalog_delete_gate").await;
    let fixture = seed_users(&db).await;
    let plain = token_for(fixture.plain_public, "del-plain");
    let app = app(db.pool.clone(), None);
    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [] })),
        Some(&plain),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, status, "{body}");
    db.drop().await;
}

#[tokio::test]
async fn artist_delete_preview_and_catalog_only_scope() {
    let db = TestDb::create("catalog_delete_artist_api").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "del-admin");
    let release = phase5_common::seed_cataloged_release(
        &db,
        401,
        CatalogFixturePresentation {
            release_title: "Deletable Artist Release",
            release_artist: "Deletable Artist",
            track_title: "Kept Track",
            track_artist: "Deletable Artist",
            release_date: "2001-01-01",
        },
    )
    .await;
    let artist = artist_public_id(&db, "Deletable Artist").await;
    let app = app(db.pool.clone(), None);

    let (status, preview) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [artist] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(1, preview["impact"]["releases"]);
    assert_eq!(1, preview["impact"]["tracks"]);
    assert_eq!(1, preview["impact"]["mediaFiles"]);
    assert_eq!(0, preview["impact"]["retainedSharedReleases"]);
    assert_eq!(false, preview["storageConfigured"]);

    let (status, deleted) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({
            "artistIds": [artist],
            "mode": "catalogOnly",
            "reason": "duplicate Artist identity created by an import",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(
        1,
        deleted["deletedArtistIds"].as_array().expect("ids").len()
    );
    assert_eq!(
        0,
        deleted["deletedReleaseIds"]
            .as_array()
            .expect("release ids")
            .len()
    );
    let artist_status: String =
        sqlx::query_scalar("SELECT status FROM artists WHERE public_api_id=$1")
            .bind(artist)
            .fetch_one(&db.pool)
            .await
            .expect("read Artist status");
    assert_eq!(artist_status, "deleted");
    let release_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(release.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Release status");
    assert_eq!(release_status, "active");

    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({
            "artistIds": [artist],
            "mode": "catalogOnly",
            "reason": " ",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("invalid_reason", error["code"]);
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({
            "artistIds": [artist],
            "mode": "everything",
            "reason": "invalid mode",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("invalid_mode", error["code"]);

    db.drop().await;
}

#[tokio::test]
async fn artist_delete_with_files_requires_storage_and_retires_owned_releases() {
    let db = TestDb::create("catalog_delete_artist_files").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "del-admin");
    let release = phase5_common::seed_cataloged_release(
        &db,
        411,
        CatalogFixturePresentation {
            release_title: "Files Release",
            release_artist: "Files Artist",
            track_title: "Files Track",
            track_artist: "Files Artist",
            release_date: "2002-02-02",
        },
    )
    .await;
    let artist = artist_public_id(&db, "Files Artist").await;

    // Without canonical storage the file-removing scope fails closed.
    let unsupported = app(db.pool.clone(), None);
    let (status, error) = call(
        unsupported,
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({
            "artistIds": [artist],
            "mode": "releasesAndFiles",
            "reason": "remove Artist and owned media",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, status, "{error}");
    assert_eq!("delete_storage_unavailable", error["code"]);

    let root = tempfile::tempdir().expect("canonical root");
    let configured = app(db.pool.clone(), Some(root.path().to_path_buf()));
    let (status, preview) = call(
        configured.clone(),
        "POST",
        "/api/admin/v1/catalog/artists/delete/preview",
        Some(serde_json::json!({ "artistIds": [artist] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(true, preview["storageConfigured"]);
    assert_eq!(1, preview["storageReadyReleases"]);

    let (status, deleted) = call(
        configured,
        "POST",
        "/api/admin/v1/catalog/artists/delete",
        Some(serde_json::json!({
            "artistIds": [artist],
            "mode": "releasesAndFiles",
            "reason": "remove Artist and owned media",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(
        1,
        deleted["deletedReleaseIds"].as_array().expect("ids").len()
    );
    let release_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(release.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Release status");
    assert_eq!(release_status, "deleted");
    let retained_tracks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count retained Tracks");
    assert_eq!(retained_tracks, 1);

    db.drop().await;
}

#[tokio::test]
async fn release_bulk_delete_preview_and_apply() {
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
    let public_id = |internal: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>("SELECT public_api_id FROM releases WHERE id=$1")
                .bind(internal)
                .fetch_one(&pool)
                .await
                .expect("read Release public id")
        }
    };
    let first_public = public_id(first.release_id().into_uuid()).await;
    let second_public = public_id(second.release_id().into_uuid()).await;
    let kept_public = public_id(kept.release_id().into_uuid()).await;
    let app = app(db.pool.clone(), None);

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
    assert_eq!(2, preview["releases"].as_array().expect("releases").len());

    let (status, deleted) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/delete",
        Some(serde_json::json!({
            "releaseIds": [first_public, second_public],
            "mode": "catalogOnly",
            "reason": "administrator removed duplicate imports",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{deleted}");
    assert_eq!(
        2,
        deleted["deletedReleaseIds"].as_array().expect("ids").len()
    );
    let statuses: (String, String, String) = sqlx::query_as(
        "SELECT
            (SELECT status FROM releases WHERE id=$1),
            (SELECT status FROM releases WHERE id=$2),
            (SELECT status FROM releases WHERE id=$3)",
    )
    .bind(first.release_id().into_uuid())
    .bind(second.release_id().into_uuid())
    .bind(kept.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read Release statuses");
    assert_eq!(
        statuses,
        (
            "deleted".to_owned(),
            "deleted".to_owned(),
            "active".to_owned()
        )
    );

    // Deleting an already deleted Release is rejected, and the untouched one
    // is still selectable.
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/delete",
        Some(serde_json::json!({
            "releaseIds": [first_public],
            "mode": "catalogOnly",
            "reason": "repeat",
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("not_deletable", error["code"]);
    let (status, preview) = call(
        app,
        "POST",
        "/api/admin/v1/catalog/releases/delete/preview",
        Some(serde_json::json!({ "releaseIds": [kept_public] })),
        Some(&admin),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(1, preview["impact"]["releases"]);

    db.drop().await;
}
