//! Artist Identity Review API integration tests: the
//! administrator gate, the unresolved observation queue, observation detail
//! with candidate evidence, confirm/create-new/leave-provisional resolution,
//! do-not-match assertions, the identity decision feed, and the explicit
//! merge/split correction entry points.

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
use melodee_domain::{ArtistMatchOutcome, Confidence, DecisionAuthority, NewArtistMatchDecision};
use melodee_persistence::{AuthStore, CatalogStore, NativeUserImport};
use melodee_server::admin_access::AdminAccessState;
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-identity-jwt-key-least-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp13-admin", true, admin_public),
        ("wp13-plain", false, plain_public),
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

fn app(pool: sqlx::PgPool) -> axum::Router {
    melodee_server::admin_identity::router().with_state(AdminAccessState {
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

/// Seeds one cataloged Release (its import observations are auto-confirmed
/// and therefore resolved) and returns the fixture.
async fn seed_release(
    db: &TestDb,
    ordinal: u32,
    release_title: &'static str,
    artist: &'static str,
) -> phase5_common::CatalogedPolicyFixture {
    phase5_common::seed_cataloged_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title,
            release_artist: artist,
            track_title: "Identity Track",
            track_artist: artist,
            release_date: "2007-08-09",
        },
    )
    .await
}

/// Raw-inserts one unresolved observation against the seeded Release; the
/// catalog import never sees it, so it has no current decision.
async fn seed_unresolved_observation(
    db: &TestDb,
    release_id: Uuid,
    display: &str,
    normalized: &str,
    ordinal: i32,
) -> Uuid {
    let observation_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO observed_artists (
            id, release_id, track_id, scope, source_ordinal, raw_value,
            normalized_value, display_value, source_pointer, source_payload,
            created_at
         ) VALUES ($1,$2,NULL,'release',$3,$4,$5,$4,$6,$7,$8)",
    )
    .bind(observation_id)
    .bind(release_id)
    .bind(ordinal)
    .bind(display)
    .bind(normalized)
    .bind(format!("/fixture/artists/{ordinal}"))
    .bind(serde_json::json!({ "value": display }))
    .bind(OffsetDateTime::now_utc())
    .execute(&db.pool)
    .await
    .expect("seed unresolved observation");
    observation_id
}

/// Creates one verified Artist directly through the catalog store and
/// returns its public id.
async fn seed_artist(db: &TestDb, canonical: &str) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let artist = melodee_domain::Artist::new(melodee_domain::NewArtist {
        id: melodee_domain::ArtistId::new(),
        public_api_id: melodee_domain::ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: canonical.to_string(),
        normalized_name: canonical.to_lowercase(),
        sort_name: canonical.to_string(),
        artist_type: melodee_domain::ArtistType::Unknown,
        disambiguation: None,
        active_dates: None,
        status: melodee_domain::ArtistStatus::Verified,
        created_at: now,
    })
    .expect("valid artist");
    let public = artist.public_api_id().into_uuid();
    CatalogStore
        .create_artist(&db.pool, &artist)
        .await
        .expect("create artist");
    public
}

/// Attaches immutable evidence plus a proposed automated decision so the
/// observation's detail view has a candidate with evidence.
async fn seed_candidate_evidence(db: &TestDb, observation_id: Uuid, artist_public: Uuid) {
    let now = OffsetDateTime::now_utc();
    let artist = CatalogStore
        .artist_by_public_id(&db.pool, artist_public)
        .await
        .expect("artist lookup")
        .expect("artist exists");
    let evidence = melodee_domain::ArtistMatchEvidence {
        id: melodee_domain::ArtistMatchEvidenceId::new(),
        observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(observation_id),
        candidate_artist_id: artist.id,
        kind: melodee_domain::EvidenceKind::Alias,
        polarity: melodee_domain::EvidencePolarity::Supports,
        confidence: Confidence::new(7_500).expect("valid confidence"),
        source: "fixture-resolver".to_string(),
    };
    CatalogStore
        .record_artist_match_evidence(
            &db.pool,
            &evidence,
            serde_json::json!({ "alias": "fixture" }),
            now,
        )
        .await
        .expect("record evidence");
    let decision = melodee_domain::ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: melodee_domain::ArtistMatchDecisionId::new(),
            observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(observation_id),
            artist_id: artist.id,
            outcome: ArtistMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "fixture-resolver-v1".to_string(),
            },
            locked: false,
            reason: "alias match proposed".to_string(),
            decided_at: now,
        },
        std::slice::from_ref(&evidence),
    )
    .expect("valid proposal");
    CatalogStore
        .record_artist_match_decision(
            &db.pool,
            &decision,
            Confidence::new(7_500).expect("valid confidence"),
            serde_json::json!({}),
        )
        .await
        .expect("record proposal");
}

#[tokio::test]
async fn identity_routes_require_administrator() {
    let db = TestDb::create("wp13_identity_gate").await;
    let fixture = seed_users(&db).await;
    let app = app(db.pool.clone());

    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let plain = token_for(fixture.plain_public, "wp13-plain");
    let (status, _) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = token_for(fixture.admin_public, "wp13-admin");
    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);

    db.drop().await;
}

#[tokio::test]
async fn unresolved_queue_lists_only_unresolved_observations() {
    let db = TestDb::create("wp13_identity_queue").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 1, "Identity Queue Fixture", "Resolved Artist").await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Queue Candidate",
        "queue candidate",
        11,
    )
    .await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1);
    let item = &body["items"][0];
    assert_eq!(item["id"].as_str().expect("id"), observation.to_string());
    assert_eq!(item["displayValue"], "Queue Candidate");
    assert_eq!(item["releaseTitle"], "Identity Queue Fixture");
    assert_eq!(item["pendingMatchCount"], 0);
    assert_eq!(item["evidenceCount"], 0);

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/identity/unresolved?sort=bogus",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_sort");

    db.drop().await;
}

#[tokio::test]
async fn observation_detail_shows_candidates_with_evidence() {
    let db = TestDb::create("wp13_identity_detail").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 2, "Identity Detail Fixture", "Resolved Artist").await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Detail Candidate",
        "detail candidate",
        12,
    )
    .await;
    let candidate_public = seed_artist(&db, "Detail Candidate").await;
    seed_candidate_evidence(&db, observation, candidate_public).await;
    let app = app(db.pool.clone());

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/identity/observations/{observation}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["current"].is_null(), "no authoritative decision yet");
    let candidates = body["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(
        candidate["artistId"].as_str().expect("artistId"),
        candidate_public.to_string()
    );
    assert_eq!(candidate["artistName"], "Detail Candidate");
    assert_eq!(candidate["latestOutcome"], "proposed");
    assert_eq!(candidate["latestConfidence"], 7_500);
    assert_eq!(candidate["decisionCount"], 1);
    let evidence = candidate["evidence"].as_array().expect("evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["kind"], "alias");
    assert_eq!(evidence[0]["polarity"], "supports");
    assert_eq!(evidence[0]["confidence"], 7_500);
    assert!(
        body["negativeAssertions"]
            .as_array()
            .expect("assertions")
            .is_empty()
    );

    // Queue row reflects the pending candidate counts.
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["pendingMatchCount"], 1);
    assert_eq!(body["items"][0]["evidenceCount"], 1);

    let (status, body) = call(
        app,
        "GET",
        &format!("/api/admin/v1/identity/observations/{}", Uuid::now_v7()),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    db.drop().await;
}

#[tokio::test]
async fn resolve_confirm_creates_locked_authoritative_decision() {
    let db = TestDb::create("wp13_identity_confirm").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 3, "Identity Confirm Fixture", "Resolved Artist").await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Confirm Me",
        "confirm me",
        13,
    )
    .await;
    let candidate_public = seed_artist(&db, "Confirm Me").await;
    let app = app(db.pool.clone());
    let path = format!("/api/admin/v1/identity/observations/{observation}/resolve");

    // Validation first: missing artistId, blank reason, unknown resolution.
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"resolution": "confirm", "reason": "looks right"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "missing_artist_id");
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "resolution": "confirm",
            "artistId": candidate_public,
            "reason": "  "
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_reason");
    let (status, _body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "resolution": "confirm",
            "artistId": Uuid::now_v7(),
            "reason": "unknown artist"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({"resolution": "guess", "reason": "nope"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_resolution");

    // Confirm: the observation leaves the queue with a locked decision.
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "resolution": "confirm",
            "artistId": candidate_public,
            "reason": "verified against booklet"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["resolution"], "confirm");
    assert_eq!(
        body["artistId"].as_str().expect("artistId"),
        candidate_public.to_string()
    );
    assert_eq!(body["changed"], true);

    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0, "confirmed observation leaves the queue");

    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/identity/observations/{observation}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let current = &body["current"];
    assert_eq!(current["outcome"], "confirmed");
    assert_eq!(current["authority"], "admin");
    assert_eq!(current["confidence"], 10_000);
    assert_eq!(current["locked"], true);
    assert_eq!(current["reason"], "verified against booklet");
    assert_eq!(
        current["artistId"].as_str().expect("artistId"),
        candidate_public.to_string()
    );

    db.drop().await;
}

#[tokio::test]
async fn resolve_create_new_and_leave_provisional() {
    let db = TestDb::create("wp13_identity_create_leave").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 4, "Identity Create Fixture", "Resolved Artist").await;
    let app = app(db.pool.clone());

    // leave-provisional is an audited no-op: the observation stays queued.
    let leave_id = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Leave Me",
        "leave me",
        14,
    )
    .await;
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{leave_id}/resolve"),
        Some(serde_json::json!({
            "resolution": "leave-provisional",
            "reason": "needs a second pass"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], false);
    assert!(body["artistId"].is_null());
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["total"], 1,
        "leave-provisional keeps the observation queued"
    );

    // create-new rejects an artistId and otherwise creates + confirms.
    let create_id = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Create Me",
        "create me",
        15,
    )
    .await;
    let path = format!("/api/admin/v1/identity/observations/{create_id}/resolve");
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "resolution": "create-new",
            "artistId": Uuid::now_v7(),
            "reason": "bad request"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unexpected_artist_id");

    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "resolution": "create-new",
            "reason": "distinct artist confirmed"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["resolution"], "create-new");
    assert_eq!(body["changed"], true);
    let new_public = body["artistId"]
        .as_str()
        .expect("new artist id")
        .to_string();

    // The new Artist is provisional and the observation is resolved.
    let created = CatalogStore
        .artist_by_public_id(&db.pool, Uuid::parse_str(&new_public).expect("uuid"))
        .await
        .expect("lookup")
        .expect("created artist");
    assert_eq!(created.status, "provisional");
    assert_eq!(created.canonical_name, "Create Me");
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/identity/unresolved",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1, "only the leave-provisional row remains");

    db.drop().await;
}

#[tokio::test]
async fn never_match_records_and_rejects_duplicates() {
    let db = TestDb::create("wp13_identity_never_match").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 5, "Identity Never Fixture", "Resolved Artist").await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Never This",
        "never this",
        16,
    )
    .await;
    let candidate_public = seed_artist(&db, "Never This").await;
    let app = app(db.pool.clone());
    let path = format!("/api/admin/v1/identity/observations/{observation}/never-match");

    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "artistId": candidate_public,
            "reason": "different person despite the name"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["recorded"], true);

    // The detail view exposes the recorded assertion.
    let (status, body) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/identity/observations/{observation}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let assertions = body["negativeAssertions"].as_array().expect("assertions");
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0]["reason"], "different person despite the name");
    assert_eq!(assertions[0]["locked"], true);

    // Repeating the exact pair conflicts with the durable assertion.
    let (status, body) = call(
        app.clone(),
        "POST",
        &path,
        Some(serde_json::json!({
            "artistId": candidate_public,
            "reason": "duplicate attempt"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let (status, _body) = call(
        app,
        "POST",
        &path,
        Some(serde_json::json!({
            "artistId": Uuid::now_v7(),
            "reason": "unknown artist"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    db.drop().await;
}

#[tokio::test]
async fn decisions_feed_lists_identity_events_newest_first() {
    let db = TestDb::create("wp13_identity_feed").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 6, "Identity Feed Fixture", "Resolved Artist").await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Feed Candidate",
        "feed candidate",
        17,
    )
    .await;
    let candidate_public = seed_artist(&db, "Feed Candidate").await;
    let app = app(db.pool.clone());

    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/resolve"),
        Some(serde_json::json!({
            "resolution": "confirm",
            "artistId": candidate_public,
            "reason": "feed verification"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        app,
        "GET",
        "/api/admin/v1/identity/decisions",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    let entry = items
        .iter()
        .find(|entry| {
            entry["eventType"] == "artist_match_decision_recorded"
                && entry["entityId"].as_str() == Some(observation.to_string().as_str())
        })
        .expect("confirmation event in feed");
    assert_eq!(entry["entityType"], "observed_artist");
    assert_eq!(entry["reason"], "feed verification");
    assert_eq!(
        entry["actor"].as_str().expect("actor"),
        fixture.admin_public.to_string()
    );

    db.drop().await;
}

#[tokio::test]
async fn merge_redirects_source_and_blocks_further_source_mutations() {
    let db = TestDb::create("wp13_identity_merge").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let source_public = seed_artist(&db, "Merge Source").await;
    let target_public = seed_artist(&db, "Merge Target").await;
    let app = app(db.pool.clone());

    // Self-merge and blank reason are rejected up front.
    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/identity/artists/merge",
        Some(serde_json::json!({
            "sourceArtistId": source_public,
            "targetArtistId": source_public,
            "reason": "same"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "self_merge");

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/identity/artists/merge",
        Some(serde_json::json!({
            "sourceArtistId": source_public,
            "targetArtistId": target_public,
            "reason": "duplicate identities confirmed"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["mergeId"].is_string());
    assert_eq!(
        body["sourceArtistId"].as_str().expect("source"),
        source_public.to_string()
    );
    assert_eq!(
        body["targetArtistId"].as_str().expect("target"),
        target_public.to_string()
    );

    // The merged source can no longer participate in identity mutations.
    let merged = CatalogStore
        .artist_by_public_id(&db.pool, source_public)
        .await
        .expect("lookup")
        .expect("source still resolvable as redirect");
    assert_eq!(merged.status, "merged");

    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/identity/artists/merge",
        Some(serde_json::json!({
            "sourceArtistId": source_public,
            "targetArtistId": target_public,
            "reason": "again"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "artist_not_resolvable");

    db.drop().await;
}

#[tokio::test]
async fn batch_merge_previews_impact_and_applies_all_sources_atomically() {
    let db = TestDb::create("wp13_identity_batch_merge").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let source_one = seed_artist(&db, "Batch Source One").await;
    let source_two = seed_artist(&db, "Batch Source Two").await;
    let target = seed_artist(&db, "Batch Target").await;
    let app = app(db.pool.clone());
    let selection = serde_json::json!({
        "sourceArtistIds": [source_one, source_two],
        "targetArtistId": target
    });

    let (status, _) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/identity/artists/merge/preview",
        Some(selection.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/identity/artists/merge/preview",
        Some(selection.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["target"]["id"], target.to_string());
    assert_eq!(body["sources"].as_array().expect("sources").len(), 2);
    assert_eq!(body["impact"]["releases"], 0);
    assert!(
        body["metadataConflicts"]
            .as_array()
            .expect("metadata conflicts")
            .contains(&serde_json::json!("canonicalName"))
    );

    let (status, body) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/identity/artists/merge/preview",
        Some(serde_json::json!({
            "sourceArtistIds": [source_one, source_one],
            "targetArtistId": target
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "duplicate_source");

    let (status, body) = call(
        app,
        "POST",
        "/api/admin/v1/identity/artists/merge/batch",
        Some(serde_json::json!({
            "sourceArtistIds": [source_one, source_two],
            "targetArtistId": target,
            "reason": "confirmed duplicate Artists"
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["targetArtistId"], target.to_string());
    assert_eq!(body["mergeIds"].as_array().expect("merge ids").len(), 2);
    assert_eq!(
        body["sourceArtistIds"]
            .as_array()
            .expect("source ids")
            .len(),
        2
    );

    let merged: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='merged'),
                (SELECT count(*) FROM artist_merge_events
                  WHERE source_artist_id=ANY(
                      SELECT id FROM artists WHERE public_api_id=ANY($1)))
           FROM artists WHERE public_api_id=ANY($1)",
    )
    .bind([source_one, source_two])
    .fetch_one(&db.pool)
    .await
    .expect("read batch merge result");
    assert_eq!(merged, (2, 2));

    db.drop().await;
}

#[tokio::test]
async fn split_moves_only_the_selected_assignments() {
    let db = TestDb::create("wp13_identity_split").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp13-admin");
    let release = seed_release(&db, 7, "Identity Split Fixture", "Split Artist").await;
    let app = app(db.pool.clone());

    // The import linked the Release to its provisional Artist; that link is
    // the explicit assignment the split moves.
    let (link_id, artist_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT id, artist_id FROM release_artists WHERE release_id = $1")
            .bind(release.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("release artist link");
    let split_path = format!("/api/admin/v1/identity/artists/{artist_id}/split");

    // Validation: empty assignments, unknown kind, blank new name.
    let (status, body) = call(
        app.clone(),
        "POST",
        &split_path,
        Some(serde_json::json!({
            "reason": "nothing selected",
            "newArtist": {"canonicalName": "Split Off"},
            "assignments": []
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "empty_split");
    let (status, body) = call(
        app.clone(),
        "POST",
        &split_path,
        Some(serde_json::json!({
            "reason": "bad kind",
            "newArtist": {"canonicalName": "Split Off"},
            "assignments": [{"kind": "song", "id": link_id}]
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_assignment_kind");
    let (status, body) = call(
        app.clone(),
        "POST",
        &split_path,
        Some(serde_json::json!({
            "reason": "blank name",
            "newArtist": {"canonicalName": "   "},
            "assignments": [{"kind": "releaseArtist", "id": link_id}]
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_artist_name");

    let (status, body) = call(
        app.clone(),
        "POST",
        &split_path,
        Some(serde_json::json!({
            "reason": "release belongs to a different artist",
            "newArtist": {"canonicalName": "Split Off", "artistType": "person"},
            "assignments": [{"kind": "releaseArtist", "id": link_id}]
        })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["splitId"].is_string());
    assert_eq!(body["movedAssignments"], 1);
    let new_public = body["newArtistId"].as_str().expect("new artist id");

    // The link now points at the new provisional Artist.
    let moved: (Uuid,) = sqlx::query_as("SELECT artist_id FROM release_artists WHERE id = $1")
        .bind(link_id)
        .fetch_one(&db.pool)
        .await
        .expect("link row");
    let new_artist = CatalogStore
        .artist_by_public_id(&db.pool, Uuid::parse_str(new_public).expect("uuid"))
        .await
        .expect("lookup")
        .expect("new artist");
    assert_eq!(moved.0, new_artist.id.into_uuid());
    assert_eq!(new_artist.status, "provisional");

    db.drop().await;
}
