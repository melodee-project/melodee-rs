//! Admin duplicate-candidate, Release merge/split, and agent-proposal API
//! integration tests (ADR-0196).
//!
//! These exercise the real PostgreSQL catalog: candidate scans over artists
//! and releases, the bounded Release merge preview/apply/split flow over the
//! existing audited store operations, and the non-authoritative proposal path
//! used by AI harnesses.

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
use melodee_domain::{Artist, ArtistId, ArtistPublicId, ArtistStatus, ArtistType, NewArtist};
use melodee_persistence::{AuthStore, CatalogStore, NativeUserImport};
use serde_json::Value;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_KEY: &str = "test-admin-duplicates-jwt-key-32b";

struct Fixture {
    admin_public: Uuid,
    plain_public: Uuid,
}

async fn seed_users(db: &TestDb) -> Fixture {
    let now = OffsetDateTime::now_utc();
    let admin_public = Uuid::now_v7();
    let plain_public = Uuid::now_v7();
    for (username, is_admin, public_id) in [
        ("wp16-admin", true, admin_public),
        ("wp16-plain", false, plain_public),
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
    axum::Router::new()
        .merge(melodee_server::admin_identity::router().with_state(state.clone()))
        .merge(melodee_server::admin_duplicates::router().with_state(state.clone()))
        .merge(melodee_server::admin_release_identity::router().with_state(state))
}

async fn call(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    agent: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(agent) = agent {
        builder = builder.header("x-melodee-agent", agent);
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

async fn seed_artist(
    db: &TestDb,
    canonical: &str,
    normalized: &str,
    artist_type: ArtistType,
    disambiguation: Option<&str>,
) -> (Uuid, ArtistId) {
    let artist = Artist::new(NewArtist {
        id: ArtistId::new(),
        public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: canonical.to_string(),
        normalized_name: normalized.to_string(),
        sort_name: canonical.to_string(),
        artist_type,
        disambiguation: disambiguation.map(str::to_string),
        active_dates: None,
        status: ArtistStatus::Active,
        created_at: OffsetDateTime::now_utc(),
    })
    .expect("valid artist");
    let public = artist.public_api_id().into_uuid();
    let internal = artist.id();
    CatalogStore
        .create_artist(&db.pool, &artist)
        .await
        .expect("create artist");
    (public, internal)
}

async fn add_alias(db: &TestDb, artist: ArtistId, alias: &str) {
    sqlx::query(
        "INSERT INTO artist_aliases (
            id, artist_id, alias, normalized_alias, alias_type, locale,
            evidence_source, confidence, is_locked, created_at
         ) VALUES ($1,$2,$3,$4,'search',NULL,'test-fixture',9000,false,now())",
    )
    .bind(Uuid::now_v7())
    .bind(artist.into_uuid())
    .bind(alias)
    .bind(alias.to_lowercase())
    .execute(&db.pool)
    .await
    .expect("insert alias");
}

async fn add_external_identity(db: &TestDb, artist: ArtistId, provider: &str, external_id: &str) {
    sqlx::query(
        "INSERT INTO artist_external_identities (
            id, artist_id, entity_type, provider, external_id,
            provider_unique_scope, confidence, evidence_source,
            verified_at, is_locked, created_at
         ) VALUES ($1,$2,'artist',$3,$4,false,9000,'test-fixture',NULL,false,now())",
    )
    .bind(Uuid::now_v7())
    .bind(artist.into_uuid())
    .bind(provider)
    .bind(external_id)
    .execute(&db.pool)
    .await
    .expect("insert external identity");
}

async fn add_provider_candidate(db: &TestDb, artist: ArtistId, provider: &str, external_id: &str) {
    sqlx::query(
        "INSERT INTO provider_identity_candidates (
            id, artist_id, provider, external_id, query, candidate_rank,
            confidence, payload, observed_at
         ) VALUES ($1,$2,$3,$4,'fixture',1,9000,'{}'::jsonb,now())",
    )
    .bind(Uuid::now_v7())
    .bind(artist.into_uuid())
    .bind(provider)
    .bind(external_id)
    .execute(&db.pool)
    .await
    .expect("insert provider candidate");
}

async fn seed_unresolved_observation(
    db: &TestDb,
    release_internal: Uuid,
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
         ) VALUES ($1,$2,NULL,'release',$3,$4,$5,$4,$6,$7,now())",
    )
    .bind(observation_id)
    .bind(release_internal)
    .bind(ordinal)
    .bind(display)
    .bind(normalized)
    .bind(format!("/fixture/artists/{ordinal}"))
    .bind(serde_json::json!({ "value": display }))
    .execute(&db.pool)
    .await
    .expect("seed unresolved observation");
    observation_id
}

#[tokio::test]
async fn duplicate_routes_require_administrator() {
    let db = TestDb::create("wp16_duplicate_gate").await;
    let fixture = seed_users(&db).await;
    let app = app(db.pool.clone());
    for path in [
        "/api/admin/v1/catalog/duplicates/artists",
        "/api/admin/v1/catalog/duplicates/releases",
    ] {
        let (status, _) = call(app.clone(), "GET", path, None, None, None).await;
        assert_eq!(StatusCode::UNAUTHORIZED, status);
        let plain = token_for(fixture.plain_public, "wp16-plain");
        let (status, _) = call(app.clone(), "GET", path, None, Some(&plain), None).await;
        assert_eq!(StatusCode::FORBIDDEN, status);
        let admin = token_for(fixture.admin_public, "wp16-admin");
        let (status, body) = call(app.clone(), "GET", path, None, Some(&admin), None).await;
        assert_eq!(StatusCode::OK, status, "{body}");
        assert_eq!(body["total"], 0);
    }
    db.drop().await;
}

#[tokio::test]
async fn artist_duplicate_scan_reports_evidence_blockers_and_filters() {
    let db = TestDb::create("wp16_artist_duplicates").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");

    let (base_public, base) = seed_artist(&db, "Nirvana", "nirvana", ArtistType::Group, None).await;
    let (same_name_public, _) =
        seed_artist(&db, "Nirvana", "nirvana", ArtistType::Group, None).await;
    let (disambiguated_public, _) = seed_artist(
        &db,
        "Nirvana",
        "nirvana",
        ArtistType::Group,
        Some("US grunge band"),
    )
    .await;
    let (alias_public, alias_artist) = seed_artist(
        &db,
        "Nirvana Tribute",
        "nirvana tribute",
        ArtistType::Group,
        None,
    )
    .await;
    add_alias(&db, alias_artist, "Nirvana").await;
    let (external_public, external_artist) = seed_artist(
        &db,
        "Nirvana Legacy",
        "nirvana legacy",
        ArtistType::Group,
        None,
    )
    .await;
    add_external_identity(&db, base, "musicbrainz", "mbid-nirvana").await;
    add_external_identity(&db, external_artist, "musicbrainz", "mbid-nirvana").await;
    let (candidate_public, candidate_artist) = seed_artist(
        &db,
        "Nirvana Provider",
        "nirvana provider",
        ArtistType::Group,
        None,
    )
    .await;
    add_provider_candidate(&db, base, "musicbrainz", "mbid-result-1").await;
    add_provider_candidate(&db, candidate_artist, "musicbrainz", "mbid-result-1").await;
    let (conflict_public, conflict_artist) =
        seed_artist(&db, "Nirvana", "nirvana", ArtistType::Group, None).await;
    add_external_identity(&db, base, "spotify", "spotify-a").await;
    add_external_identity(&db, conflict_artist, "spotify", "spotify-b").await;

    let app = app(db.pool.clone());
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/duplicates/artists?limit=100&minScore=0",
        None,
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    assert!(body["total"].as_i64().unwrap_or_default() > 0);
    let items = body["items"].as_array().expect("items");

    let pair_for = |left: Uuid, right: Uuid| {
        items.iter().find(|item| {
            let left_id = item["left"]["id"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok());
            let right_id = item["right"]["id"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok());
            (left_id == Some(left) && right_id == Some(right))
                || (left_id == Some(right) && right_id == Some(left))
        })
    };

    let name_only = pair_for(base_public, same_name_public).expect("same-name pair");
    assert!(
        name_only["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["kind"] == "name_only")
    );

    let disambiguated_pair =
        pair_for(base_public, disambiguated_public).expect("disambiguation pair");
    assert!(
        disambiguated_pair["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["kind"] == "disambiguation_conflict")
    );

    let alias_pair = pair_for(base_public, alias_public).expect("alias-overlap pair");
    assert!(
        alias_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "alias_overlap")
    );

    let external_pair = pair_for(base_public, external_public).expect("shared-external pair");
    assert!(
        external_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "shared_trusted_external_id")
    );
    assert!(external_pair["score"].as_i64().unwrap_or_default() >= 6000);

    let provider_pair =
        pair_for(base_public, candidate_public).expect("shared-provider-candidate pair");
    assert!(
        provider_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "shared_provider_candidate")
    );

    let conflict_pair = pair_for(base_public, conflict_public).expect("provider-conflict pair");
    assert!(
        conflict_pair["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["kind"] == "trusted_provider_conflict")
    );

    // High minimum score drops name-only and provider-candidate pairs.
    let (status, filtered) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/duplicates/artists?limit=100&minScore=6000",
        None,
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert!(
        filtered["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["score"].as_i64().unwrap_or_default() >= 6000)
    );
    assert!(
        filtered["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["score"].as_i64().unwrap_or_default() >= 6000)
    );

    // Pagination remains bounded and reports the unpaged total.
    let (status, page) = call(
        app,
        "GET",
        "/api/admin/v1/catalog/duplicates/artists?limit=1&minScore=0",
        None,
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(1, page["items"].as_array().unwrap().len());
    assert!(page["total"].as_i64().unwrap_or_default() > 1);

    // Everything above is advisory: no identity decision or merge happened.
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM artist_match_decisions")
        .fetch_one(&db.pool)
        .await
        .expect("count decisions");
    assert_eq!(0, decisions);
    let merged: i64 = sqlx::query_scalar("SELECT count(*) FROM artists WHERE status='merged'")
        .fetch_one(&db.pool)
        .await
        .expect("count merged artists");
    assert_eq!(0, merged);

    db.drop().await;
}

#[tokio::test]
async fn release_duplicate_scan_merge_preview_merge_and_split() {
    let db = TestDb::create("wp16_release_duplicates").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");

    let first = phase5_common::seed_cataloged_release(
        &db,
        201,
        CatalogFixturePresentation {
            release_title: "Duplicate Source",
            release_artist: "Duplicate Artist",
            track_title: "Shared Recording",
            track_artist: "Duplicate Artist",
            release_date: "2001-01-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        202,
        CatalogFixturePresentation {
            release_title: "Duplicate Target",
            release_artist: "Duplicate Artist",
            track_title: "Shared Recording",
            track_artist: "Duplicate Artist",
            release_date: "2001-01-01",
        },
    )
    .await;
    let third = phase5_common::seed_cataloged_release(
        &db,
        203,
        CatalogFixturePresentation {
            release_title: "Duplicate Target",
            release_artist: "Duplicate Artist",
            track_title: "Different Recording",
            track_artist: "Duplicate Artist",
            release_date: "2001-01-01",
        },
    )
    .await;

    let first_internal = first.release_id().into_uuid();
    let second_internal = second.release_id().into_uuid();
    for release in [first_internal, second_internal] {
        sqlx::query(
            "INSERT INTO release_external_identities (
                id, release_id, provider, external_id, trusted,
                confidence, evidence_source, is_locked, observed_at
             ) VALUES ($1,$2,'musicbrainz','mbid-shared-release',true,9500,'test-fixture',false,now())",
        )
        .bind(Uuid::now_v7())
        .bind(release)
        .execute(&db.pool)
        .await
        .expect("insert trusted release identity");
    }
    // Make the second Release share the first's audio identity so the scan
    // has both provider and recording evidence.
    sqlx::query(
        "UPDATE tracks target
            SET audio_identity_id = source.audio_identity_id
           FROM tracks source
          WHERE target.release_id = $2
            AND source.release_id = $1",
    )
    .bind(first_internal)
    .bind(second_internal)
    .execute(&db.pool)
    .await
    .expect("share audio identity");

    let third_internal = third.release_id().into_uuid();
    let first_public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(first_internal)
        .fetch_one(&db.pool)
        .await
        .expect("first public id");
    let second_public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(second_internal)
        .fetch_one(&db.pool)
        .await
        .expect("second public id");
    let third_public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
        .bind(third_internal)
        .fetch_one(&db.pool)
        .await
        .expect("third public id");

    let app = app(db.pool.clone());
    let (status, body) = call(
        app.clone(),
        "GET",
        "/api/admin/v1/catalog/duplicates/releases?limit=100&minScore=0",
        None,
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{body}");
    let items = body["items"].as_array().expect("items");
    let pair_for = |left: Uuid, right: Uuid| {
        items.iter().find(|item| {
            let left_id = item["left"]["id"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok());
            let right_id = item["right"]["id"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok());
            (left_id == Some(left) && right_id == Some(right))
                || (left_id == Some(right) && right_id == Some(left))
        })
    };
    let provider_pair =
        pair_for(first_public, second_public).expect("shared provider/recording pair");
    assert!(
        provider_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "shared_trusted_external_id")
    );
    assert!(
        provider_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "shared_audio_identity")
    );
    assert!(provider_pair["score"].as_i64().unwrap_or_default() >= 6000);

    let title_pair = pair_for(second_public, third_public).expect("title pair");
    assert!(
        title_pair["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "same_title_and_display_artist")
    );
    assert!(
        title_pair["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["kind"] == "no_shared_recording_or_provider_evidence")
    );

    // Preview reports the impact without mutating anything.
    let preview_body = serde_json::json!({
        "sourceReleaseId": first_public,
        "targetReleaseId": second_public,
    });
    let (status, preview) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/preview",
        Some(preview_body.clone()),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(1, preview["impact"]["movedTracks"]);
    assert_eq!(2, preview["impact"]["targetTrackCountAfter"]);
    assert_eq!(1, preview["impact"]["sharedAudioIdentities"]);
    assert!(
        preview["metadataConflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "title")
    );

    // Merge through the API, then verify the durable source tombstone.
    let merge_body = serde_json::json!({
        "sourceReleaseId": first_public,
        "targetReleaseId": second_public,
        "reason": "same trusted MusicBrainz release and recording",
    });
    let (status, merged) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge",
        Some(merge_body),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{merged}");
    let merge_event: Uuid = merged["mergeEventId"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("merge event id");
    let (stored_status, merged_into) = {
        let row: (String, Option<Uuid>) =
            sqlx::query_as("SELECT status, merged_into_release_id FROM releases WHERE id=$1")
                .bind(first_internal)
                .fetch_one(&db.pool)
                .await
                .expect("read merged source");
        (row.0, row.1)
    };
    assert_eq!("merged", stored_status);
    assert_eq!(Some(second_internal), merged_into);
    let target_tracks: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
        .bind(second_internal)
        .fetch_one(&db.pool)
        .await
        .expect("count target tracks");
    assert_eq!(2, target_tracks);

    // The operator detail read exposes the tombstone target, the trusted
    // provider evidence, and the reversible merge event.
    let detail = CatalogStore
        .release_catalog_detail(&db.pool, first_public)
        .await
        .expect("read merged source detail")
        .expect("merged source detail exists");
    assert_eq!(Some(second_public), detail.merged_into_public_id);
    assert!(
        detail
            .merge_events
            .items
            .iter()
            .any(|event| event.id == merge_event)
    );
    assert!(
        detail
            .external_identities
            .items
            .iter()
            .any(|identity| identity.provider == "musicbrainz" && identity.trusted)
    );

    // Split through the API using the merge event from the response.
    let split_body = serde_json::json!({
        "mergeEventId": merge_event,
        "reason": "separate editions after review",
    });
    let (status, split) = call(
        app,
        "POST",
        &format!("/api/admin/v1/catalog/releases/{first_public}/split"),
        Some(split_body),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{split}");
    assert_eq!(true, split["restored"]);
    let (restored_status, restored_owner): (String, Uuid) = {
        let status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
            .bind(first_internal)
            .fetch_one(&db.pool)
            .await
            .expect("restored status");
        let owner: Uuid = sqlx::query_scalar(
            "SELECT release_id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1",
        )
        .bind(first_internal)
        .fetch_one(&db.pool)
        .await
        .expect("restored track owner");
        (status, owner)
    };
    assert_eq!("active", restored_status);
    assert_eq!(first_internal, restored_owner);
    let split_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM release_split_events WHERE merge_event_id=$1")
            .bind(merge_event)
            .fetch_one(&db.pool)
            .await
            .expect("count split events");
    assert_eq!(1, split_events);

    db.drop().await;
}

#[tokio::test]
async fn agent_proposal_is_evidence_backed_and_non_authoritative() {
    let db = TestDb::create("wp16_agent_proposal").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");
    let plain = token_for(fixture.plain_public, "wp16-plain");

    let release = phase5_common::seed_cataloged_release(
        &db,
        301,
        CatalogFixturePresentation {
            release_title: "Proposal Fixture",
            release_artist: "Proposal Artist",
            track_title: "Proposal Track",
            track_artist: "Proposal Artist",
            release_date: "2012-03-04",
        },
    )
    .await;
    let observation = seed_unresolved_observation(
        &db,
        release.release_id().into_uuid(),
        "Proposed Name",
        "proposed name",
        0,
    )
    .await;
    let (artist_public, _) = seed_artist(
        &db,
        "Proposed Name",
        "proposed name",
        ArtistType::Person,
        None,
    )
    .await;

    let app = app(db.pool.clone());
    let body = serde_json::json!({
        "artistId": artist_public,
        "confidence": 8200,
        "reason": "exact normalized name plus shared release context",
        "evidence": [
            {
                "kind": "name_similarity",
                "polarity": "supports",
                "confidence": 8200,
                "payload": { "normalized": "proposed name" }
            },
            {
                "kind": "release_context",
                "polarity": "supports",
                "confidence": 7000,
                "payload": { "release": "Proposal Fixture" }
            }
        ]
    });
    let (status, proposed) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/propose"),
        Some(body.clone()),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{proposed}");
    assert_eq!("proposed", proposed["outcome"]);
    assert_eq!(true, proposed["changed"]);

    // The proposal is visible in observation detail with its evidence, but
    // it is not the authoritative current decision.
    let (status, detail) = call(
        app.clone(),
        "GET",
        &format!("/api/admin/v1/identity/observations/{observation}"),
        None,
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{detail}");
    assert!(detail["current"].is_null());
    let candidates = detail["candidates"].as_array().expect("candidates");
    let candidate = candidates
        .iter()
        .find(|candidate| candidate["artistId"] == artist_public.to_string())
        .expect("proposed candidate");
    assert_eq!("proposed", candidate["latestOutcome"]);
    assert!(candidate["evidence"].as_array().unwrap().len() >= 2);

    // Authority, lock, and agent attribution are durable and non-authoritative.
    let (authority, locked, agent_payload): (String, bool, Value) = sqlx::query_as(
        "SELECT authority, is_locked, payload FROM artist_match_decisions
          WHERE observed_artist_id=$1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(observation)
    .fetch_one(&db.pool)
    .await
    .expect("read proposal decision");
    assert_eq!("automated", authority);
    assert!(!locked);
    assert_eq!("test-harness", agent_payload["agent"]);

    // A listener cannot propose.
    let (status, _) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/propose"),
        Some(body.clone()),
        Some(&plain),
        None,
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, status);

    // Invalid evidence is rejected before any write.
    let mut invalid = body.clone();
    invalid["evidence"][0]["kind"] = Value::String("made_up".to_string());
    let (status, error) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/propose"),
        Some(invalid),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("invalid_evidence_kind", error["code"]);

    // An observation with an authoritative decision cannot be re-proposed.
    let resolve = serde_json::json!({
        "resolution": "confirm",
        "artistId": artist_public,
        "reason": "administrator confirmed after agent proposal",
    });
    let (status, resolved) = call(
        app.clone(),
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/resolve"),
        Some(resolve),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{resolved}");
    let (status, error) = call(
        app,
        "POST",
        &format!("/api/admin/v1/identity/observations/{observation}/propose"),
        Some(body),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("already_resolved", error["code"]);

    db.drop().await;
}

#[tokio::test]
async fn release_batch_merge_folds_selected_releases_from_any_artist() {
    let db = TestDb::create("wp16_release_batch_merge").await;
    let fixture = seed_users(&db).await;
    let admin = token_for(fixture.admin_public, "wp16-admin");

    // Three Releases from three different Artists: the administrator may pick
    // any two as sources and fold them into the third.
    let first = phase5_common::seed_cataloged_release(
        &db,
        301,
        CatalogFixturePresentation {
            release_title: "Batch Source One",
            release_artist: "First Artist",
            track_title: "First Recording",
            track_artist: "First Artist",
            release_date: "2001-01-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        302,
        CatalogFixturePresentation {
            release_title: "Batch Source Two",
            release_artist: "Second Artist",
            track_title: "Second Recording",
            track_artist: "Second Artist",
            release_date: "2002-02-02",
        },
    )
    .await;
    let target = phase5_common::seed_cataloged_release(
        &db,
        303,
        CatalogFixturePresentation {
            release_title: "Batch Target",
            release_artist: "Target Artist",
            track_title: "Target Recording",
            track_artist: "Target Artist",
            release_date: "2003-03-03",
        },
    )
    .await;

    let first_internal = first.release_id().into_uuid();
    let second_internal = second.release_id().into_uuid();
    let target_internal = target.release_id().into_uuid();
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
    let first_public = public_id(first_internal).await;
    let second_public = public_id(second_internal).await;
    let target_public = public_id(target_internal).await;

    // The source owns artwork the target lacks: the merge adopts it.
    sqlx::query(
        "INSERT INTO catalog_entity_images (
             entity_type,release_id,image_mime_type,asset_key,uploaded_by,created_at,updated_at
         ) VALUES ('release',$1,'image/png',$2,'curator',now(),now())",
    )
    .bind(first_internal)
    .bind(
        melodee_persistence::asset_files::put(&db.pool, &[0x89_u8, 0x50, 0x4e, 0x47, 5])
            .await
            .expect("store fixture"),
    )
    .execute(&db.pool)
    .await
    .expect("seed source operator image");

    let app = app(db.pool.clone());
    let (status, preview) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/preview",
        Some(serde_json::json!({
            "sourceReleaseIds": [first_public, second_public],
            "targetReleaseId": target_public,
        })),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{preview}");
    assert_eq!(2, preview["sources"].as_array().expect("sources").len());
    assert_eq!(2, preview["impact"]["movedTracks"]);
    assert_eq!(3, preview["impact"]["targetTrackCountAfter"]);
    assert_eq!(1, preview["artwork"]["operatorImagesAdopted"]);
    assert_eq!(target_public.to_string(), preview["target"]["id"]);
    let conflicts = preview["metadataConflicts"]
        .as_array()
        .expect("conflicts")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(conflicts.contains(&"title"));
    assert!(conflicts.contains(&"displayArtist"));
    assert!(conflicts.contains(&"releaseArtist"));

    let (status, merged) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/batch",
        Some(serde_json::json!({
            "sourceReleaseIds": [first_public, second_public],
            "targetReleaseId": target_public,
            "reason": "same Release imported twice under two Artists",
        })),
        Some(&admin),
        Some("test-harness"),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{merged}");
    assert_eq!(2, merged["mergeCount"]);
    let merge_events = merged["mergeEvents"]
        .as_array()
        .expect("merge events")
        .iter()
        .map(|event| {
            (
                Uuid::parse_str(event["sourceReleaseId"].as_str().expect("source id"))
                    .expect("source uuid"),
                Uuid::parse_str(event["mergeEventId"].as_str().expect("merge event id"))
                    .expect("merge event uuid"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(2, merge_events.len());
    assert_eq!(target_public.to_string(), merged["targetReleaseId"]);
    let first_event = merge_events
        .iter()
        .find(|(source, _)| *source == first_public)
        .map(|(_, event)| *event)
        .expect("merge event for the first source");

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='active'),
                count(*) FILTER (WHERE status='merged')
           FROM releases WHERE id=ANY($1)",
    )
    .bind([first_internal, second_internal, target_internal])
    .fetch_one(&db.pool)
    .await
    .expect("read Release statuses");
    assert_eq!(statuses, (1, 2));
    let target_tracks: Vec<i32> =
        sqlx::query_scalar("SELECT ordinal FROM tracks WHERE release_id=$1 ORDER BY ordinal")
            .bind(target_internal)
            .fetch_all(&db.pool)
            .await
            .expect("read surviving Track ordinals");
    assert_eq!(target_tracks, vec![1, 2, 3]);

    // The surviving Release keeps the target's Artist billing, and the target
    // adopts the artwork it lacked.
    let target_artist: String = sqlx::query_scalar(
        "SELECT artist.canonical_name FROM release_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
          WHERE billing.release_id=$1 ORDER BY billing.presentation_order LIMIT 1",
    )
    .bind(target_internal)
    .fetch_one(&db.pool)
    .await
    .expect("read surviving Release Artist");
    assert_eq!(target_artist, "Target Artist");
    let image_owner: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read adopted cover owner");
    assert_eq!(image_owner, target_internal);

    // Bounded validation errors stay explicit.
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/batch",
        Some(serde_json::json!({
            "sourceReleaseIds": [first_public, first_public],
            "targetReleaseId": second_public,
            "reason": "duplicate ids",
        })),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("duplicate_source", error["code"]);
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/batch",
        Some(serde_json::json!({
            "sourceReleaseIds": [target_public],
            "targetReleaseId": target_public,
            "reason": "self merge",
        })),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("self_merge", error["code"]);
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/batch",
        Some(serde_json::json!({
            "sourceReleaseIds": [second_public],
            "targetReleaseId": target_public,
            "reason": "  ",
        })),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!("invalid_reason", error["code"]);
    let (status, error) = call(
        app.clone(),
        "POST",
        "/api/admin/v1/catalog/releases/merge/batch",
        Some(serde_json::json!({
            "sourceReleaseIds": [second_public],
            "targetReleaseId": target_public,
            "reason": "already merged source",
        })),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("release_not_mergeable", error["code"]);

    // Splitting one recorded event restores that source and its Tracks.
    let (status, split) = call(
        app,
        "POST",
        &format!("/api/admin/v1/catalog/releases/{first_public}/split"),
        Some(serde_json::json!({
            "mergeEventId": first_event,
            "reason": "separate edition after review",
        })),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{split}");
    let restored_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(first_internal)
        .fetch_one(&db.pool)
        .await
        .expect("read restored status");
    assert_eq!(restored_status, "active");
    let restored_owner: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM catalog_entity_images WHERE entity_type='release'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read cover owner after split");
    assert_eq!(restored_owner, first_internal);

    db.drop().await;
}
