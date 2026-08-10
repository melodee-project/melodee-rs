#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

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
