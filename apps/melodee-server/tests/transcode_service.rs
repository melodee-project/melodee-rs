//! Shared transcoding-service integration evidence (ADR-0147, R2.3).
//!
//! Exercises the bounded, cancellation-safe FFmpeg execution contract with the
//! real toolchain (ADR-0108): template resolution through layered settings,
//! semaphore saturation and recovery, spawn failure, client-disconnect permit
//! release, and an end-to-end WAV→MP3 transcode whose output ffprobe
//! re-parses. Route-level wire behavior per adapter lives in
//! `opensubsonic_media_api.rs` and `jellyfin_media_api.rs`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "../../../crates/melodee-persistence/tests/common/mod.rs"]
mod common;

use http_body_util::BodyExt;
use melodee_persistence::SettingsStore;
use melodee_server::native_stream::{CanonicalAssetRoot, PlayableAsset};
use melodee_server::transcode::{
    OutputFormat, TranscodeError, TranscodeTemplate, TranscodingService,
};
use sqlx::PgPool;
use tempfile::TempDir;
use time::OffsetDateTime;

/// Builds a real PCM WAV fixture (mono 8-bit 44.1 kHz) under `media/` in a
/// fresh tempdir and returns the root service inputs plus the asset metadata.
fn wav_fixture(data_size: usize) -> (TempDir, PlayableAsset, Vec<u8>) {
    let data_size_u32 = u32::try_from(data_size).expect("wav data fits u32");
    let mut bytes = Vec::with_capacity(44 + data_size);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data_size_u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
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
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("media")).expect("mkdir");
    std::fs::write(temp.path().join("media/track.wav"), &bytes).expect("write wav");
    let asset = PlayableAsset {
        canonical_relative_path: "media/track.wav".to_string(),
        byte_size: bytes.len() as u64,
        mime_type: "audio/wav".to_string(),
    };
    (temp, asset, bytes)
}

/// A lazy pool for tests that never touch the database (`stream()` resolves
/// no settings; only `template()` does).
fn lazy_pool() -> PgPool {
    PgPool::connect_lazy("postgres://127.0.0.1:1/melodee_transcode_unit").expect("lazy pool")
}

fn service(temp: &TempDir, pool: PgPool, max: usize) -> TranscodingService {
    let root = CanonicalAssetRoot::open(temp.path()).expect("canonical asset root");
    TranscodingService::new(root, pool, max).expect("transcoding service")
}

/// The corrected seed mp3 template (ADR-0147 decision 2), resolved from
/// `KNOWN_SETTINGS` defaults when no env/file/DB override exists.
fn seed_mp3_template() -> TranscodeTemplate {
    TranscodeTemplate::parse(
        "{\"format\":\"Mp3\",\"bitrate\":192,\"command\":\"ffmpeg -i %s -ss %t -map 0:a:0 -b:a %bk -v 0 -f mp3 -\"}",
    )
    .expect("seed template parses")
}

#[tokio::test]
async fn semaphore_saturation_rejects_immediately_and_recovers_on_drop() {
    let (temp, asset, _bytes) = wav_fixture(44_100);
    let transcoding = service(&temp, lazy_pool(), 1);
    let template = seed_mp3_template();

    // First stream holds the only permit until its body completes or drops;
    // the body is deliberately never polled here.
    let first = transcoding
        .stream(&asset, OutputFormat::Mp3, &template, 192)
        .await
        .expect("first stream acquires the permit");
    let second = transcoding
        .stream(&asset, OutputFormat::Mp3, &template, 192)
        .await;
    assert!(matches!(second, Err(TranscodeError::CapacityExhausted)));
    drop(first);
    let third = transcoding
        .stream(&asset, OutputFormat::Mp3, &template, 192)
        .await;
    assert!(third.is_ok(), "permit is released when the body drops");
}

#[tokio::test]
async fn client_disconnect_releases_capacity_and_kills_child() {
    let (temp, asset, bytes) = wav_fixture(220_500);
    let transcoding = service(&temp, lazy_pool(), 1);
    // A deliberately slow encode (atempo chain ≈ 0.016x) keeps the FFmpeg
    // child alive for minutes without the kill; the child is killed and
    // reaped via `kill_on_drop` + tokio's orphan reaping when the response
    // body (and with it the Child handle) is dropped on disconnect.
    let slow = TranscodeTemplate::parse(
        "{\"format\":\"Mp3\",\"bitrate\":64,\"command\":\"ffmpeg -i %s -filter:a atempo=0.5,atempo=0.5,atempo=0.5,atempo=0.5,atempo=0.5,atempo=0.5 -b:a %bk -v 0 -f mp3 -\"}",
    )
    .expect("slow template parses");
    let mut body = transcoding
        .stream(&asset, OutputFormat::Mp3, &slow, 64)
        .await
        .expect("first stream starts");
    let first_chunk = body
        .frame()
        .await
        .expect("stream yields a frame")
        .expect("frame reads")
        .into_data()
        .expect("data frame");
    assert!(
        !first_chunk.is_empty(),
        "slow encode streams mp3 bytes before disconnect"
    );
    drop(body); // simulated client disconnect

    // Permit release is the observable resource contract; with capacity 1 a
    // leaked permit would reject immediately. A fresh stream must work.
    let replacement = transcoding
        .stream(&asset, OutputFormat::Mp3, &seed_mp3_template(), 192)
        .await
        .expect("permit reacquired after disconnect");
    let collected = replacement
        .collect()
        .await
        .expect("replacement stream completes")
        .to_bytes();
    assert!(!collected.is_empty());
    assert_ne!(
        collected.as_ref(),
        bytes.as_slice(),
        "transcoded bytes differ from the wav source"
    );
}

#[tokio::test]
async fn spawn_failure_maps_to_spawn_failed() {
    let (temp, asset, _bytes) = wav_fixture(44_100);
    let transcoding = service(&temp, lazy_pool(), 2);
    let missing_tool = TranscodeTemplate::parse(
        "{\"format\":\"Mp3\",\"bitrate\":64,\"command\":\"melodee-no-such-tool-xyz -i %s -\"}",
    )
    .expect("template parses");
    let result = transcoding
        .stream(&asset, OutputFormat::Mp3, &missing_tool, 64)
        .await;
    assert!(matches!(result, Err(TranscodeError::SpawnFailed)));
}

#[tokio::test]
async fn template_resolution_uses_db_override_and_fails_closed_on_malformed() {
    let db = common::TestDb::create("transcode_template_override").await;
    let (temp, _asset, _bytes) = wav_fixture(44_100);
    let transcoding = service(&temp, db.pool.clone(), 2);

    // Seed default resolves when no override exists.
    let seed = transcoding
        .template(OutputFormat::Mp3)
        .await
        .expect("seed template resolves");
    assert_eq!(seed.bitrate_kbps, 192);

    // A valid DB override wins over the seed (ADR-0147 decision 2 layering).
    let store = SettingsStore::new();
    store
        .set(
            &db.pool,
            "transcoding.command.mp3",
            &serde_json::Value::String(
                "{\"format\":\"Mp3\",\"bitrate\":99,\"command\":\"ffmpeg -i %s -b:a %bk -f mp3 -\"}"
                    .to_string(),
            ),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("override setting");
    let overridden = transcoding
        .template(OutputFormat::Mp3)
        .await
        .expect("override template resolves");
    assert_eq!(overridden.bitrate_kbps, 99);

    // The malformed legacy seed shape fails closed (ADR-0147 consequences).
    store
        .set(
            &db.pool,
            "transcoding.command.mp3",
            &serde_json::Value::String(
                "{ 'format': 'Mp3', 'bitrate: 192, 'command': 'ffmpeg -i %s -f mp3 -' }"
                    .to_string(),
            ),
            None,
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("malformed setting");
    let malformed = transcoding.template(OutputFormat::Mp3).await;
    assert_eq!(malformed, Err(TranscodeError::TemplateUnavailable));
}

#[tokio::test]
async fn real_ffmpeg_wav_to_mp3_output_reparses_with_ffprobe() {
    let (temp, asset, _bytes) = wav_fixture(220_500);
    let transcoding = service(&temp, lazy_pool(), 2);
    let body = transcoding
        .stream(&asset, OutputFormat::Mp3, &seed_mp3_template(), 192)
        .await
        .expect("stream starts");
    let mp3 = body.collect().await.expect("stream completes").to_bytes();
    // 2.5 s of PCM at 192 kbps must produce tens of KB; a header-only or
    // bitrate-clamped output (e.g. `%bk` rendered without the `k` suffix)
    // collapses to near-zero bytes.
    assert!(
        mp3.len() > 20_000,
        "transcoded mp3 carries real audio bytes"
    );
    assert!(
        mp3.starts_with(b"ID3") || (mp3.len() > 2 && mp3[0] == 0xFF && mp3[1] & 0xE0 == 0xE0),
        "output starts with an ID3 tag or MP3 frame sync"
    );
    let out = temp.path().join("out.mp3");
    std::fs::write(&out, &mp3).expect("write mp3");
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&out)
        .output()
        .expect("ffprobe runs");
    assert!(
        probe.status.success(),
        "ffprobe re-parses transcoded output"
    );
    let format = String::from_utf8_lossy(&probe.stdout);
    assert!(
        format.trim().contains("mp3"),
        "ffprobe reports mp3: {format}"
    );
}

#[tokio::test]
async fn gauge_records_saturation_rejection_and_release() {
    let (temp, asset, _bytes) = wav_fixture(44_100);
    let registry = melodee_server::resource_bounds::ResourceBounds::new();
    let gauge = registry.register("streams.transcoding", 1);
    let transcoding = service(&temp, lazy_pool(), 1).with_gauge(gauge.clone());
    let template = seed_mp3_template();

    let first = transcoding
        .stream(&asset, OutputFormat::Mp3, &template, 192)
        .await
        .expect("first stream acquires the permit");
    let snapshot = gauge.snapshot();
    assert_eq!(snapshot.in_flight, 1);
    assert_eq!(snapshot.admitted_total, 1);
    assert!(snapshot.saturated);

    let second = transcoding
        .stream(&asset, OutputFormat::Mp3, &template, 192)
        .await;
    assert!(matches!(second, Err(TranscodeError::CapacityExhausted)));
    assert_eq!(gauge.snapshot().rejected_total, 1);

    drop(first);
    let snapshot = gauge.snapshot();
    assert_eq!(snapshot.in_flight, 0);
    assert!(!snapshot.saturated);

    let classes: Vec<&str> = registry.snapshot().iter().map(|row| row.class).collect();
    assert_eq!(classes, ["streams.transcoding"]);
}
