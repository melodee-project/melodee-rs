#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use melodee_domain::ObservedFileKind;
use melodee_media::{
    InspectionOptions, NormalizationError, NormalizationOptions, inspect_release, normalize_release,
};
use uuid::Uuid;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "melodee-real-normalization-test-{}",
            Uuid::now_v7().simple()
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tools_available() -> bool {
    ["ffmpeg", "ffprobe", "fpcalc"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn run(command: &mut Command) {
    let output = command.output().expect("start media tool");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_audio(path: &Path, codec_args: &[&str], frequency: u32) {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency={frequency}:duration=5"))
        .args(codec_args)
        .arg(path);
    run(&mut command);
}

fn generate_image(path: &Path, color: &str, size: &str) {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c={color}:s={size}"))
        .args(["-frames:v", "1"])
        .arg(path);
    run(&mut command);
}

fn generate_multichannel_wav(path: &Path) {
    let output = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("aevalsrc=sin(2*PI*220*t)|sin(2*PI*330*t)|sin(2*PI*440*t)|sin(2*PI*550*t)|sin(2*PI*660*t)|sin(2*PI*770*t):s=48000:d=5:c=5.1")
        .args(["-c:a", "pcm_s16le"])
        .arg(path)
        .output()
        .expect("generate multichannel WAV");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn file_hashes(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, path: &Path, hashes: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(path).expect("read source tree") {
            let entry = entry.expect("source entry");
            let file_type = entry.file_type().expect("source type");
            if file_type.is_dir() {
                visit(root, &entry.path(), hashes);
            } else if file_type.is_file() {
                let relative = entry.path().strip_prefix(root).unwrap().to_path_buf();
                let bytes = fs::read(entry.path()).expect("read source file");
                hashes.insert(relative, blake3::hash(&bytes).to_hex().to_string());
            }
        }
    }

    let mut hashes = BTreeMap::new();
    visit(root, root, &mut hashes);
    hashes
}

fn probe(path: &Path) -> serde_json::Value {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).expect("ffprobe JSON")
}

fn normalization_options(work_area: &Path, attempt_id: Uuid) -> NormalizationOptions {
    NormalizationOptions {
        work_area: work_area.to_path_buf(),
        attempt_id,
        ffmpeg_timeout: Duration::from_secs(30),
        fpcalc_timeout: Duration::from_secs(30),
        ffprobe_timeout: Duration::from_secs(10),
        ..NormalizationOptions::default()
    }
}

#[cfg(unix)]
fn write_script(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn tool_path(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .expect("media tool on PATH")
}

#[cfg(unix)]
fn inspected_single_wav(source: &Path) -> melodee_domain::ReleaseManifest {
    fs::create_dir(source).unwrap();
    generate_audio(&source.join("track.wav"), &["-c:a", "pcm_s16le"], 440);
    inspect_release(
        Uuid::now_v7(),
        None,
        "failure-case",
        source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .unwrap()
    .manifest
}

#[test]
fn real_format_matrix_is_verified_source_safe_and_deterministic() {
    if !tools_available() {
        eprintln!("skipping real normalization test: ffmpeg, ffprobe, or fpcalc unavailable");
        return;
    }

    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    let work = temp.path().join("work");
    fs::create_dir_all(source.join("disc one")).unwrap();
    generate_audio(
        &source.join("disc one/-- leading 音楽.flac"),
        &[
            "-c:a",
            "flac",
            "-metadata",
            "title=Synthetic FLAC",
            "-metadata",
            "REPLAYGAIN_TRACK_GAIN=-7.00 dB",
        ],
        330,
    );
    generate_audio(
        &source.join("track with spaces.ogg"),
        &["-c:a", "libvorbis", "-q:a", "5"],
        440,
    );
    generate_audio(&source.join("track.wav"), &["-c:a", "pcm_s16le"], 550);
    generate_audio(
        &source.join("track.m4a"),
        &["-c:a", "aac", "-b:a", "192k"],
        660,
    );
    generate_audio(
        &source.join("retained-44k.mp3"),
        &["-c:a", "libmp3lame", "-q:a", "4", "-ar", "44100"],
        770,
    );
    generate_audio(
        &source.join("retained-cbr.mp3"),
        &["-c:a", "libmp3lame", "-b:a", "320k", "-ar", "44100"],
        880,
    );
    generate_multichannel_wav(&source.join("multichannel.wav"));

    generate_image(&source.join("cover.jpg"), "red", "320x200");
    generate_image(&source.join("folder.png"), "blue", "200x320");
    fs::write(source.join("broken.jpg"), b"not an image").unwrap();
    let mut oversized_header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    oversized_header.extend_from_slice(&20_000_u32.to_be_bytes());
    oversized_header.extend_from_slice(&20_000_u32.to_be_bytes());
    fs::write(source.join("absurd.png"), oversized_header).unwrap();

    fs::write(
        source.join("release.cue"),
        "FILE \"track.wav\" WAVE\n TRACK 01 AUDIO\n  TITLE \"Track\"\n  INDEX 01 00:00:00\n",
    )
    .unwrap();
    fs::write(
        source.join("release.m3u"),
        "#EXTM3U\n#EXTINF:5,Track\ntrack with spaces.ogg\n",
    )
    .unwrap();
    fs::write(
        source.join("release.sfv"),
        "track with spaces.ogg ABCDEF01\n",
    )
    .unwrap();
    fs::write(
        source.join("release.nfo"),
        "<release><title>Test</title></release>",
    )
    .unwrap();
    fs::write(
        source.join("release.lrc"),
        "[00:01.00][00:02.00]Synthetic words\n",
    )
    .unwrap();

    let source_before = file_hashes(&source);
    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "release",
        &source,
        Some(1),
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect synthetic release");
    let original_manifest = inspected.manifest;

    let first_attempt = Uuid::now_v7();
    let mut first_manifest = original_manifest.clone();
    let first = normalize_release(
        &mut first_manifest,
        &source,
        &normalization_options(&work, first_attempt),
    )
    .expect("first normalization");
    assert_eq!(first.outputs_produced, 7);
    assert_eq!(first.fingerprints_produced, 7);
    assert_eq!(first.sidecars_parsed, 5);
    assert_eq!(first.artwork_produced, 2);
    assert!(
        first
            .findings
            .iter()
            .any(|finding| finding.code == "artwork.normalization_failed")
    );

    let normalization = first.manifest.normalization.as_ref().unwrap();
    let attempt_root = PathBuf::from(
        normalization
            .provenance
            .work_area
            .as_deref()
            .expect("attempt work area"),
    );
    for output in &normalization.outputs {
        let path = attempt_root.join(&output.output_path);
        assert!(path.is_file());
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(blake3::hash(&bytes).to_hex().as_str(), output.byte_hash);
        let document = probe(&path);
        let audio = document["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .unwrap();
        assert_eq!(audio["codec_name"], "mp3");
        if output.retained {
            assert_eq!(audio["sample_rate"], "44100");
            assert_eq!(
                fs::read(&path).unwrap(),
                fs::read(source.join(&output.source_file)).unwrap()
            );
        } else {
            assert_eq!(audio["sample_rate"], "48000");
        }
        if output.source_file.ends_with(".flac") {
            let tags = document["format"]["tags"].as_object().unwrap();
            assert!(tags.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("title") && value == "Synthetic FLAC"
            }));
            assert!(tags.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("replaygain_track_gain") && value == "-7.00 dB"
            }));
        }
        if output.source_file == "multichannel.wav" {
            assert_eq!(audio["channels"], 2);
        }
    }
    assert_eq!(source_before, file_hashes(&source), "source media changed");

    let mut second_manifest = original_manifest;
    let second = normalize_release(
        &mut second_manifest,
        &source,
        &normalization_options(&work, Uuid::now_v7()),
    )
    .expect("second normalization");
    assert_eq!(first.manifest_hash, second.manifest_hash);
    let first_normalization = first.manifest.normalization.unwrap();
    let second_normalization = second.manifest.normalization.unwrap();
    assert_eq!(first_normalization.outputs, second_normalization.outputs);
    assert_eq!(
        first_normalization.fingerprints,
        second_normalization.fingerprints
    );
    assert_eq!(
        first_normalization.normalized_artwork,
        second_normalization.normalized_artwork
    );
    assert_eq!(
        first_normalization.parsed_sidecars,
        second_normalization.parsed_sidecars
    );
    assert_ne!(
        first_attempt,
        second_normalization.provenance.attempt_id.unwrap()
    );
}

/// content-based detection, mirroring legacy compatibility case
/// `not-mp3-named-mp3` (compatibility/media/cases/not-mp3-named-mp3.json):
/// WAV content wearing `.mp3` filenames must be detected from probed streams
/// (not the extension) and converted to MP3 — the captured expectation
/// `conversion: mp3 despite extension`.
#[test]
fn wav_content_in_mp3_named_files_is_detected_from_content_and_converted() {
    if !tools_available() {
        eprintln!("skipping real content-detection test: ffmpeg, ffprobe, or fpcalc unavailable");
        return;
    }

    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    for (name, frequency) in [
        ("01 First Light.mp3", 440),
        ("02 Second Wave.mp3", 550),
        ("03 Third Echo.mp3", 660),
    ] {
        let output = Command::new("ffmpeg")
            .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("sine=frequency={frequency}:duration=5"))
            .args(["-c:a", "pcm_s16le", "-f", "wav"])
            .arg(source.join(name))
            .output()
            .expect("generate WAV content in an .mp3 filename");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "not-mp3",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect misnamed release");
    assert_eq!(inspected.manifest.files.len(), 3);
    for file in &inspected.manifest.files {
        assert_eq!(file.kind, ObservedFileKind::Audio);
        let audio = file.audio.as_ref().expect("probed audio observation");
        assert_eq!(audio.container.as_deref(), Some("wav"));
        assert_eq!(audio.codec.as_deref(), Some("pcm_s16le"));
        assert_eq!(
            file.mime_type.as_deref(),
            Some("audio/wav"),
            "probed content must override the extension-derived MIME for {file:?}"
        );
        assert!(
            file.findings
                .iter()
                .any(|finding| finding.code == "media.extension_mismatch"),
            "mismatch finding recorded for {file:?}"
        );
    }

    let source_before = file_hashes(&source);
    let mut manifest = inspected.manifest;
    let report = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .expect("normalize misnamed release");
    assert_eq!(report.outputs_produced, 3);
    let normalization = report.manifest.normalization.as_ref().unwrap();
    let attempt_root = PathBuf::from(
        normalization
            .provenance
            .work_area
            .as_deref()
            .expect("attempt work area"),
    );
    for output in &normalization.outputs {
        assert!(
            !output.retained,
            "WAV content must convert despite the .mp3 name"
        );
        assert_eq!(output.codec, "mp3");
        let document = probe(&attempt_root.join(&output.output_path));
        let audio = document["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .unwrap();
        assert_eq!(audio["codec_name"], "mp3");
        assert_eq!(audio["sample_rate"], "48000");
    }
    assert_eq!(source_before, file_hashes(&source), "source media changed");
}

/// zero-length fixture: inspection must record an actionable finding
/// and normalization must refuse to publish unverified output.
#[test]
fn zero_length_audio_is_an_actionable_finding_and_blocks_normalization() {
    if !tools_available() {
        eprintln!("skipping real zero-length test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("empty.flac"), b"").unwrap();

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "zero-length",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect zero-length release");
    let file = &inspected.manifest.files[0];
    assert_eq!(file.kind, ObservedFileKind::Audio);
    assert!(
        file.findings
            .iter()
            .any(|finding| finding.code == "media.empty_file"),
        "zero-length audio is an actionable inspection finding"
    );
    // ffprobe's raw-FLAC demuxer still answers for an empty file; the
    // observation records that degenerate truth (no duration) rather than
    // hiding it.
    let audio = file.audio.as_ref().expect("degenerate probe observation");
    assert!(audio.duration_ms.is_none());

    let mut manifest = inspected.manifest;
    let error = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap_err();
    assert!(
        matches!(error, NormalizationError::Ffmpeg(_)),
        "ffmpeg refuses empty input, got {error:?}"
    );
    assert!(manifest.normalization.is_none());
}

/// truncated fixture: a FLAC cut mid-stream still probes (STREAMINFO
/// carries the full duration) but must fail normalization actionably — the
/// encode either errors or decodes short and trips output duration
/// verification — and nothing is published.
#[test]
fn truncated_audio_fails_normalization_without_publishing() {
    if !tools_available() {
        eprintln!("skipping real truncation test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let truncated = source.join("truncated.flac");
    generate_audio(&truncated, &["-c:a", "flac"], 440);
    let bytes = fs::read(&truncated).unwrap();
    fs::write(&truncated, &bytes[..bytes.len() * 2 / 5]).unwrap();

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "truncated",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect truncated release");
    let file = &inspected.manifest.files[0];
    let audio = file.audio.as_ref().expect("STREAMINFO still probes");
    assert_eq!(audio.codec.as_deref(), Some("flac"));
    assert!(
        audio.duration_ms.is_some_and(|duration| duration > 4_000),
        "STREAMINFO reports the full 5 s duration"
    );

    let source_before = file_hashes(&source);
    let mut manifest = inspected.manifest;
    let error = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            NormalizationError::Verification(_) | NormalizationError::Ffmpeg(_)
        ),
        "typed actionable failure, got {error:?}"
    );
    assert!(manifest.normalization.is_none());
    assert_eq!(source_before, file_hashes(&source), "source media changed");
}

/// multi-stream fixture: a container with more than one audio stream is
/// observed with its true stream count and rejected actionably — Melodee
/// never guesses which stream is the music.
#[test]
fn multi_stream_audio_is_observed_and_rejected_actionably() {
    if !tools_available() {
        eprintln!("skipping real multi-stream test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=550:duration=2"])
        .args(["-map", "0:a:0", "-map", "1:a:0", "-c:a", "aac"])
        .arg(source.join("two-streams.m4a"));
    run(&mut command);

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "multi-stream",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect multi-stream release");
    let file = &inspected.manifest.files[0];
    assert_eq!(file.audio.as_ref().unwrap().stream_count, Some(2));

    let mut manifest = inspected.manifest;
    let error = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap_err();
    assert!(matches!(error, NormalizationError::Verification(_)));
    assert!(manifest.normalization.is_none());
}

/// oversized-tag fixture: embedded artwork beyond the configured
/// observation bound is skipped with a structured finding; audio observation
/// and normalization are unaffected.
#[test]
fn oversized_embedded_artwork_is_bounded_and_recorded() {
    if !tools_available() {
        eprintln!("skipping real oversized-tag test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let audio = source.join("base.flac");
    generate_audio(&audio, &["-c:a", "flac"], 440);
    // Random-noise PNG compresses to roughly raw size, reliably exceeding
    // the 16 MiB observation bound at 4096x4096 RGB.
    let oversized = source.join("oversized.png");
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("nullsrc=s=4096x4096,noise=alls=100:allf=u")
        .args(["-frames:v", "1"])
        .arg(&oversized);
    run(&mut command);
    assert!(
        fs::metadata(&oversized).unwrap().len() > 16 * 1024 * 1024,
        "noise fixture must exceed the observation bound"
    );

    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error", "-i"])
        .arg(&audio)
        .arg("-i")
        .arg(&oversized)
        .args([
            "-map",
            "0:a:0",
            "-map",
            "1:v:0",
            "-c:a",
            "libmp3lame",
            "-q:a",
            "4",
            "-c:v",
            "copy",
            "-disposition:v:0",
            "attached_pic",
            "-id3v2_version",
            "3",
        ])
        .arg(source.join("oversized-tag.mp3"));
    run(&mut command);
    fs::remove_file(&audio).unwrap();
    fs::remove_file(&oversized).unwrap();

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "oversized-tag",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect oversized-tag release");
    let file = &inspected.manifest.files[0];
    assert!(
        file.findings
            .iter()
            .any(|finding| finding.code == "artwork.embedded_oversized"),
        "oversized artwork finding recorded"
    );
    assert!(
        file.embedded_artwork.is_empty(),
        "oversized artwork is skipped, not observed"
    );
    assert!(file.audio.is_some(), "audio observation unaffected");

    let mut manifest = inspected.manifest;
    let report = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .expect("audio still normalizes");
    assert_eq!(report.outputs_produced, 1);
    assert_eq!(report.artwork_produced, 0);
}

/// unsupported-content fixture: an extension in the audio vocabulary
/// whose probed content carries no audio stream is recorded as unsupported
/// with an actionable finding and rejected by normalization.
#[test]
fn video_only_container_is_recorded_unsupported_and_rejected() {
    if !tools_available() {
        eprintln!("skipping real video-only test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=1:size=320x240:rate=10")
        .args(["-c:v", "mpeg4"])
        .arg(source.join("video-only.mp4"));
    run(&mut command);

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "video-only",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect video-only release");
    let file = &inspected.manifest.files[0];
    assert!(file.audio.is_none(), "no audio stream observed");
    assert_eq!(
        file.kind_code.as_deref(),
        Some("unsupported_or_corrupt_audio")
    );
    assert!(
        file.findings
            .iter()
            .any(|finding| finding.code == "media.ffprobe_failed")
    );

    let mut manifest = inspected.manifest;
    let error = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap_err();
    assert!(matches!(error, NormalizationError::Verification(_)));
    assert!(manifest.normalization.is_none());
}

/// per-format normalization-input rows for the remaining matrix
/// vocabulary: AAC (ADTS), AC-3, WMA, and Opus sources must be observed from
/// probed content and converted to verified canonical MP3, with the source
/// tree byte-identical afterwards.
#[test]
fn aac_ac3_wma_and_opus_inputs_normalize_to_verified_mp3() {
    if !tools_available() {
        eprintln!("skipping real format-conversion test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let cases: [(&str, &[&str]); 4] = [
        ("aac", &["-c:a", "aac", "-f", "adts"]),
        ("ac3", &["-c:a", "ac3", "-f", "ac3"]),
        ("wma", &["-c:a", "wmav2"]),
        ("opus", &["-c:a", "libopus"]),
    ];
    for (extension, codec_args) in cases {
        generate_audio(&source.join(format!("track.{extension}")), codec_args, 440);
    }

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "format-conversion",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect conversion formats");
    assert_eq!(inspected.manifest.files.len(), 4);
    for file in &inspected.manifest.files {
        assert!(
            file.audio.is_some(),
            "probed observation for {}",
            file.relative_path
        );
    }

    let source_before = file_hashes(&source);
    let mut manifest = inspected.manifest;
    let report = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .expect("normalize conversion formats");
    assert_eq!(report.outputs_produced, 4);
    let normalization = report.manifest.normalization.as_ref().unwrap();
    let attempt_root = PathBuf::from(
        normalization
            .provenance
            .work_area
            .as_deref()
            .expect("attempt work area"),
    );
    for output in &normalization.outputs {
        assert!(!output.retained, "{} must convert", output.source_file);
        assert_eq!(output.codec, "mp3");
        assert_eq!(output.sample_rate_hz, 48_000);
        let document = probe(&attempt_root.join(&output.output_path));
        let audio = document["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .unwrap();
        assert_eq!(audio["codec_name"], "mp3", "{}", output.source_file);
    }
    assert_eq!(source_before, file_hashes(&source), "source media changed");
}

#[test]
fn real_embedded_jpeg_and_png_are_normalized_to_verified_jpeg() {
    if !tools_available() {
        eprintln!("skipping real artwork test: ffmpeg, ffprobe, or fpcalc unavailable");
        return;
    }

    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let audio = source.join("base.flac");
    let jpeg = source.join("embedded.jpg");
    let png = source.join("embedded.png");
    generate_audio(&audio, &["-c:a", "flac"], 440);
    generate_image(&jpeg, "green", "300x180");
    generate_image(&png, "yellow", "180x300");

    for (input_art, output_name) in [(&jpeg, "with-jpeg.mp3"), (&png, "with-png.mp3")] {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-y", "-v", "error", "-i"])
            .arg(&audio)
            .arg("-i")
            .arg(input_art)
            .args([
                "-map",
                "0:a:0",
                "-map",
                "1:v:0",
                "-c:a",
                "libmp3lame",
                "-q:a",
                "4",
                "-c:v",
                "copy",
                "-disposition:v:0",
                "attached_pic",
                "-metadata:s:v:0",
                "comment=Cover (front)",
                "-id3v2_version",
                "3",
            ])
            .arg(source.join(output_name));
        run(&mut command);
    }
    fs::remove_file(audio).unwrap();
    fs::remove_file(jpeg).unwrap();
    fs::remove_file(png).unwrap();

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "embedded",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .unwrap();
    assert_eq!(
        inspected
            .manifest
            .files
            .iter()
            .map(|file| file.embedded_artwork.len())
            .sum::<usize>(),
        2
    );
    let mut manifest = inspected.manifest;
    let report = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap();
    let normalization = report.manifest.normalization.unwrap();
    assert_eq!(normalization.normalized_artwork.len(), 2);
    let attempt_root = PathBuf::from(normalization.provenance.work_area.unwrap());
    assert_eq!(normalization.outputs.len(), 2);
    for output in &normalization.outputs {
        assert!(output.retained);
        assert_eq!(
            fs::read(attempt_root.join(&output.output_path)).unwrap(),
            fs::read(source.join(&output.source_file)).unwrap()
        );
    }
    for artwork in normalization.normalized_artwork {
        assert_eq!(artwork.mime_type, "image/jpeg");
        assert_eq!((artwork.width, artwork.height), (1600, 1600));
        assert!(!artwork.byte_hash.is_empty());
        let bytes = fs::read(attempt_root.join(artwork.output_path)).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]));
    }
}

#[cfg(unix)]
#[test]
fn source_change_and_disappearance_during_normalization_are_not_published() {
    if !tools_available() {
        eprintln!("skipping real source-race test: media tools unavailable");
        return;
    }

    for (name, mutation) in [
        ("change", "printf 'changed' >> \"$5\""),
        ("disappear", "rm -f -- \"$5\""),
    ] {
        let temp = TestDirectory::new();
        let source = temp.path().join("source");
        let mut manifest = inspected_single_wav(&source);
        let fpcalc = write_script(
            temp.path(),
            &format!("fpcalc-{name}"),
            &format!(
                "if [ \"$#\" -lt 5 ]; then printf 'fpcalc test wrapper'; exit 0; fi\nprintf '{{\"duration\":5,\"fingerprint\":\"synthetic\"}}'\n{mutation}"
            ),
        );
        let mut options = normalization_options(&temp.path().join("work"), Uuid::now_v7());
        options.fpcalc_path = fpcalc;
        let error = normalize_release(&mut manifest, &source, &options).unwrap_err();
        if name == "change" {
            assert!(matches!(error, NormalizationError::SourceChanged(_)));
        } else {
            assert!(matches!(error, NormalizationError::SourceMissing(_)));
        }
        assert!(manifest.normalization.is_none());
    }
}

#[cfg(unix)]
#[test]
fn unprobeable_or_unhashable_ffmpeg_output_blocks_completion() {
    if !tools_available() {
        eprintln!("skipping real output-verification test: media tools unavailable");
        return;
    }
    let real_ffmpeg = tool_path("ffmpeg");
    for (name, mutation) in [
        ("unprobeable", ": > \"$last\""),
        ("unhashable", "chmod 000 \"$last\""),
    ] {
        let temp = TestDirectory::new();
        let source = temp.path().join("source");
        let mut manifest = inspected_single_wav(&source);
        let wrapper = write_script(
            temp.path(),
            &format!("ffmpeg-{name}"),
            &format!(
                "'{}' \"$@\" || exit $?\nif [ \"$#\" -gt 1 ]; then\n  last=''\n  for argument in \"$@\"; do last=\"$argument\"; done\n  {mutation}\nfi",
                real_ffmpeg.display()
            ),
        );
        let mut options = normalization_options(&temp.path().join("work"), Uuid::now_v7());
        options.ffmpeg_path = wrapper;
        let error = normalize_release(&mut manifest, &source, &options).unwrap_err();
        assert!(matches!(error, NormalizationError::Verification(_)));
        assert!(manifest.normalization.is_none());
    }
}

#[test]
fn corrupt_audio_is_rejected_by_normalization() {
    if !tools_available() {
        eprintln!("skipping real corrupt-input test: media tools unavailable");
        return;
    }
    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("corrupt.flac"),
        b"fLaC definitely not valid audio",
    )
    .unwrap();
    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "corrupt",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .unwrap();
    let mut manifest = inspected.manifest;
    let error = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .unwrap_err();
    assert!(matches!(error, NormalizationError::Verification(_)));
    assert!(manifest.normalization.is_none());
}

/// ADR-0146 end-to-end proof, mirroring the frozen legacy capture
/// `compatibility/media/cases/cue-split.json` (one WAV + one CUE sheet → one
/// album of per-track MP3 segments): a CUE-governed source produces one
/// verified MP3 segment per CUE track, the whole-file output is suppressed,
/// manifest Tracks join on the deterministic segment pseudo-path, metadata
/// observations come from the sheet, and the staged source is never modified.
#[test]
fn cue_governed_source_splits_into_verified_per_track_segments() {
    if !tools_available() {
        eprintln!("skipping real CUE segmentation test: ffmpeg, ffprobe, or fpcalc unavailable");
        return;
    }

    let temp = TestDirectory::new();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    // Six seconds so three two-second segments stay well inside the output
    // duration verification tolerance.
    let output = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=6")
        .args(["-c:a", "pcm_s16le"])
        .arg(source.join("album.wav"))
        .output()
        .expect("generate CUE source WAV");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Quoted FILE, spaces, and a Unicode title cover the sheet-lexing
    // dimensions of the fixture list in the end-to-end path.
    fs::write(
        source.join("album.cue"),
        "PERFORMER \"Cue Artist\"\n\
         TITLE \"Cue Path\"\n\
         FILE \"album.wav\" WAVE\n\
         \x20 TRACK 01 AUDIO\n\
         \x20   TITLE \"First Light\"\n\
         \x20   INDEX 01 00:00:00\n\
         \x20 TRACK 02 AUDIO\n\
         \x20   TITLE \"Second Wave Ω\"\n\
         \x20   PERFORMER \"Guest Artist\"\n\
         \x20   INDEX 01 00:02:00\n\
         \x20 TRACK 03 AUDIO\n\
         \x20   TITLE \"Third Echo\"\n\
         \x20   INDEX 01 00:04:00\n",
    )
    .unwrap();

    let inspected = inspect_release(
        Uuid::now_v7(),
        None,
        "cue-split",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("inspect CUE release");
    assert_eq!(inspected.manifest.files.len(), 2);
    assert!(
        inspected.manifest.files.iter().any(
            |file| file.kind == ObservedFileKind::CueSheet && file.relative_path == "album.cue"
        ),
        "the sheet is observed as a CUE sidecar"
    );

    let source_before = file_hashes(&source);
    let mut manifest = inspected.manifest;
    let report = normalize_release(
        &mut manifest,
        &source,
        &normalization_options(&temp.path().join("work"), Uuid::now_v7()),
    )
    .expect("normalize CUE-governed release");
    assert_eq!(report.outputs_produced, 3);
    let normalization = report.manifest.normalization.as_ref().unwrap();
    let attempt_root = PathBuf::from(
        normalization
            .provenance
            .work_area
            .as_deref()
            .expect("attempt work area"),
    );

    // Exactly the three planned segment outputs exist; the governed source's
    // whole-file output is suppressed.
    assert_eq!(normalization.outputs.len(), 3);
    for output in &normalization.outputs {
        assert_eq!(output.source_file, "album.wav");
        assert!(
            !output.retained,
            "segments are encoded, never byte-retained from the source"
        );
        assert_eq!(output.codec, "mp3");
        let segment = output.segment.as_ref().expect("CUE segment identity");
        assert_eq!(segment.cue, "album.cue");
        assert_eq!(
            output.track_key(),
            format!("album.wav#cue-track-{:02}", segment.track),
            "deterministic segment pseudo-path"
        );
        let document = probe(&attempt_root.join(&output.output_path));
        let audio = document["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .unwrap();
        assert_eq!(audio["codec_name"], "mp3");
        assert_eq!(audio["sample_rate"], "48000");
        let duration_ms = (document["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            * 1_000.0) as u64;
        assert!(
            duration_ms.abs_diff(2_000) <= 300,
            "segment {} duration {duration_ms} ms strays from the two-second boundary",
            segment.track
        );
    }
    let mut track_numbers: Vec<u32> = normalization
        .outputs
        .iter()
        .map(|output| output.segment.as_ref().unwrap().track)
        .collect();
    track_numbers.sort_unstable();
    assert_eq!(track_numbers, [1, 2, 3]);

    // The segment carries the sheet's metadata observations (track 2 has a
    // track-level performer override and a Unicode title).
    let second = normalization
        .outputs
        .iter()
        .find(|output| output.segment.as_ref().unwrap().track == 2)
        .unwrap();
    let tags = probe(&attempt_root.join(&second.output_path));
    assert_eq!(tags["format"]["tags"]["title"], "Second Wave Ω");
    assert_eq!(tags["format"]["tags"]["artist"], "Guest Artist");
    assert_eq!(tags["format"]["tags"]["album"], "Cue Path");
    assert_eq!(tags["format"]["tags"]["album_artist"], "Cue Artist");
    assert_eq!(tags["format"]["tags"]["track"], "2");

    // The inspection-time whole-file Track is replaced by one Track per CUE
    // segment, joined on the segment pseudo-path, with release observations
    // filled from the sheet.
    assert_eq!(report.manifest.tracks.len(), 3);
    let expected_titles = ["First Light", "Second Wave Ω", "Third Echo"];
    for (index, track) in report.manifest.tracks.iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap();
        assert_eq!(track.file_path, format!("album.wav#cue-track-{number:02}"));
        assert_eq!(track.track_number, Some(number));
        assert_eq!(track.sort_order, Some(number));
        assert_eq!(track.track_title.as_deref(), Some(expected_titles[index]));
        let metadata = track.observed_metadata.as_ref().expect("CUE metadata");
        assert_eq!(metadata.album.as_deref(), Some("Cue Path"));
        assert_eq!(metadata.album_artist.as_deref(), Some("Cue Artist"));
        assert_eq!(metadata.track_total, Some(3));
    }
    assert_eq!(
        report.manifest.tracks[1]
            .observed_metadata
            .as_ref()
            .unwrap()
            .artist
            .as_deref(),
        Some("Guest Artist")
    );

    // The sheet remains a parsed sidecar observation.
    assert!(
        normalization
            .parsed_sidecars
            .iter()
            .any(|sidecar| sidecar.sidecar_type == "cue" && sidecar.relative_path == "album.cue"),
        "the governing sheet stays on the record as a parsed sidecar"
    );

    // The staged source is never rewritten, retagged, split in place, or
    // deleted; segment bytes exist only in the attempt work area.
    assert_eq!(source_before, file_hashes(&source), "source media changed");

    // Segment identity is deterministic for retries: a fresh attempt produces
    // identical work-area-relative output paths and byte hashes.
    let reinspected = inspect_release(
        Uuid::now_v7(),
        None,
        "cue-split",
        &source,
        None,
        Some("stable"),
        &InspectionOptions::default(),
    )
    .expect("re-inspect CUE release");
    let mut second_manifest = reinspected.manifest;
    let second_report = normalize_release(
        &mut second_manifest,
        &source,
        &normalization_options(&temp.path().join("work-retry"), Uuid::now_v7()),
    )
    .expect("retry normalization");
    let first_identity: Vec<(String, String)> = normalization
        .outputs
        .iter()
        .map(|output| (output.output_path.clone(), output.byte_hash.clone()))
        .collect();
    let mut second_identity: Vec<(String, String)> = second_report
        .manifest
        .normalization
        .as_ref()
        .unwrap()
        .outputs
        .iter()
        .map(|output| (output.output_path.clone(), output.byte_hash.clone()))
        .collect();
    second_identity.sort();
    let mut first_sorted = first_identity;
    first_sorted.sort();
    assert_eq!(first_sorted, second_identity);
}
