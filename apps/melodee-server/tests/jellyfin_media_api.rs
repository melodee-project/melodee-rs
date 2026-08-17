//! Jellyfin media-surface compatibility evidence (Phase 8): browse/items,
//! genres, user views, similar/instant-mix, favorites/played state, playlists,
//! images (including Phase 10 catalog artwork overrides), sessions, and audio streaming — against PostgreSQL
//! fixtures seeded through the public Phase 3–5 catalog handoff, with
//! frozen-golden shape comparisons for the captured wire shapes.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{io::Cursor, sync::Arc};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, Request, StatusCode, header},
};
use catalog_common::CatalogFixturePresentation;
use http_body_util::BodyExt;
use image::{ImageFormat, ImageReader, Rgba, RgbaImage};
use melodee_auth::PasswordHashAlgorithm;
use melodee_domain::ReleaseId;
use melodee_persistence::{
    AuthStore, CatalogImageAudit, CatalogImageEntity, CatalogImageStore, NativeUserImport,
};
use melodee_server::jellyfin::{JfLibrary, JfState, SecretPepper};
use melodee_server::native_stream::{CanonicalAssetRoot, ClientAddressResolver, DirectPlayService};
use tempfile::TempDir;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "Mel0deeR0cks!";
const PEPPER: &[u8] = b"melodee-jellyfin-fixture-pepper-NOT-A-REAL-SECRET-32b!";
const USERNAME: &str = "jf-media-fixture";

struct Fixture {
    state: JfState,
    token: String,
    user_public_id: Uuid,
}

async fn import_user(db: &common::TestDb, username: &str, roles: &[&str]) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let public_api_id = Uuid::now_v7();
    let owned: Vec<String> = roles.iter().map(|role| role.to_string()).collect();
    AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id,
                username,
                normalized_username: &username.to_uppercase(),
                email: &format!("{username}@example.test"),
                normalized_email: &format!("{}@EXAMPLE.TEST", username.to_uppercase()),
                password_hash: &bcrypt::hash(FIXTURE_PASSWORD, 4).expect("hash"),
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "test-stream-signing-key",
                is_admin: roles.contains(&"Administrator"),
                is_editor: false,
                enabled: true,
                locked: false,
                blacklisted: false,
                preferred_language: Some("en-US"),
                preferred_theme: Some("dark"),
                auth_revision: 1,
                roles: &owned,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .expect("import user");
    public_api_id
}

fn base_state(db: &common::TestDb) -> JfState {
    JfState {
        pool: db.pool.clone(),
        server_name: "Melodee".to_string(),
        server_version: "0.0.0-test".to_string(),
        server_id: "93cd3cc3a120127ccfda124620290cae".to_string(),
        local_address: "http://melodee.test".to_string(),
        public_origin: "http://melodee.test".to_string(),
        token_pepper: SecretPepper::new(PEPPER.to_vec()).expect("valid fixture pepper"),
        token_expiry_hours: 168,
        max_active_tokens: 10,
        direct_play: None,
        transcoding: None,
        client_addresses: ClientAddressResolver::default(),
        search: None,
        libraries: Vec::new(),
    }
}

async fn respond(state: &JfState, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = melodee_server::jellyfin::configured_router(state.clone())
        .oneshot(request)
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

fn json_body(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("json")
}

fn get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("X-Emby-Token", token)
        .body(Body::empty())
        .expect("request")
}

fn header_str(headers: &HeaderMap, name: HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn catalog_artwork(format: ImageFormat, color: [u8; 4]) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(512, 512, Rgba(color)));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, format)
        .expect("encode catalog artwork");
    encoded.into_inner()
}

fn dimensions(bytes: &[u8]) -> (u32, u32) {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .expect("detect image")
        .into_dimensions()
        .expect("image dimensions")
}

fn post_json(path: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("X-Emby-Token", token)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn delete(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("X-Emby-Token", token)
        .body(Body::empty())
        .expect("request")
}

async fn login(state: &JfState, username: &str) -> String {
    let body = format!(r#"{{"Username":"{username}","Pw":"{FIXTURE_PASSWORD}"}}"#);
    let request = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("request");
    let (status, _, bytes) = respond(state, request).await;
    assert_eq!(status, StatusCode::OK);
    json_body(&bytes)["AccessToken"]
        .as_str()
        .expect("access token")
        .to_string()
}

/// A seeded single-track cataloged release plus an authenticated user with
/// the full stream/download role set.
async fn seeded_fixture(name: &str) -> (common::TestDb, Fixture, CatalogIds) {
    let db = common::TestDb::create(name).await;
    let user_public_id = import_user(
        &db,
        USERNAME,
        &["Administrator", "HasStreamRole", "HasDownloadRole"],
    )
    .await;
    let seeded = phase5_common::seed_cataloged_release_with_genre(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Frozen Golden Release",
            release_artist: "Aurora Fields",
            track_title: "Polar Veil",
            track_artist: "Aurora Fields",
            release_date: "2001-02-03",
        },
        "Post-Rock",
    )
    .await;
    let ids = catalog_ids(&db, seeded.release_id(), "Aurora Fields").await;
    let state = base_state(&db);
    let token = login(&state, USERNAME).await;
    (
        db,
        Fixture {
            state,
            token,
            user_public_id,
        },
        ids,
    )
}

struct CatalogIds {
    artist: Uuid,
    release: Uuid,
    track: Uuid,
}

fn nf(id: Uuid) -> String {
    id.simple().to_string()
}

async fn catalog_ids(db: &common::TestDb, release_id: ReleaseId, artist_name: &str) -> CatalogIds {
    // The fixture resolves release and track billing strings into separate
    // Artist identities (a string is never an Artist identity); the
    // release-primary artist is the addressable one for billing-scoped tests.
    let artist: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM release_artists billing \
         JOIN artists artist ON artist.id = billing.artist_id \
         WHERE billing.release_id = $1 AND billing.role = 'primary' \
           AND artist.canonical_name = $2",
    )
    .bind(release_id.into_uuid())
    .bind(artist_name)
    .fetch_one(&db.pool)
    .await
    .expect("artist id");
    let release: Uuid = sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id = $1")
        .bind(release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("release id");
    let track: Uuid = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
        .bind(release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("track id");
    CatalogIds {
        artist,
        release,
        track,
    }
}

fn golden(value: &str) -> serde_json::Value {
    serde_json::from_str(value).expect("frozen legacy golden JSON")
}

/// Recursive wire-shape comparison: same property sets and JSON types; array
/// comparison samples the first element (mirrors native_catalog_api.rs).
fn assert_json_contract_shape(actual: &serde_json::Value, golden: &serde_json::Value) {
    match (actual, golden) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(golden)) => {
            let actual_keys = actual.keys().collect::<std::collections::BTreeSet<_>>();
            let golden_keys = golden.keys().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual_keys, golden_keys, "wire property set changed");
            for (key, golden_value) in golden {
                assert_json_contract_shape(&actual[key], golden_value);
            }
        }
        (serde_json::Value::Array(actual), serde_json::Value::Array(golden)) => {
            if let (Some(actual), Some(golden)) = (actual.first(), golden.first()) {
                assert_json_contract_shape(actual, golden);
            }
        }
        (serde_json::Value::String(_), serde_json::Value::String(_))
        | (serde_json::Value::Number(_), serde_json::Value::Number(_))
        | (serde_json::Value::Bool(_), serde_json::Value::Bool(_))
        | (serde_json::Value::Null, serde_json::Value::Null) => {}
        (actual, golden) => panic!("wire JSON type/nullability changed: {actual:?} vs {golden:?}"),
    }
}

// ---------------------------------------------------------------------------
// Browse: Artists / Items / Filters / Genres / UserViews
// ---------------------------------------------------------------------------

#[tokio::test]
async fn artists_list_matches_frozen_shape_and_etags() {
    let (_db, fixture, _ids) = seeded_fixture("jf_media_artists").await;
    let (status, headers, bytes) = respond(&fixture.state, get("/Artists", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(header::ETAG));
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/browse/jf-artists-genres-artists-list.json"
        )),
    );
    // The fixture yields two Artist identities (release and track billing).
    assert_eq!(json["TotalRecordCount"], 2);
    assert_eq!(json["Items"][0]["Name"], "Aurora Fields");
    assert_eq!(json["Items"][0]["Type"], "MusicArtist");

    // If-None-Match round trip -> 304 with the same ETag.
    let etag = headers
        .get(header::ETAG)
        .expect("etag")
        .to_str()
        .expect("etag str")
        .to_string();
    let request = Request::builder()
        .method("GET")
        .uri("/Artists")
        .header("X-Emby-Token", &fixture.token)
        .header(header::IF_NONE_MATCH, &etag)
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = respond(&fixture.state, request).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(
        headers.get(header::ETAG).and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );

    // The AlbumArtists alias serves the same shape.
    let (status, _, bytes) =
        respond(&fixture.state, get("/Artists/AlbumArtists", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 2);
}

#[tokio::test]
async fn artist_detail_and_search_term_folding() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_artist_detail").await;
    let (status, headers, bytes) = respond(
        &fixture.state,
        get(&format!("/Artists/{}", nf(ids.artist)), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(header::ETAG));
    let json = json_body(&bytes);
    assert_eq!(json["Id"], nf(ids.artist));
    assert_eq!(json["Name"], "Aurora Fields");
    assert_eq!(json["ChildCount"], 1);

    // Unknown artist -> captured 404 detail.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Artists/{}", nf(Uuid::now_v7())), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Artist not found.");

    // Invalid id -> captured 400.
    let (status, _, bytes) =
        respond(&fixture.state, get("/Artists/not-a-guid", &fixture.token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Invalid artist ID format.");

    // Normalized CONTAINS search.
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Artists?searchTerm=aurora", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 2);
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Artists?searchTerm=nomatch", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 0);
}

#[tokio::test]
async fn items_dispatch_include_item_types_and_parent_probe() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_items").await;

    // MusicArtist: literal CanDownload=true (captured Items-surface quirk).
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Items?includeItemTypes=MusicArtist", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    // Two Artist identities: release billing and track billing are separate.
    assert_eq!(json["TotalRecordCount"], 2);
    assert_eq!(json["Items"][0]["Type"], "MusicArtist");
    assert_eq!(json["Items"][0]["CanDownload"], true);

    // MusicAlbum.
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Items?includeItemTypes=MusicAlbum", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/browse/jf-browse-items-albums.json"
        )),
    );
    assert_eq!(json["Items"][0]["Id"], nf(ids.release));
    assert_eq!(json["Items"][0]["Type"], "MusicAlbum");

    // Audio.
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Items?includeItemTypes=Audio", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Id"], nf(ids.track));
    assert_eq!(json["Items"][0]["Type"], "Audio");

    // Non-music types are silently dropped (empty result).
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Items?includeItemTypes=Movie", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 0);

    // parentId = artist -> albums; parentId = album -> songs.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items?parentId={}", nf(ids.artist)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Items"][0]["Type"], "MusicAlbum");
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items?parentId={}", nf(ids.release)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["Items"][0]["Type"], "Audio");
    assert_eq!(json["Items"][0]["AlbumId"], nf(ids.release));

    // Unknown parentId -> captured empty envelope (no 404).
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items?parentId={}", nf(Uuid::now_v7())),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 0);
}

#[tokio::test]
async fn item_detail_probes_artist_album_song() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_item_detail").await;

    // Song detail matches the frozen shape; no ETag header (captured).
    let (status, headers, bytes) = respond(
        &fixture.state,
        get(&format!("/Items/{}", nf(ids.track)), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::ETAG));
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/browse/jf-browse-item-song-detail.json"
        )),
    );
    assert_eq!(json["Id"], nf(ids.track));
    assert_eq!(json["Name"], "Polar Veil");

    for (id, expected_type) in [(ids.artist, "MusicArtist"), (ids.release, "MusicAlbum")] {
        let (status, _, bytes) = respond(
            &fixture.state,
            get(&format!("/Items/{}", nf(id)), &fixture.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json_body(&bytes)["Type"], expected_type);
    }

    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Items/{}", nf(Uuid::now_v7())), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Item not found.");

    let (status, _, bytes) = respond(&fixture.state, get("/Items/junk", &fixture.token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Invalid item ID format.");
}

#[tokio::test]
async fn filters_genres_and_user_views() {
    let (db, fixture, _ids) = seeded_fixture("jf_media_filters").await;

    let (status, _, bytes) = respond(&fixture.state, get("/Items/Filters", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/browse/jf-browse-items-filters.json"
        )),
    );
    assert_eq!(json["genres"], serde_json::json!(["Post-Rock"]));
    assert_eq!(json["years"], serde_json::json!([2001]));

    // Genres/MusicGenres share the shape with the volatile now-based ETag.
    // Captured: both genre routes emit Type=MusicGenre items and differ only
    // in the volatile ETag prefix.
    for (path, item_type) in [("/Genres", "MusicGenre"), ("/MusicGenres", "MusicGenre")] {
        let (status, headers, bytes) = respond(&fixture.state, get(path, &fixture.token)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(headers.contains_key(header::ETAG));
        let json = json_body(&bytes);
        assert_eq!(json["TotalRecordCount"], 1);
        assert_eq!(json["Items"][0]["Name"], "Post-Rock");
        assert_eq!(json["Items"][0]["Type"], item_type);
    }

    // Empty libraries -> the captured synthetic "Music" UserView.
    let (status, _, bytes) = respond(&fixture.state, get("/UserViews", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Name"], "Music");
    assert_eq!(json["Items"][0]["CollectionType"], "music");
    assert_eq!(json["Items"][0]["Type"], "UserView");

    // /Users/{id}/Views serves the same envelope (captured alias).
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Users/{}/Views", nf(fixture.user_public_id)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Items"][0]["Name"], "Music");

    // Configured libraries replace the synthetic view.
    let mut state = base_state(&db);
    let library_id = Uuid::now_v7();
    state.libraries = vec![Arc::new(JfLibrary {
        id: library_id,
        name: "Storage".to_string(),
        created_at: OffsetDateTime::UNIX_EPOCH,
    })];
    let token = login(&state, USERNAME).await;
    let (status, _, bytes) = respond(&state, get("/UserViews", &token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Name"], "Storage");
    assert_eq!(json["Items"][0]["Id"], nf(library_id));
    // The captured Storage view has the Unix-epoch DateCreated quirk.
    assert_eq!(
        json["Items"][0]["DateCreated"],
        "1970-01-01T00:00:00.0000000Z"
    );
}

#[tokio::test]
async fn similar_and_instant_mix_are_deterministic() {
    let db = common::TestDb::create("jf_media_similar").await;
    import_user(
        &db,
        USERNAME,
        &["Administrator", "HasStreamRole", "HasDownloadRole"],
    )
    .await;
    // Two artists sharing the "Post-Rock" genre.
    let first = phase5_common::seed_cataloged_release_with_genre(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "First Release",
            release_artist: "Aurora Fields",
            track_title: "Polar Veil",
            track_artist: "Aurora Fields",
            release_date: "2001-02-03",
        },
        "Post-Rock",
    )
    .await;
    let second = phase5_common::seed_cataloged_release_with_genre(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Second Release",
            release_artist: "Binary Suns",
            track_title: "Twin Horizon",
            track_artist: "Binary Suns",
            release_date: "2003-04-05",
        },
        "Post-Rock",
    )
    .await;
    let first_ids = catalog_ids(&db, first.release_id(), "Aurora Fields").await;
    let second_ids = catalog_ids(&db, second.release_id(), "Binary Suns").await;
    let state = base_state(&db);
    let token = login(&state, USERNAME).await;

    // Artist similar: genre overlap surfaces the other artist, deterministic.
    let path = format!("/Artists/{}/Similar", nf(first_ids.artist));
    let (status, _, bytes) = respond(&state, get(&path, &token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Id"], nf(second_ids.artist));
    let (_, _, repeat) = respond(&state, get(&path, &token)).await;
    assert_eq!(bytes, repeat, "similar artists must be deterministic");

    // Unknown artist seed -> 404 "Artist not found."
    let (status, _, bytes) = respond(
        &state,
        get(&format!("/Artists/{}/Similar", nf(Uuid::now_v7())), &token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Artist not found.");

    // Item similar (song seed) -> the other song, deterministic.
    let path = format!("/Items/{}/Similar", nf(first_ids.track));
    let (status, _, bytes) = respond(&state, get(&path, &token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Id"], nf(second_ids.track));

    // Captured quirk: artist ids return the empty envelope from /Items/Similar.
    let (status, _, bytes) = respond(
        &state,
        get(&format!("/Items/{}/Similar", nf(first_ids.artist)), &token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 0);

    // InstantMix: deterministic order and the captured 404 classes.
    let path = format!("/Items/{}/InstantMix", nf(first_ids.track));
    let (status, _, bytes) = respond(&state, get(&path, &token)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, repeat) = respond(&state, get(&path, &token)).await;
    assert_eq!(bytes, repeat, "instant mix must be deterministic");
    let json = json_body(&bytes);
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Id"], nf(second_ids.track));

    let (status, _, bytes) = respond(
        &state,
        get(&format!("/Items/{}/InstantMix", nf(Uuid::now_v7())), &token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Item not found.");
    let (status, _, bytes) = respond(
        &state,
        get(&format!("/Songs/{}/InstantMix", nf(Uuid::now_v7())), &token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Song not found.");

    // Refresh is the captured 204 no-op.
    let (status, _, _) = respond(
        &state,
        post_json(
            &format!("/Items/{}/Refresh", nf(first_ids.track)),
            &token,
            "{}",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn user_items_redirect_pair() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_redirect").await;
    let (status, headers, _) = respond(
        &fixture.state,
        get(
            &format!(
                "/Users/{}/Items?includeItemTypes=Audio&recursive=true&limit=10",
                nf(fixture.user_public_id)
            ),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    let location = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("location");
    // Captured reconstruction: whitelisted params, booleans re-serialized as
    // True/False, userId appended.
    assert_eq!(
        location,
        format!(
            "/Items?includeItemTypes=Audio&recursive=True&limit=10&userId={}",
            nf(fixture.user_public_id)
        )
    );

    let (status, headers, _) = respond(
        &fixture.state,
        get(
            &format!(
                "/Users/{}/Items/{}",
                nf(fixture.user_public_id),
                nf(ids.track)
            ),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    let location = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("location");
    // Captured: the item GUID re-serializes in dashed form.
    assert_eq!(location, format!("/Items/{}", ids.track));

    // Invalid item id -> the captured non-ProblemDetails 400 shape.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Users/{}/Items/junk", nf(fixture.user_public_id)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["error"], "Invalid item ID format");
}

// ---------------------------------------------------------------------------
// User state: favorites and played
// ---------------------------------------------------------------------------

#[tokio::test]
async fn favorites_and_played_round_trip() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_userstate").await;
    let user = nf(fixture.user_public_id);
    let track = nf(ids.track);

    // Favorite add -> partial UserData echo.
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Users/{user}/FavoriteItems/{track}"),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["IsFavorite"], true);
    assert_eq!(json["Key"], track);

    // Favorite remove echoes IsFavorite=false.
    let (status, _, bytes) = respond(
        &fixture.state,
        delete(
            &format!("/Users/{user}/FavoriteItems/{track}"),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["IsFavorite"], false);

    // Mark played twice -> cumulative count echoes.
    for expected in [1, 2] {
        let (status, _, bytes) = respond(
            &fixture.state,
            post_json(
                &format!("/Users/{user}/PlayedItems/{track}"),
                &fixture.token,
                "",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let json = json_body(&bytes);
        assert_eq!(json["Played"], true);
        assert_eq!(json["PlayCount"], expected);
        assert!(json["LastPlayedDate"].is_string());
    }

    // Mark unplayed resets.
    let (status, _, bytes) = respond(
        &fixture.state,
        delete(
            &format!("/Users/{user}/PlayedItems/{track}"),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_eq!(json["Played"], false);
    assert_eq!(json["PlayCount"], 0);

    // Other-user paths -> the captured 403 details.
    let other = nf(Uuid::now_v7());
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Users/{other}/FavoriteItems/{track}"),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Cannot modify favorites for other users."
    );
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Users/{other}/PlayedItems/{track}"),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Cannot modify played status for other users."
    );

    // Artists are not playable -> captured 404.
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Users/{user}/PlayedItems/{}", nf(ids.artist)),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Item not found.");

    // Unknown item favorite -> 404.
    let (status, _, _) = respond(
        &fixture.state,
        post_json(
            &format!("/Users/{user}/FavoriteItems/{}", nf(Uuid::now_v7())),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playlist_full_lifecycle_and_d4_clean_add() {
    let (db, fixture, ids) = seeded_fixture("jf_media_playlists").await;
    // A second track for add/move/remove coverage.
    let second = phase5_common::seed_cataloged_release_with_genre(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Second Release",
            release_artist: "Binary Suns",
            track_title: "Twin Horizon",
            track_artist: "Binary Suns",
            release_date: "2003-04-05",
        },
        "Post-Rock",
    )
    .await;
    let second_ids = catalog_ids(&db, second.release_id(), "Binary Suns").await;

    // Create with one song.
    let body = format!(r#"{{"Name":"Mix One","Ids":["{}"]}}"#, nf(ids.track));
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json("/Playlists", &fixture.token, &body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let playlist_id = json_body(&bytes)["Id"].as_str().expect("id").to_string();
    // The legacy fixture playlist carried a comment (Overview is a string in
    // the golden); set one so the shape comparison is nullability-stable.
    sqlx::query("UPDATE native_playlists SET description = 'Synthetic public playlist spanning multiple Releases.' WHERE public_api_id = $1")
        .bind(Uuid::parse_str(&playlist_id).expect("playlist uuid"))
        .execute(&db.pool)
        .await
        .expect("set playlist description");

    // List matches the frozen shape.
    let (status, _, bytes) = respond(&fixture.state, get("/Playlists", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/playlists/jf-playlists-playlists-list.json"
        )),
    );
    assert_eq!(json["TotalRecordCount"], 1);
    assert_eq!(json["Items"][0]["Id"], playlist_id);
    assert_eq!(json["Items"][0]["Name"], "Mix One");

    // Detail carries an ETag; If-None-Match -> 304.
    let (status, headers, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers
        .get(header::ETAG)
        .expect("etag")
        .to_str()
        .expect("etag str")
        .to_string();
    assert_json_contract_shape(
        &json_body(&bytes),
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/playlists/jf-playlists-playlist-detail.json"
        )),
    );
    assert_eq!(json_body(&bytes)["ChildCount"], 1);
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Playlists/{playlist_id}"))
        .header("X-Emby-Token", &fixture.token)
        .header(header::IF_NONE_MATCH, &etag)
        .body(Body::empty())
        .expect("request");
    let (status, _, _) = respond(&fixture.state, request).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    // Update: rename + visibility flip; blank names are ignored (captured).
    let (status, _, _) = respond(
        &fixture.state,
        post_json(
            &format!("/Playlists/{playlist_id}"),
            &fixture.token,
            r#"{"Name":"Mix One Renamed","IsPublic":true}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["Name"], "Mix One Renamed");
    let (status, _, _bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Playlists/{playlist_id}"),
            &fixture.token,
            r#"{"Name":"   "}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}"), &fixture.token),
    )
    .await;
    assert_eq!(json_body(&bytes)["Name"], "Mix One Renamed");

    // Unknown playlist update/delete -> the captured 404 detail.
    let unknown = nf(Uuid::now_v7());
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json(
            &format!("/Playlists/{unknown}"),
            &fixture.token,
            r#"{"Name":"Nope"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Playlist not found or access denied."
    );

    // Items: 1-based ParentIndexNumber; UserData hydrated only with
    // enableUserData=true (captured).
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}/Items"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert_json_contract_shape(
        &json,
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/playlists/jf-playlists-playlist-items.json"
        )),
    );
    assert_eq!(json["Items"][0]["ParentIndexNumber"], 1);
    assert!(json["Items"][0]["UserData"].is_null());
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Playlists/{playlist_id}/Items?enableUserData=true"),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(json_body(&bytes)["Items"][0]["UserData"].is_object());

    // D4 deviation: add to a non-empty playlist is a clean 204 (the legacy
    // 500 corruption is not preserved).
    let (status, _, _) = respond(
        &fixture.state,
        post_json(
            &format!(
                "/Playlists/{playlist_id}/Items?ids={}",
                nf(second_ids.track)
            ),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}/Items"), &fixture.token),
    )
    .await;
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 2);

    // Move the second entry to index 0.
    let (status, _, _) = respond(
        &fixture.state,
        post_json(
            &format!(
                "/Playlists/{playlist_id}/Items/{}/Move/0",
                nf(second_ids.track)
            ),
            &fixture.token,
            "",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}/Items"), &fixture.token),
    )
    .await;
    assert_eq!(json_body(&bytes)["Items"][0]["Id"], nf(second_ids.track));

    // Remove it again.
    let (status, _, _) = respond(
        &fixture.state,
        delete(
            &format!(
                "/Playlists/{playlist_id}/Items?entryIds={}",
                nf(second_ids.track)
            ),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}/Items"), &fixture.token),
    )
    .await;
    assert_eq!(json_body(&bytes)["TotalRecordCount"], 1);

    // DELETE /Items/{id} deletes owned playlists (captured delete surface).
    let (status, _, _) = respond(
        &fixture.state,
        delete(&format!("/Items/{playlist_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{playlist_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Playlist not found.");

    // DELETE /Items/{id} on a non-playlist id -> captured 404 detail.
    let (status, _, bytes) = respond(
        &fixture.state,
        delete(&format!("/Items/{}", nf(ids.track)), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Item not found or cannot be deleted."
    );

    // Blank name -> captured 400.
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json("/Playlists", &fixture.token, r#"{"Name":"  "}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Playlist name is required.");

    // DELETE /Playlists/{id} (the playlist-surface delete route).
    let (status, _, bytes) = respond(
        &fixture.state,
        post_json("/Playlists", &fixture.token, r#"{"Name":"Delete Me"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let delete_id = json_body(&bytes)["Id"].as_str().expect("id").to_string();
    let (status, _, _) = respond(
        &fixture.state,
        delete(&format!("/Playlists/{delete_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Playlists/{delete_id}"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Playlist not found.");
    // Deleting an unknown playlist -> captured 404 detail.
    let (status, _, bytes) = respond(
        &fixture.state,
        delete(
            &format!("/Playlists/{}", nf(Uuid::now_v7())),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Playlist not found or access denied."
    );
}

// ---------------------------------------------------------------------------
// Images (captured no-artwork 404 classes and Phase 10 override precedence)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn images_no_artwork_404_classes() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_images").await;

    // Unknown id -> "Item not found." (captured shape vs frozen golden).
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items/{}/Images/Primary", nf(Uuid::now_v7())),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_json_contract_shape(
        &json_body(&bytes),
        &golden(include_str!(
            "../../../compatibility/jellyfin/golden/images/jf-images-image-unknown.json"
        )),
    );
    assert_eq!(json_body(&bytes)["detail"], "Item not found.");

    // The captured 404 classes remain intact when neither an operator override
    // nor immutable publication-plan artwork exists.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items/{}/Images/Primary", nf(ids.artist)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Artist image not found.");
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items/{}/Images/Primary", nf(ids.release)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Album image not found.");
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Items/{}/Images/Primary?maxWidth=100", nf(ids.track)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Song album image not found.");

    // /Artists/{id}/Images/Primary mirrors the artist class.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Artists/{}/Images/Primary", nf(ids.artist)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Artist image not found.");

    // Invalid id -> captured 400.
    let (status, _, bytes) = respond(
        &fixture.state,
        get("/Items/junk/Images/Primary", &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Invalid item ID format.");
}

#[tokio::test]
async fn images_serve_artist_and_release_overrides_on_all_catalog_routes() {
    let (db, fixture, ids) = seeded_fixture("jf_media_images_overrides").await;
    let artist_id: Uuid = sqlx::query_scalar("SELECT id FROM artists WHERE public_api_id = $1")
        .bind(ids.artist)
        .fetch_one(&db.pool)
        .await
        .expect("artist internal id");
    let release_id: Uuid = sqlx::query_scalar("SELECT id FROM releases WHERE public_api_id = $1")
        .bind(ids.release)
        .fetch_one(&db.pool)
        .await
        .expect("release internal id");
    let artist_artwork = catalog_artwork(ImageFormat::Png, [10, 20, 30, 255]);
    let release_artwork = catalog_artwork(ImageFormat::Jpeg, [60, 70, 80, 255]);
    let audit = CatalogImageAudit {
        actor: "jf-media-test",
        reason: "verify adapter artwork precedence",
        now: OffsetDateTime::now_utc(),
    };
    CatalogImageStore::set_image(
        &db.pool,
        CatalogImageEntity::Artist,
        artist_id,
        "image/png",
        &artist_artwork,
        audit,
    )
    .await
    .expect("store artist override");
    CatalogImageStore::set_image(
        &db.pool,
        CatalogImageEntity::Release,
        release_id,
        "image/jpeg",
        &release_artwork,
        audit,
    )
    .await
    .expect("store release override");

    // Artist artwork is served from both Jellyfin image-route forms.
    let artist_path = format!("/Items/{}/Images/Primary?maxWidth=100", nf(ids.artist));
    let (status, headers, bytes) = respond(&fixture.state, get(&artist_path, &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/png");
    assert_eq!(
        header_str(&headers, header::CACHE_CONTROL),
        "public, max-age=86400"
    );
    assert_eq!(dimensions(&bytes), (100, 100), "small bucket is bounded");
    let etag = header_str(&headers, header::ETAG);
    let body_length = bytes.len().to_string();
    let head = Request::builder()
        .method("HEAD")
        .uri(&artist_path)
        .header("X-Emby-Token", &fixture.token)
        .body(Body::empty())
        .expect("HEAD request");
    let (status, headers, bytes) = respond(&fixture.state, head).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_str(&headers, header::CONTENT_LENGTH), body_length);
    assert!(bytes.is_empty());
    let conditional = Request::builder()
        .method("GET")
        .uri(&artist_path)
        .header("X-Emby-Token", &fixture.token)
        .header(header::IF_NONE_MATCH, &etag)
        .body(Body::empty())
        .expect("conditional request");
    let (status, headers, bytes) = respond(&fixture.state, conditional).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(header_str(&headers, header::ETAG), etag);
    assert!(bytes.is_empty());
    let (status, headers, bytes) = respond(
        &fixture.state,
        get(
            &format!("/Artists/{}/Images/Primary", nf(ids.artist)),
            &fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/png");
    assert_eq!(dimensions(&bytes), (1024, 1024));

    // A Track deliberately inherits its Release override.
    for path in [
        format!("/Items/{}/Images/Primary", nf(ids.release)),
        format!("/Items/{}/Images/Primary/0", nf(ids.release)),
        format!("/Items/{}/Images/Primary", nf(ids.track)),
    ] {
        let (status, headers, bytes) = respond(&fixture.state, get(&path, &fixture.token)).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/jpeg");
        assert_eq!(dimensions(&bytes), (1024, 1024), "{path}");
    }
}

#[tokio::test]
async fn images_fall_back_to_published_canonical_artwork() {
    let (db, mut fixture, ids) = seeded_fixture("jf_media_images_canonical_artwork").await;
    let (work_item_id, target_relative_path): (Uuid, String) = sqlx::query_as(
        "SELECT plan.work_item_id, plan.target_relative_path
         FROM release_publication_plans plan
         JOIN catalog_imports catalog_import ON catalog_import.work_item_id = plan.work_item_id
         JOIN releases release ON release.id = catalog_import.release_id
         WHERE release.public_api_id = $1",
    )
    .bind(ids.release)
    .fetch_one(&db.pool)
    .await
    .expect("published plan");
    let artwork = catalog_artwork(ImageFormat::Jpeg, [100, 110, 120, 255]);
    let canonical_hash = "a".repeat(64);
    let canonical_path = format!("artwork/{canonical_hash}.jpg");

    // The phase-5 fixture intentionally publishes media-only plans. Add a
    // pre-existing immutable artwork asset to its isolated plan so this test
    // exercises the real plan query and no-follow storage read, not a mock.
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
    .bind(work_item_id)
    .bind(&canonical_path)
    .bind(i64::try_from(artwork.len()).expect("artwork size"))
    .bind(&canonical_hash)
    .execute(&db.pool)
    .await
    .expect("add canonical artwork fixture");
    let root = tempfile::tempdir().expect("canonical root");
    let artwork_path = root
        .path()
        .join(&target_relative_path)
        .join(&canonical_path);
    std::fs::create_dir_all(artwork_path.parent().expect("artwork parent")).expect("mkdir");
    std::fs::write(&artwork_path, &artwork).expect("write canonical artwork");
    fixture.state.direct_play = Some(
        DirectPlayService::new(
            CanonicalAssetRoot::open(root.path()).expect("open canonical root"),
            4,
        )
        .expect("direct play"),
    );

    // There is no override: the Artist, Release, and Track all reach the
    // immutable source and obey the requested Jellyfin size bucket.
    for id in [ids.artist, ids.release, ids.track] {
        let (status, headers, bytes) = respond(
            &fixture.state,
            get(
                &format!("/Items/{}/Images/Primary?maxHeight=300", nf(id)),
                &fixture.token,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{id}");
        assert_eq!(header_str(&headers, header::CONTENT_TYPE), "image/jpeg");
        assert_eq!(dimensions(&bytes), (300, 300), "{id}");
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sessions_synthesis_and_playback_reports() {
    let (_db, fixture, ids) = seeded_fixture("jf_media_sessions").await;
    let track = nf(ids.track);

    // Empty baseline -> captured [].
    let (status, _, bytes) = respond(&fixture.state, get("/Sessions", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes), serde_json::json!([]));

    // Playing posts do NOT register a listable session: the frozen baseline
    // lists [] even immediately after Playing/Progress
    // (`jf-userstate/sessions-list`).
    let body = format!(r#"{{"ItemId":"{track}","PlaySessionId":"ps-1"}}"#);
    let (status, _, _) = respond(
        &fixture.state,
        post_json("/Sessions/Playing", &fixture.token, &body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, bytes) = respond(&fixture.state, get("/Sessions", &fixture.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes), serde_json::json!([]));

    // Progress/Ping/Capabilities are 204.
    for (path, body) in [
        ("/Sessions/Playing/Progress", body.as_str()),
        ("/Sessions/Playing/Ping", ""),
        ("/Sessions/Capabilities", ""),
        ("/Sessions/Capabilities/Full", "{}"),
    ] {
        let (status, _, _) = respond(&fixture.state, post_json(path, &fixture.token, body)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{path}");
    }

    // Stopped with Failed=true skips the scrobble but stays 204.
    let body = format!(r#"{{"ItemId":"{track}","PlaySessionId":"ps-failed","Failed":true}}"#);
    let (status, _, _) = respond(
        &fixture.state,
        post_json("/Sessions/Playing/Stopped", &fixture.token, &body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let plays: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays")
        .fetch_one(&_db.pool)
        .await
        .expect("plays");
    assert_eq!(plays, 0, "Failed=true must not scrobble");

    // Stopped records the scrobble and clears now-playing.
    let body = format!(r#"{{"ItemId":"{track}","PlaySessionId":"ps-2"}}"#);
    let (status, _, _) = respond(
        &fixture.state,
        post_json("/Sessions/Playing/Stopped", &fixture.token, &body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let plays: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays")
        .fetch_one(&_db.pool)
        .await
        .expect("plays");
    assert_eq!(plays, 1);

    // Malformed ItemId is a silent 204 (captured).
    let (status, _, _) = respond(
        &fixture.state,
        post_json(
            "/Sessions/Playing",
            &fixture.token,
            r#"{"ItemId":"not-a-guid"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ---------------------------------------------------------------------------
// Audio streaming, lyrics, download, PlaybackInfo
// ---------------------------------------------------------------------------

struct AudioFixture {
    _temp: TempDir,
    fixture: Fixture,
    ids: CatalogIds,
    // Keeps the test database alive for the fixture's lifetime.
    _db: common::TestDb,
}

async fn audio_fixture(name: &str) -> AudioFixture {
    let (db, mut fixture, ids) = seeded_fixture(name).await;
    let temp = tempfile::tempdir().expect("audio root");
    std::fs::create_dir_all(temp.path().join("jf-audio")).expect("mkdir");
    // 4096 deterministic bytes; the playable asset is mp3 by catalog metadata.
    let bytes: Vec<u8> = (0..4096u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(temp.path().join("jf-audio/song.mp3"), &bytes).expect("write asset");
    std::fs::write(
        temp.path().join("jf-audio/song.lrc"),
        "[00:01.50]First line\nplain text line\n[00:02.750]Second line\n",
    )
    .expect("write lyrics");
    sqlx::query(
        "UPDATE media_assets SET canonical_relative_path = 'jf-audio/song.mp3', byte_size = 4096 \
          WHERE id = (SELECT playable_media_asset_id FROM tracks WHERE public_api_id = $1)",
    )
    .bind(ids.track)
    .execute(&db.pool)
    .await
    .expect("repoint media asset");
    let root = CanonicalAssetRoot::open(temp.path()).expect("open root");
    fixture.state.direct_play = Some(DirectPlayService::new(root, 4).expect("direct play"));
    AudioFixture {
        _temp: temp,
        fixture,
        ids,
        _db: db,
    }
}

/// Builds a real PCM WAV body (mono 8-bit 44.1 kHz) for transcode fixtures;
/// the file is stored as `song.mp3` so direct-play content-type derivation
/// is unchanged while FFmpeg probes the real content from stdin.
fn wav_bytes(data_size: usize) -> Vec<u8> {
    let data_size_u32 = u32::try_from(data_size).expect("wav data fits u32");
    let mut bytes = Vec::with_capacity(44 + data_size);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data_size_u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size_u32.to_le_bytes());
    let pattern: Vec<u8> = (0u8..=255).collect();
    for index in 0..data_size {
        bytes.push(pattern[index % 256]);
    }
    bytes
}

/// Fixture variant with a real WAV payload and the shared transcoding
/// service attached (ADR-0147).
async fn audio_transcode_fixture(name: &str) -> AudioFixture {
    let audio = audio_fixture(name).await;
    let wav = wav_bytes(220_500);
    std::fs::write(audio._temp.path().join("jf-audio/song.mp3"), &wav).expect("write wav payload");
    let byte_size = i64::try_from(wav.len()).expect("wav size fits i64");
    sqlx::query(
        "UPDATE media_assets SET byte_size = $2 \
          WHERE id = (SELECT playable_media_asset_id FROM tracks WHERE public_api_id = $1)",
    )
    .bind(audio.ids.track)
    .bind(byte_size)
    .execute(&audio._db.pool)
    .await
    .expect("sync asset byte size");
    let root = CanonicalAssetRoot::open(audio._temp.path()).expect("open root");
    let mut fixture = audio.fixture;
    fixture.state.transcoding = Some(
        melodee_server::transcode::TranscodingService::new(root, fixture.state.pool.clone(), 2)
            .expect("transcoding service"),
    );
    AudioFixture {
        _temp: audio._temp,
        fixture,
        ids: audio.ids,
        _db: audio._db,
    }
}

#[tokio::test]
async fn audio_stream_transcoding_configured() {
    let audio = audio_transcode_fixture("jf_media_audio_transcode").await;
    let track = nf(audio.ids.track);

    // stream.mp3 without static -> configured mp3 transcode (ADR-0147).
    let (status, headers, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream.mp3"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/mpeg")
    );
    assert!(!headers.contains_key(header::CONTENT_LENGTH));
    assert!(!headers.contains_key(header::ACCEPT_RANGES));
    assert!(
        bytes.starts_with(b"ID3")
            || (bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0),
        "mp3 frame sync or ID3 tag"
    );

    // stream.aac -> ADTS output at audio/aac.
    let (status, headers, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream.aac"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/aac")
    );
    assert!(!bytes.is_empty());
    assert_eq!(bytes[0], 0xFF);
    assert_eq!(bytes[1] & 0xF6, 0xF0, "ADTS sync word");

    // stream.opus -> Ogg/Opus output at audio/ogg.
    let (status, headers, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream.opus"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/ogg")
    );
    assert!(bytes.starts_with(b"OggS"), "Ogg container magic");

    // audioBitRate (bits per second) without a container defaults to mp3.
    let (status, headers, _) = respond(
        &audio.fixture.state,
        get(
            &format!("/Audio/{track}/stream?audioBitRate=64000"),
            &audio.fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/mpeg")
    );

    // HEAD on a transcode request answers headers only and spawns no FFmpeg
    // process (ADR-0147 decision 4).
    let request = Request::builder()
        .method("HEAD")
        .uri(format!("/Audio/{track}/stream.mp3"))
        .header("X-Emby-Token", &audio.fixture.token)
        .body(Body::empty())
        .expect("request");
    let (status, headers, bytes) = respond(&audio.fixture.state, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/mpeg")
    );
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn audio_stream_full_range_and_416_nuances() {
    let audio = audio_fixture("jf_media_audio_stream").await;
    let track = nf(audio.ids.track);

    // Full GET -> 200 with captured headers.
    let (status, headers, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes.len(), 4096);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/mpeg")
    );
    assert_eq!(
        headers
            .get(header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok()),
        Some("bytes")
    );

    // HEAD -> headers only.
    let request = Request::builder()
        .method("HEAD")
        .uri(format!("/Audio/{track}/stream"))
        .header("X-Emby-Token", &audio.fixture.token)
        .body(Body::empty())
        .expect("request");
    let (status, headers, bytes) = respond(&audio.fixture.state, request).await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.is_empty());
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some("4096")
    );

    // Range -> 206 with Content-Range.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Audio/{track}/stream"))
        .header("X-Emby-Token", &audio.fixture.token)
        .header(header::RANGE, "bytes=100-199")
        .body(Body::empty())
        .expect("request");
    let (status, headers, bytes) = respond(&audio.fixture.state, request).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes.len(), 100);
    assert_eq!(
        headers
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()),
        Some("bytes 100-199/4096")
    );

    // Out-of-range start -> 416 WITH Content-Range: bytes */len (captured).
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Audio/{track}/stream"))
        .header("X-Emby-Token", &audio.fixture.token)
        .header(header::RANGE, "bytes=99999-100000")
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = respond(&audio.fixture.state, request).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        headers
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()),
        Some("bytes */4096")
    );

    // Unparseable range -> 416 WITHOUT Content-Range (captured nuance).
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Audio/{track}/stream"))
        .header("X-Emby-Token", &audio.fixture.token)
        .header(header::RANGE, "garbage")
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = respond(&audio.fixture.state, request).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert!(!headers.contains_key(header::CONTENT_RANGE));

    // stream.{extension} with static=true -> 200; /universal forces static.
    let (status, _, _) = respond(
        &audio.fixture.state,
        get(
            &format!("/Audio/{track}/stream.mp3?static=true"),
            &audio.fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/universal"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ADR-0147 retires the captured 400 rejection: with transcoding disabled
    // (no service) a transcode request fails closed with 503, and an
    // unsupported container answers 400 before any service lookup.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream.mp3"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(&bytes)["detail"], "Transcoding is not available.");
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/stream.flac"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Unsupported transcoding container."
    );

    // Unknown song -> captured 404 classes.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get(
            &format!("/Audio/{}/stream", nf(Uuid::now_v7())),
            &audio.fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Audio item not found.");
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get("/Audio/junk/stream", &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&bytes)["detail"], "Invalid item ID format.");
}

#[tokio::test]
async fn audio_lyrics_download_and_playback_info() {
    let audio = audio_fixture("jf_media_audio_misc").await;
    let track = nf(audio.ids.track);

    // Lyrics sidecar: captured lowercase keys; two-digit millis right-pad.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Audio/{track}/lyrics"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    let lyrics = json["lyrics"].as_array().expect("lyrics");
    assert_eq!(lyrics.len(), 3);
    assert_eq!(lyrics[0]["text"], "First line");
    assert_eq!(lyrics[0]["start"], 1_500 * 10_000);
    assert_eq!(lyrics[1]["text"], "plain text line");
    assert!(lyrics[1]["start"].is_null());
    assert_eq!(lyrics[2]["start"], 2_750 * 10_000);

    // Download: attachment disposition with the on-disk filename.
    let (status, headers, bytes) = respond(
        &audio.fixture.state,
        get(&format!("/Items/{track}/Download"), &audio.fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes.len(), 4096);
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"song.mp3\"")
    );

    // PlaybackInfo shape.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        post_json(
            &format!("/Items/{track}/PlaybackInfo"),
            &audio.fixture.token,
            "{}",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = json_body(&bytes);
    assert!(json["PlaySessionId"].is_string());
    let source = &json["MediaSources"][0];
    assert_eq!(source["Id"], track);
    assert_eq!(source["Container"], "mp3");
    assert_eq!(source["SupportsDirectPlay"], true);
    assert_eq!(source["SupportsDirectStream"], true);
    assert_eq!(source["SupportsTranscoding"], false);
    assert_eq!(source["Size"], 4096);

    // Captured: PlaybackInfo also answers GET identically.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        get(
            &format!("/Items/{track}/PlaybackInfo"),
            &audio.fixture.token,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes)["MediaSources"][0]["Id"], track);

    // Unknown song PlaybackInfo -> captured 404.
    let (status, _, bytes) = respond(
        &audio.fixture.state,
        post_json(
            &format!("/Items/{}/PlaybackInfo", nf(Uuid::now_v7())),
            &audio.fixture.token,
            "{}",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Item not found.");
}

#[tokio::test]
async fn audio_role_gates_and_missing_storage() {
    let (db, fixture, ids) = seeded_fixture("jf_media_audio_gates").await;
    let track = nf(ids.track);

    // No direct_play configured -> the captured "Audio file not found." 404.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Audio/{track}/stream"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json_body(&bytes)["detail"], "Audio file not found.");

    // Lyrics without direct_play -> the captured empty shape.
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Audio/{track}/lyrics"), &fixture.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&bytes), serde_json::json!({"lyrics": []}));

    // A user without HasStreamRole -> captured 403.
    import_user(&db, "jf-nostream", &[]).await;
    let token = login(&fixture.state, "jf-nostream").await;
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Audio/{track}/stream"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(&bytes)["detail"],
        "Streaming permission required."
    );
    let (status, _, bytes) = respond(
        &fixture.state,
        get(&format!("/Items/{track}/Download"), &token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json_body(&bytes)["detail"], "Download permission required.");

    // Missing token -> indistinguishable 401.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/Audio/{track}/stream"))
        .body(Body::empty())
        .expect("request");
    let (status, _, _) = respond(&fixture.state, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
