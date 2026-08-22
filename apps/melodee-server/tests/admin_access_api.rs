//! Admin API integration tests: group CRUD lifecycle,
//! membership/binding effects on the ACL predicate, admin user management,
//! and the administrator-only authorization gate (PRD-AUTH-003/004).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use melodee_auth::{NativeAccessClaims, PasswordHashAlgorithm, SecretString, issue_access_token};
use melodee_persistence::{AuthLookup, AuthStore, NativeUserImport};
use melodee_server::admin_access::{AdminAccessState, router};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-access-jwt-key-at-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed(db: &common::TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp03-admin", true, admin_public),
        ("wp03-plain", false, plain_public),
    ] {
        let email = format!("{username}@example.test");
        let normalized = username.to_uppercase();
        let normalized_email = email.to_uppercase();
        AuthStore::new()
            .import_user(
                &db.pool,
                &NativeUserImport {
                    id: Uuid::now_v7(),
                    public_api_id: public_id,
                    username,
                    normalized_username: &normalized,
                    email: &email,
                    normalized_email: &normalized_email,
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

fn app(pool: sqlx::PgPool) -> axum::Router {
    router().with_state(AdminAccessState {
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(pool.clone()),
        ),
        scripting: std::sync::Arc::new(melodee_server::scripting::ScriptingService::new(
            pool.clone(),
        )),
        pool,
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(JWT_KEY.to_string(), 90)
                .expect("auth config"),
        ),
    })
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
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, json)
}

#[tokio::test]
async fn group_lifecycle_via_http() {
    let db = common::TestDb::create("wp03_admin_group_lifecycle").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let app = app(db.pool.clone());

    // Create.
    let (status, group) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "Listeners"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = group["id"].as_str().expect("group id").to_string();
    assert_eq!(group["name"], "Listeners");
    assert_eq!(group["memberCount"], 0);
    assert_eq!(group["bindingCount"], 0);

    // List.
    let (status, groups) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/groups",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(groups.as_array().expect("array").len(), 1);

    // Rename.
    let (status, renamed) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/groups/{group_id}"),
        Some(serde_json::json!({"name": "Audited"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Audited");

    // Membership + binding.
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!(
            "/api/admin/v1/groups/{group_id}/members/{}",
            fixture.plain_public
        ),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let library_id = Uuid::now_v7();
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/groups/{group_id}/libraries/{library_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Detail reflects both.
    let (status, detail) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/groups/{group_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["members"].as_array().expect("members").len(), 1);
    assert_eq!(
        detail["members"][0]["userId"].as_str(),
        Some(fixture.plain_public.to_string().as_str())
    );
    assert_eq!(
        detail["libraryIds"].as_array().expect("libraries"),
        &vec![serde_json::json!(library_id.to_string())]
    );

    // Delete fails closed while members/bindings remain.
    let (status, body) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/groups/{group_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "group_not_empty");

    // Detach everything, then delete succeeds.
    for path in [
        format!(
            "/api/admin/v1/groups/{group_id}/members/{}",
            fixture.plain_public
        ),
        format!("/api/admin/v1/groups/{group_id}/libraries/{library_id}"),
    ] {
        let (status, _) = call(app.clone(), "DELETE", &path, None, Some(&admin)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{path}");
    }
    let (status, _) = call(
        app.clone(),
        "DELETE",
        &format!("/api/admin/v1/groups/{group_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/groups/{group_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn membership_and_binding_changes_drive_acl_enforcement() {
    let db = common::TestDb::create("wp03_admin_acl_effect").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let app = app(db.pool.clone());
    let auth = AuthStore::new();
    let plain_internal: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE public_api_id=$1")
            .bind(fixture.plain_public)
            .fetch_one(&db.pool)
            .await
            .expect("internal id");
    let library_id = Uuid::now_v7();

    let (status, group) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "ACL"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = group["id"].as_str().expect("id").to_string();

    // Open access before any binding.
    assert!(
        auth.can_access_library(&db.pool, plain_internal, Some(library_id))
            .await
            .expect("acl")
    );

    // Binding without membership closes access.
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/groups/{group_id}/libraries/{library_id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        !auth
            .can_access_library(&db.pool, plain_internal, Some(library_id))
            .await
            .expect("acl")
    );

    // Membership reopens it.
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!(
            "/api/admin/v1/groups/{group_id}/members/{}",
            fixture.plain_public
        ),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        auth.can_access_library(&db.pool, plain_internal, Some(library_id))
            .await
            .expect("acl")
    );

    // Unknown user/group ids are 404, never silent success.
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/groups/{group_id}/members/{}", Uuid::now_v7()),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!(
            "/api/admin/v1/groups/{}/members/{}",
            Uuid::now_v7(),
            fixture.plain_public
        ),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn create_user_round_trips_without_secret_leakage() {
    let db = common::TestDb::create("wp03_admin_create_user").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let app = app(db.pool.clone());

    let (status, created) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/users",
        Some(serde_json::json!({
            "username": "created-user",
            "email": "created@example.test",
            "password": "Sup3rSecretPassword!",
            "isEditor": true,
            "roles": ["HasStreamRole", "HasDownloadRole"]
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["username"], "created-user");
    assert_eq!(created["isEditor"], true);
    assert_eq!(created["isAdmin"], false);
    assert_eq!(created["enabled"], true);
    let serialized = serde_json::to_string(&created).expect("serialize");
    assert!(!serialized.contains("Sup3rSecret"));
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("Hash"));
    assert!(!serialized.contains("stream"));

    // The stored credential authenticates through the normal authority.
    let record = AuthStore::new()
        .user_for_auth(&db.pool, AuthLookup::Username("CREATED-USER"))
        .await
        .expect("lookup")
        .expect("created user");
    assert!(
        melodee_auth::verify_password(
            b"Sup3rSecretPassword!",
            record.password_hash_algorithm,
            record.password_hash.expose_secret(),
        )
        .expect("verify")
    );

    // Duplicate username conflicts; weak password is rejected.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/users",
        Some(serde_json::json!({
            "username": "CREATED-user",
            "email": "other@example.test",
            "password": "Sup3rSecretPassword!"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "user_conflict");
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/users",
        Some(serde_json::json!({
            "username": "weak-password-user",
            "email": "weak@example.test",
            "password": "short"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    db.drop().await;
}

#[tokio::test]
async fn update_user_flags_and_disable_enable() {
    let db = common::TestDb::create("wp03_admin_update_user").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let app = app(db.pool.clone());

    // Grant admin + replace roles.
    let (status, updated) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/users/{}", fixture.plain_public),
        Some(serde_json::json!({"isAdmin": true, "roles": ["Administrator"]})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["isAdmin"], true);
    assert_eq!(updated["roles"], serde_json::json!(["Administrator"]));
    assert_eq!(
        updated["id"].as_str(),
        Some(fixture.plain_public.to_string().as_str())
    );

    // Disable: the account can no longer authenticate.
    let (status, updated) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/users/{}", fixture.plain_public),
        Some(serde_json::json!({"enabled": false})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["enabled"], false);
    let disabled = AuthStore::new()
        .user_by_public_id(&db.pool, fixture.plain_public)
        .await
        .expect("lookup")
        .expect("user");
    assert!(!disabled.can_authenticate());

    // A disabled account's token stops authorizing admin actions even when
    // the account was granted admin rights above.
    let disabled_token = token_for(fixture.plain_public, "wp03-plain");
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/groups",
        None,
        Some(&disabled_token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Re-enable.
    let (status, updated) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/users/{}", fixture.plain_public),
        Some(serde_json::json!({"enabled": true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["enabled"], true);

    // Unknown user is 404.
    let (status, _) = call(
        app.clone(),
        "PUT",
        &format!("/api/admin/v1/users/{}", Uuid::now_v7()),
        Some(serde_json::json!({"enabled": false})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    db.drop().await;
}

#[tokio::test]
async fn non_administrators_and_anonymous_callers_are_rejected() {
    let db = common::TestDb::create("wp03_admin_authorization").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let plain = token_for(fixture.plain_public, "wp03-plain");
    let app = app(db.pool.clone());

    for (method, path, body) in [
        ("GET", "/api/admin/v1/groups".to_string(), None),
        (
            "POST",
            "/api/admin/v1/groups".to_string(),
            Some(serde_json::json!({"name": "Nope"})),
        ),
        (
            "POST",
            "/api/admin/v1/users".to_string(),
            Some(serde_json::json!({
                "username": "x", "email": "x@example.test", "password": "long-enough-password"
            })),
        ),
        (
            "PUT",
            format!("/api/admin/v1/users/{}", fixture.plain_public),
            Some(serde_json::json!({"isAdmin": true})),
        ),
    ] {
        let (status, _) = call(app.clone(), method, &path, body.clone(), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {path}"
        );
        let (status, _) = call(app.clone(), method, &path, body, Some(&plain)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "non-admin {method} {path}");
    }

    // A malformed token is unauthenticated, and the admin token works.
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/groups",
        None,
        Some("not-a-jwt"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/groups",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    db.drop().await;
}

#[tokio::test]
async fn group_names_are_bounded() {
    let db = common::TestDb::create("wp03_admin_group_bounds").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "   "})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_group_name");

    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "x".repeat(201)})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Exact-name conflict is a 409.
    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "Dup"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/groups",
        Some(serde_json::json!({"name": "Dup"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "group_exists");
    db.drop().await;
}

#[tokio::test]
async fn list_users_is_paginated_filtered_sorted_and_gated() {
    let db = common::TestDb::create("wp10_admin_list_users").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let plain = token_for(fixture.plain_public, "wp03-plain");
    let app = app(db.pool.clone());

    // Gating: anonymous 401, non-admin 403 with the stable envelope.
    let (status, body) = call(app.clone(), "GET", "/api/admin/v1/users", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthenticated");
    assert!(body["correlationId"].as_str().is_some());
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");

    // The seeded pair is returned in a pagination envelope, username ascending.
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 2);
    assert_eq!(page["limit"], 50);
    assert_eq!(page["offset"], 0);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["username"], "wp03-admin");
    assert_eq!(items[1]["username"], "wp03-plain");
    // The envelope never carries secret material.
    let serialized = serde_json::to_string(&page).expect("serialize");
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains("$2"));

    // Substring query filter matches username or email, case-insensitively.
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users?query=PLAIN",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["username"], "wp03-plain");
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users?query=example.test&limit=1&offset=1",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 2, "total respects the filter, not the page");
    assert_eq!(page["items"].as_array().expect("items").len(), 1);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    // LIKE metacharacters in the query are literal, not wildcards.
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users?query=%25",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 0);

    // Sort whitelist: descending username reverses the order.
    let (status, page) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/users?sort=username:desc",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["username"], "wp03-plain");
    for sort in ["username:asc", "createdAt:asc", "createdAt:desc"] {
        let (status, page) = call(
            app.clone(),
            "GET",
            &format!("/api/admin/v1/users?sort={sort}"),
            None,
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sort}");
        assert_eq!(page["items"].as_array().expect("items").len(), 2, "{sort}");
    }
    for sort in ["email:asc", "username:sideways", "username"] {
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/api/admin/v1/users?sort={sort}"),
            None,
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{sort}");
        assert_eq!(body["code"], "unknown_sort", "{sort}");
    }
    db.drop().await;
}

#[tokio::test]
async fn password_reset_rotates_credential_and_revokes_sessions() {
    let db = common::TestDb::create("wp10_admin_password_reset").await;
    let fixture = seed(&db).await;
    let admin = token_for(fixture.admin_public, "wp03-admin");
    let plain = token_for(fixture.plain_public, "wp03-plain");
    let app = app(db.pool.clone());
    let path = format!("/api/admin/v1/users/{}/password", fixture.plain_public);

    // Gating: anonymous 401, non-admin 403.
    let body = Some(serde_json::json!({"password": "N3wPasswordLongEnough!"}));
    let (status, _) = call(app.clone(), "POST", &path, body.clone(), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(app.clone(), "POST", &path, body, Some(&plain)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Issue a live refresh family for the target user so revocation is
    // observable after the reset.
    let plain_internal: Uuid =
        sqlx::query_scalar("SELECT id FROM native_users WHERE public_api_id=$1")
            .bind(fixture.plain_public)
            .fetch_one(&db.pool)
            .await
            .expect("internal id");
    AuthStore::new()
        .issue_refresh_family(
            &db.pool,
            &melodee_persistence::RefreshFamilyIssue {
                family_id: Uuid::now_v7(),
                token_id: Uuid::now_v7(),
                user_id: plain_internal,
                expected_auth_revision: 1,
                token_digest: &melodee_auth::RefreshTokenDigest::from_token(b"pre-reset-session"),
                device_id: Some("fixture-device"),
                session_started_at: OffsetDateTime::now_utc(),
                family_expires_at: OffsetDateTime::now_utc() + time::Duration::days(90),
                token_expires_at: OffsetDateTime::now_utc() + time::Duration::days(30),
                issued_at: OffsetDateTime::now_utc(),
            },
        )
        .await
        .expect("issue family");

    // Weak passwords are rejected before any mutation.
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"password": "short"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_password");

    // Unknown user is 404.
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/users/{}/password", Uuid::now_v7()),
        Some(serde_json::json!({"password": "N3wPasswordLongEnough!"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The reset succeeds.
    let (status, response) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"password": "N3wPasswordLongEnough!"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{response}");

    // The new password authenticates; the old one does not; the credential
    // revision moved so pre-reset issuance fences reject.
    let record = AuthStore::new()
        .user_for_auth(&db.pool, AuthLookup::Username("WP03-PLAIN"))
        .await
        .expect("lookup")
        .expect("user");
    assert!(
        melodee_auth::verify_password(
            b"N3wPasswordLongEnough!",
            record.password_hash_algorithm,
            record.password_hash.expose_secret(),
        )
        .expect("verify new")
    );
    assert!(
        !melodee_auth::verify_password(
            b"Mel0deeR0cks!",
            record.password_hash_algorithm,
            record.password_hash.expose_secret(),
        )
        .expect("verify old")
    );
    assert_eq!(record.user.auth_revision, 2);

    // Every refresh family for the account is revoked.
    let open_families: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM native_refresh_families
         WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(plain_internal)
    .fetch_one(&db.pool)
    .await
    .expect("family count");
    assert_eq!(open_families, 0);
    let reason: String = sqlx::query_scalar(
        "SELECT revocation_reason FROM native_refresh_families WHERE user_id=$1",
    )
    .bind(plain_internal)
    .fetch_one(&db.pool)
    .await
    .expect("revocation reason");
    assert_eq!(reason, "admin_password_reset");
    db.drop().await;
}
