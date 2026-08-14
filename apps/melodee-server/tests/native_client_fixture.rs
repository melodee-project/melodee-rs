//! Disposable real-client fixture for the Phase 7 Android/Auto/MeloAmp gate.
//!
//! Run this ignored test, point clients at the printed URL and credentials,
//! and stop the test after the smoke. It owns and drops a disposable database
//! and canonical-media root; it never touches an existing library.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;
#[path = "../../../crates/melodee-persistence/tests/phase5_common/mod.rs"]
mod phase5_common;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use bcrypt::hash;
use catalog_common::CatalogFixturePresentation;
use melodee_auth::PasswordHashAlgorithm;
use melodee_persistence::{AuthStore, NativeUserImport};
use melodee_server::native::stream::DatabaseStreamUrlSigner;
use melodee_server::{
    native::{NativeState, PostgresNativeUserStateReader, podcasts::PodcastDiscoveryClient},
    native_stream::{CanonicalAssetRoot, ClientAddressResolver, DirectPlayService},
    podcast_worker::PodcastConfig,
};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

const FIXTURE_PASSWORD: &str = "phase7-client-password";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FixtureDescriptor {
    base_url: String,
    api_url: String,
    username: &'static str,
    email: &'static str,
    password: &'static str,
    artist_public_id: Uuid,
    release_public_id: Uuid,
    track_public_id: Uuid,
    next_track_public_id: Uuid,
    playlist_public_id: Uuid,
    lifetime_seconds: u64,
}

#[tokio::test]
#[ignore = "interactive external-client acceptance fixture"]
async fn serve_native_phase7_real_client_fixture() {
    let port = std::env::var("MELODEE_PHASE7_FIXTURE_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(5157);
    let lifetime_seconds = std::env::var("MELODEE_PHASE7_FIXTURE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_800)
        .clamp(30, 7_200);
    let public_host = std::env::var("MELODEE_PHASE7_FIXTURE_PUBLIC_HOST")
        .unwrap_or_else(|_| "10.0.2.2".to_string());
    let base_url = format!("http://{public_host}:{port}");

    let db = common::TestDb::create("phase7_real_clients").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7_701,
        CatalogFixturePresentation {
            release_title: "Phase Seven Client Release",
            release_artist: "Phase Seven Client Artist",
            track_title: "Phase Seven Playback Track",
            track_artist: "Phase Seven Client Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let next_fixture = phase5_common::seed_cataloged_release(
        &db,
        7_702,
        CatalogFixturePresentation {
            release_title: "Phase Seven Client Encore",
            release_artist: "Phase Seven Client Artist",
            track_title: "Phase Seven Next Track",
            track_artist: "Phase Seven Client Artist",
            release_date: "2026-08-12",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let password_hash = hash(FIXTURE_PASSWORD, 4).expect("fixture bcrypt hash");
    // The catalog URL signer intentionally fails closed unless this role is
    // present. Keep the real-client fixture aligned with migrated users that
    // are allowed to stream so DTO assembly cannot degrade to a typed 503.
    let roles = vec!["user".to_string(), "HasStreamRole".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "phase7-client",
                normalized_username: "PHASE7-CLIENT",
                email: "phase7-client@example.test",
                normalized_email: "PHASE7-CLIENT@EXAMPLE.TEST",
                password_hash: &password_hash,
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "phase7-client-stream-signing-key",
                is_admin: true,
                is_editor: true,
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
        .expect("fixture user");

    let release_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM releases WHERE id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("fixture Release ID");
    let (track_id, track_public_id, media_asset_id, canonical_relative_path): (
        Uuid,
        Uuid,
        Uuid,
        String,
    ) = sqlx::query_as(
        "SELECT track.id,track.public_api_id,asset.id,asset.canonical_relative_path
         FROM tracks track JOIN media_assets asset ON asset.id=track.playable_media_asset_id
         WHERE track.release_id=$1",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("fixture playable Track");
    let (next_track_id, next_track_public_id, next_media_asset_id, next_canonical_relative_path): (
        Uuid,
        Uuid,
        Uuid,
        String,
    ) = sqlx::query_as(
        "SELECT track.id,track.public_api_id,asset.id,asset.canonical_relative_path
         FROM tracks track JOIN media_assets asset ON asset.id=track.playable_media_asset_id
         WHERE track.release_id=$1",
    )
    .bind(next_fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("fixture next playable Track");
    let artist_public_id: Uuid = sqlx::query_scalar(
        "SELECT artist.public_api_id FROM release_artists billing
         JOIN artists artist ON artist.id=billing.artist_id
         WHERE billing.release_id=$1 AND billing.role='primary'",
    )
    .bind(fixture.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("fixture Artist ID");

    let canonical_root = tempfile::tempdir().expect("fixture canonical root");
    let wave = thirty_second_wave();
    for (current_track_id, current_asset_id, current_relative_path) in [
        (track_id, media_asset_id, canonical_relative_path),
        (
            next_track_id,
            next_media_asset_id,
            next_canonical_relative_path,
        ),
    ] {
        let asset_path = canonical_root.path().join(current_relative_path);
        std::fs::create_dir_all(asset_path.parent().expect("asset parent"))
            .expect("fixture asset directories");
        std::fs::write(&asset_path, &wave).expect("fixture audio file");
        sqlx::query(
            "UPDATE media_assets SET byte_size=$2,mime_type='audio/wav',container='wav',codec='pcm_s16le',
                 sample_rate_hz=8000,channels=1,bitrate_bps=128000,duration_ms=30000 WHERE id=$1",
        )
        .bind(current_asset_id)
        .bind(i64::try_from(wave.len()).expect("wave length"))
        .execute(&db.pool)
        .await
        .expect("fixture MediaAsset metadata");
        sqlx::query("UPDATE tracks SET duration_ms=30000 WHERE id=$1")
            .bind(current_track_id)
            .execute(&db.pool)
            .await
            .expect("fixture Track duration");
    }

    let playlist_public_id = Uuid::now_v7();
    let playlist_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_playlists
           (id,public_api_id,owner_user_id,name,is_public,created_at,updated_at)
         VALUES ($1,$2,$3,'Phase Seven Client Playlist',true,$4,$4)",
    )
    .bind(playlist_id)
    .bind(playlist_public_id)
    .bind(user.id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("fixture playlist");
    sqlx::query(
        "INSERT INTO native_playlist_items
           (id,playlist_id,track_id,position,added_by_user_id,created_at)
         VALUES ($1,$2,$3,0,$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(playlist_id)
    .bind(track_id)
    .bind(user.id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("fixture playlist item");
    sqlx::query(
        "INSERT INTO native_playlist_items
           (id,playlist_id,track_id,position,added_by_user_id,created_at)
         VALUES ($1,$2,$3,1,$4,$5)",
    )
    .bind(Uuid::now_v7())
    .bind(playlist_id)
    .bind(next_track_id)
    .bind(user.id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("fixture next playlist item");

    let root = CanonicalAssetRoot::open(canonical_root.path()).expect("canonical root handle");
    let state = NativeState {
        pool: db.pool.clone(),
        notifications: std::sync::Arc::new(
            melodee_server::notifications::NotificationService::noop(db.pool.clone()),
        ),
        search: None,
        public_origin: base_url.clone(),
        podcast_config: Arc::new(PodcastConfig::default()),
        podcast_discovery: PodcastDiscoveryClient::itunes()
            .expect("bounded podcast discovery client"),
        stream_url_signer: Arc::new(DatabaseStreamUrlSigner::new(
            db.pool.clone(),
            base_url.clone(),
        )),
        user_state: Arc::new(PostgresNativeUserStateReader::new(db.pool.clone())),
        direct_play: DirectPlayService::new(root, 8),
        client_addresses: ClientAddressResolver::default(),
        auth: Arc::new(
            melodee_server::native::auth::NativeAuthService::new(
                "phase7-real-client-jwt-signing-key-at-least-32-chars".to_string(),
                90,
            )
            .expect("fixture auth configuration"),
        ),
    };
    let descriptor = FixtureDescriptor {
        base_url: base_url.clone(),
        api_url: format!("{base_url}/api/v1"),
        username: "phase7-client",
        email: "phase7-client@example.test",
        password: FIXTURE_PASSWORD,
        artist_public_id,
        release_public_id,
        track_public_id,
        next_track_public_id,
        playlist_public_id,
        lifetime_seconds,
    };
    println!(
        "phase7-real-client-fixture={}",
        serde_json::to_string(&descriptor).expect("fixture descriptor")
    );

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("fixture listener");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            melodee_server::native::configured_router(state)
                .layer(
                    melodee_server::native::auth::cors_from_origins(
                        "http://127.0.0.1:18080,http://localhost:18080,http://127.0.0.1:3001,http://localhost:3001",
                    )
                    .expect("fixture CORS origins"),
                )
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_secs(lifetime_seconds)).await;
    server.abort();
    let _ = server.await;
    db.drop().await;
}

fn thirty_second_wave() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 8_000;
    const SAMPLES: u32 = SAMPLE_RATE * 30;
    let data_size = SAMPLES * 2;
    let mut bytes = Vec::with_capacity(usize::try_from(data_size + 44).unwrap());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for index in 0..SAMPLES {
        let phase = (index % 80) as i32;
        let sample = i16::try_from((phase - 40) * 400).unwrap();
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[tokio::test]
async fn native_client_fixture_identity_is_authorized_to_sign_playback_urls() {
    let db = common::TestDb::create("phase7_client_stream_role").await;
    let now = OffsetDateTime::now_utc();
    let roles = vec!["user".to_string(), "HasStreamRole".to_string()];
    let user = AuthStore::new()
        .import_user(
            &db.pool,
            &NativeUserImport {
                id: Uuid::now_v7(),
                public_api_id: Uuid::now_v7(),
                username: "phase7-client",
                normalized_username: "PHASE7-CLIENT",
                email: "phase7-client@example.test",
                normalized_email: "PHASE7-CLIENT@EXAMPLE.TEST",
                password_hash: "$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS",
                password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
                stream_signing_key: "phase7-client-stream-signing-key",
                is_admin: true,
                is_editor: true,
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
        .expect("fixture user");
    let principal = AuthStore::new()
        .stream_principal_by_public_id(&db.pool, user.public_api_id)
        .await
        .expect("stream lookup")
        .expect("stream principal");
    assert!(principal.is_authorized());
    db.drop().await;
}
