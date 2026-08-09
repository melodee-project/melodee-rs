#!/usr/bin/env python3
"""Generate the deterministic synthetic media corpus for the Phase 0 media
baseline (FIRST-10-ISSUES.md Issue 8).

All audio is synthesized with ffmpeg (FOSS, already a pinned tool for the
rewrite) from sine sources with fixed parameters. No copyrighted material is
or can be introduced. The corpus is regenerated on demand into a work
directory (default ``/tmp/media-baseline/corpus``); it is intentionally NOT
committed — the generator is the durable artifact.

Each case directory gets a ``case.json`` describing the intended input
characteristics; the capture harness records outcomes against it.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import wave
import struct
import math
import zlib
from pathlib import Path

DEFAULT_OUT = Path("/tmp/media-baseline/corpus")


def run(cmd: list[str]) -> None:
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"command failed: {' '.join(cmd)}\n{result.stderr[-2000:]}")


def ffmpeg_sine(dest: Path, *, seconds: float = 2.0, frequency: int = 440,
                sample_rate: int = 44100, channels: int = 1, extra: list[str] | None = None) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    run([
        "ffmpeg", "-y", "-v", "error",
        "-f", "lavfi", "-i", f"sine=frequency={frequency}:duration={seconds}:sample_rate={sample_rate}",
        "-ac", str(channels),
        *(extra or []),
        str(dest),
    ])


def write_wav(dest: Path, *, seconds: float, frequency: int, sample_rate: int = 44100,
              channels: int = 1) -> None:
    """Pure-python WAV for cases where ffmpeg container metadata would interfere."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    frames = int(sample_rate * seconds)
    with wave.open(str(dest), "wb") as wav:
        wav.setnchannels(channels)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        samples = bytearray()
        for i in range(frames):
            v = int(12000 * math.sin(2 * math.pi * frequency * i / sample_rate))
            samples += struct.pack("<h", v) * channels
        wav.writeframes(bytes(samples))


def make_png(dest: Path, *, size: int, rgb: tuple[int, int, int]) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b""
    for _ in range(size):
        raw += b"\x00" + bytes(rgb) * size
    dest.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def make_jpeg(dest: Path, *, size: int, rgb: tuple[int, int, int]) -> None:
    png_tmp = dest.with_suffix(".png.tmp")
    make_png(png_tmp, size=size, rgb=rgb)
    run(["ffmpeg", "-y", "-v", "error", "-i", str(png_tmp), str(dest)])
    png_tmp.unlink()


# Standard clean tag set used by several cases.
def clean_tags(artist: str, album: str, title: str, track: int, total: int,
               year: int = 2021, genre: str = "Ambient", album_artist: str | None = None,
               disc: str | None = None, extra: list[str] | None = None) -> list[str]:
    tags = [
        "-metadata", f"title={title}",
        "-metadata", f"artist={artist}",
        "-metadata", f"album_artist={album_artist or artist}",
        "-metadata", f"album={album}",
        "-metadata", f"date={year}",
        "-metadata", f"genre={genre}",
        "-metadata", f"track={track}/{total}",
    ]
    if disc:
        tags += ["-metadata", f"disc={disc}"]
    return tags + (extra or [])


def build_corpus(out: Path) -> dict[str, dict]:
    cases: dict[str, dict] = {}

    def case(slug: str, description: str, expected: dict) -> Path:
        directory = out / slug
        directory.mkdir(parents=True, exist_ok=True)
        cases[slug] = {"slug": slug, "description": description, "expected": expected}
        return directory

    # ---- clean formats (same tags, different codecs) ----
    d = case("clean-mp3", "Clean fully-tagged MP3 (ID3v2) with embedded artwork.", {
        "codec": "mp3", "extractedTags": "full", "conversion": "none",
    })
    ffmpeg_sine(d / "01 First Light.mp3", extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Signal Path", "First Light", 1, 3),
    ])
    ffmpeg_sine(d / "02 Second Wave.mp3", frequency=550, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Signal Path", "Second Wave", 2, 3),
    ])
    ffmpeg_sine(d / "03 Third Echo.mp3", frequency=660, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Signal Path", "Third Echo", 3, 3),
    ])
    make_jpeg(d / "cover.jpg", size=600, rgb=(30, 90, 160))

    d = case("clean-flac", "Clean tagged FLAC (Vorbis comments).", {
        "codec": "flac", "conversion": "mp3 (NeedsConversionToMp3Async)",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.flac", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "flac",
            *clean_tags("Corpus Artist", "Lossless Path", title, i, 3, genre="Electronic"),
        ])
    make_jpeg(d / "cover.jpg", size=600, rgb=(160, 90, 30))

    d = case("clean-ogg", "Clean tagged OGG Vorbis.", {"codec": "vorbis", "conversion": "mp3"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.ogg", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libvorbis", "-q:a", "5",
            *clean_tags("Corpus Artist", "Vorbis Path", title, i, 3, genre="Rock"),
        ])

    d = case("clean-m4a", "Clean tagged AAC/M4A.", {"codec": "aac", "conversion": "mp3"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.m4a", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "aac", "-b:a", "160k",
            *clean_tags("Corpus Artist", "AAC Path", title, i, 3, genre="Jazz"),
        ])

    d = case("clean-opus", "Clean tagged OPUS.", {"codec": "opus", "conversion": "mp3"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.opus", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libopus", "-b:a", "128k",
            *clean_tags("Corpus Artist", "Opus Path", title, i, 3, genre="Folk"),
        ])

    d = case("clean-wav", "Untagged PCM WAV (WAV metadata support is minimal).", {
        "codec": "pcm", "conversion": "mp3", "extractedTags": "filename-fallback",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        write_wav(d / f"0{i} {title}.wav", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110)

    # ---- Unicode ----
    d = case("unicode-tags", "CJK + diacritic + emoji-adjacent metadata.", {
        "codec": "mp3", "normalization": "normalized-name evidence",
    })
    for i, title in enumerate(["Neon 東京", "Eple (Røyskatt Mix)", "Ünïcode Söng"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Sæglópur 日本", "Ünïcode Rëlease 東京", title, i, 3, genre="環境音楽"),
        ])

    # ---- tag pathology ----
    d = case("missing-tags", "No tags at all (bare MP3 stream).", {
        "fallback": "filename/directory", "validation": "fails or flags",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k", "-map_metadata", "-1",
        ])

    d = case("partial-tags", "Only a title tag; no artist/album.", {
        "fallback": "directory-name segments",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k", "-map_metadata", "-1",
            "-metadata", f"title={title}",
        ])

    d = case("duplicate-track-numbers", "Two songs sharing TRCK=1.", {
        "validation": "AreSongsUniquelyNumbered failure expected",
    })
    ffmpeg_sine(d / "01 First Light.mp3", extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Duplicate Path", "First Light", 1, 2),
    ])
    ffmpeg_sine(d / "01b Second Wave.mp3", frequency=550, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Duplicate Path", "Second Wave", 1, 2),
    ])
    ffmpeg_sine(d / "02 Third Echo.mp3", frequency=660, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Duplicate Path", "Third Echo", 2, 2),
    ])

    d = case("missing-track-number", "One song without TRCK.", {
        "validation": "Song.IsValid false (SongNumber 0)",
    })
    ffmpeg_sine(d / "01 First Light.mp3", extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Numberless Path", "First Light", 1, 2),
    ])
    ffmpeg_sine(d / "Unknown Position.mp3", frequency=550, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        "-metadata", "title=Unknown Position",
        "-metadata", "artist=Corpus Artist",
        "-metadata", "album=Numberless Path",
        "-metadata", "album_artist=Corpus Artist",
        "-metadata", "date=2021",
    ])

    # ---- artwork ----
    d = case("embedded-artwork", "Embedded PNG cover in each MP3.", {
        "artwork": "embedded discovered as Front",
    })
    art = d / "_art.png"
    make_png(art, size=600, rgb=(90, 160, 30))
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        dest = d / f"0{i} {title}.mp3"
        run([
            "ffmpeg", "-y", "-v", "error",
            "-f", "lavfi", "-i", f"sine=frequency={440 + (i - 1) * 110}:duration=2.0:sample_rate=44100",
            "-i", str(art),
            "-map", "0:a", "-map", "1:v",
            "-codec:a", "libmp3lame", "-b:a", "192k", "-ac", "1",
            "-c:v", "copy",
            *clean_tags("Corpus Artist", "Embedded Path", title, i, 3),
            "-metadata:s:v", "title=Album cover", "-metadata:s:v", "comment=Cover (front)",
            str(dest),
        ])
    art.unlink()

    d = case("oversized-artwork", "4000x4000 JPEG cover.", {
        "artwork": "resized to square tiers",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Oversized Path", title, i, 3),
        ])
    make_jpeg(d / "cover.jpg", size=4000, rgb=(160, 30, 90))

    d = case("invalid-artwork", "Garbage bytes named cover.jpg.", {
        "artwork": "deleted; album flagged missing cover",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Broken Art Path", title, i, 3),
        ])
    (d / "cover.jpg").write_bytes(b"this is not an image file at all" * 32)

    # ---- audio pathology ----
    d = case("corrupt-audio", "Truncated MP3 (first 4 KiB of a longer encode).", {
        "behavior": "skipped or invalid song",
    })
    full = d / "_full.mp3"
    ffmpeg_sine(full, seconds=2.0, extra=["-codec:a", "libmp3lame", "-b:a", "192k"])
    (d / "01 Truncated.mp3").write_bytes(full.read_bytes()[:4096])
    full.unlink()
    for i, title in enumerate(["Second Wave", "Third Echo"], start=2):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Corrupt Path", title, i, 3),
        ])

    d = case("very-short", "0.5s track (below any sane duration).", {
        "validation": "duration rules",
    })
    for i in range(1, 4):
        ffmpeg_sine(d / f"0{i} Blip {i}.mp3", seconds=0.5, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Short Path", f"Blip {i}", i, 3),
        ])

    d = case("long-track", "One unusually long track (120s) among normal ones.", {
        "validation": "duration rules",
    })
    ffmpeg_sine(d / "01 Marathon.mp3", seconds=120.0, extra=[
        "-codec:a", "libmp3lame", "-b:a", "192k",
        *clean_tags("Corpus Artist", "Long Path", "Marathon", 1, 3),
    ])
    for i, title in enumerate(["Second Wave", "Third Echo"], start=2):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Long Path", title, i, 3),
        ])

    d = case("high-samplerate", "96 kHz FLAC source.", {
        "conversion": "resampled to conversion.samplingRate (48000)",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.flac", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110,
                    sample_rate=96000, extra=[
                        "-codec:a", "flac",
                        *clean_tags("Corpus Artist", "HiRes Path", title, i, 3),
                    ])

    d = case("multichannel", "5.1 channel WAV source.", {
        "conversion": "channels preserved (no channel mapping at baseline)",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        write_wav(d / f"0{i} {title}.wav", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, channels=6)

    d = case("high-bitrate", "320k CBR MP3 (already MP3: no conversion).", {
        "conversion": "none",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "320k",
            *clean_tags("Corpus Artist", "Bitrate Path", title, i, 3),
        ])

    d = case("not-mp3-named-mp3", "WAV content in an .mp3 filename (forces conversion).", {
        "conversion": "mp3 despite extension",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        write_wav(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110)

    # ---- multi-disc ----
    d = case("multi-disc", "Two discs via TPOS tags (disc 1/2 and 2/2).", {
        "disc": "MediaNumber ordering; SortOrder = SongNumber + disc*10000 - 10000",
    })
    for disc, tracks in [(1, ["D1 First", "D1 Second"]), (2, ["D2 First", "D2 Second"])]:
        for i, title in enumerate(tracks, start=1):
            ffmpeg_sine(d / f"{disc}0{i} {title}.mp3", frequency=440 + disc * i * 55, extra=[
                "-codec:a", "libmp3lame", "-b:a", "192k",
                *clean_tags("Corpus Artist", "Disc Path", title, i, 2, disc=f"{disc}/2"),
            ])

    # ---- classification ----
    d = case("title-ep", "Album title containing 'EP'.", {"albumType": "EP (regex on title)"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Signals EP", title, i, 3),
        ])

    d = case("title-live", "Album title containing 'live'.", {"albumType": "Other (live regex)"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Signals Live at the Depot", title, i, 3),
        ])

    d = case("title-featuring", "Featuring fragment in album title (undesired text).", {
        "validation": "Undesired severity (flagged, not invalid)",
    })
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Signals (feat. Guest Vocalist)", title, i, 3),
        ])

    # ---- sidecars ----
    d = case("cue-split", "Single WAV + CUE sheet (split into tracks).", {
        "sidecar": "cue -> ffmpeg split per track",
    })
    write_wav(d / "album.wav", seconds=6.0, frequency=440)
    (d / "album.cue").write_text(
        'PERFORMER "Corpus Artist"\n'
        'TITLE "Cue Path"\n'
        'FILE "album.wav" WAVE\n'
        '  TRACK 01 AUDIO\n'
        '    TITLE "First Light"\n'
        '    PERFORMER "Corpus Artist"\n'
        '    INDEX 01 00:00:00\n'
        '  TRACK 02 AUDIO\n'
        '    TITLE "Second Wave"\n'
        '    PERFORMER "Corpus Artist"\n'
        '    INDEX 01 00:02:00\n'
        '  TRACK 03 AUDIO\n'
        '    TITLE "Third Echo"\n'
        '    PERFORMER "Corpus Artist"\n'
        '    INDEX 01 00:04:00\n',
        encoding="utf-8",
    )

    d = case("sfv-sidecar", "MP3s plus an SFV manifest.", {"sidecar": "sfv parsed"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "SFV Path", title, i, 3),
        ])
    (d / "release.sfv").write_text("; synthetic SFV\n01 First Light.mp3 DEADBEEF\n", encoding="utf-8")

    d = case("m3u-sidecar", "MP3s plus an M3U playlist.", {"sidecar": "m3u parsed"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "M3U Path", title, i, 3),
        ])
    (d / "release.m3u").write_text(
        "#EXTM3U\n01 First Light.mp3\n02 Second Wave.mp3\n03 Third Echo.mp3\n",
        encoding="utf-8",
    )

    d = case("nfo-sidecar", "MP3s plus an NFO file.", {"sidecar": "nfo parsed"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "NFO Path", title, i, 3),
        ])
    (d / "release.nfo").write_text("Synthetic NFO\nArtist: Corpus Artist\nAlbum: NFO Path\n", encoding="utf-8")

    # ---- same-size destructive-dedupe demonstration ----
    d = case("same-size-files", "Two DIFFERENT songs whose converted sizes collide.", {
        "legacyBug": "FindDuplicatesAsync deletes every same-size file after the first, "
                     "regardless of SHA256 equality (FileSystemDirectoryInfoExtensions.cs:28-89). "
                     "Documented destructive behavior; the Rust pipeline must compare content "
                     "before deleting anything.",
    })
    for i, title in enumerate(["First Light", "Second Wave"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Collision Path", title, i, 2),
        ])
    # Equalize byte sizes exactly with a padding comment tag on the smaller file.
    files = sorted(d.glob("*.mp3"), key=lambda f: f.stat().st_size)
    delta = files[-1].stat().st_size - files[0].stat().st_size
    if delta > 0:
        run([
            "ffmpeg", "-y", "-v", "error", "-i", str(files[0]),
            "-codec", "copy", "-metadata", f"comment={'x' * delta}",
            str(d / "_padded.mp3"),
        ])
        (d / "_padded.mp3").replace(files[0])
    assert len({f.stat().st_size for f in d.glob('*.mp3')}) == 1, "sizes must collide"

    # ---- multi-file album with lyrics sidecar ----
    d = case("lrc-sidecar", "MP3 plus an .lrc lyrics sidecar.", {"sidecar": "lrc (lyrics plugin)"})
    for i, title in enumerate(["First Light", "Second Wave", "Third Echo"], start=1):
        ffmpeg_sine(d / f"0{i} {title}.mp3", seconds=2.0 + (i - 1) * 0.13, frequency=440 + (i - 1) * 110, extra=[
            "-codec:a", "libmp3lame", "-b:a", "192k",
            *clean_tags("Corpus Artist", "Lyric Path", title, i, 3),
        ])
    (d / "01 First Light.lrc").write_text(
        "[00:00.00] Synthetic lyric line one\n[00:01.00] Synthetic lyric line two\n",
        encoding="utf-8",
    )

    (out / "case-index.json").write_text(json.dumps(list(cases.values()), indent=2) + "\n")
    return cases


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args()
    if not any(Path(p).exists() for p in [Path("/usr/bin/ffmpeg"), Path("/usr/local/bin/ffmpeg")]):
        print("ffmpeg is required", file=sys.stderr)
        raise SystemExit(1)
    if args.output.exists():
        import shutil
        shutil.rmtree(args.output)
    cases = build_corpus(args.output)
    files = sum(len(list((args.output / slug).iterdir())) for slug in cases)
    print(f"corpus: {len(cases)} cases, {files} files -> {args.output}")


if __name__ == "__main__":
    main()
