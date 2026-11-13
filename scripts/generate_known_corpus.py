#!/usr/bin/env python3
"""Generate the known-answer media corpus from `tests/fixtures/media-test-metadata.json`.

Why this exists
---------------
Spot-checking a running instance by eye is how four defects (an unreachable
terminal state, an unreachable Artist status, a dead edit field, and a statistic
with no drill-down rows) reached an operator with every gate green. The remedy is
a *known-answer* corpus: fabricated media whose exact tag values, durations, and
expected catalog outcome are declared up front, so every assertion downstream
compares reality against a declaration instead of against memory.

Design rules
------------
* **Declarative.** All content comes from `media-test-metadata.json`. Adding a
  fixture means adding a JSON object; no Python change is required.
* **No committed binaries.** Audio and artwork are synthesised with ffmpeg on
  demand (sine tones; no copyrighted material). This script and the JSON are the
  durable artifacts — match the existing media-corpus precedent.
* **Self-describing oracle.** Alongside the media this writes
  `expected.json`: the values ffprobe actually reports for each generated track
  plus the derived expectations (sort keys, track counts, totals, folders,
  artwork presence). Downstream verification reads that file, so nothing is
  transcribed by hand.
* **Deterministic.** Fixed sine frequencies, durations, tag values, and
  single-channel 128 kbps MP3 with ID3v2.3, so regenerating produces equivalent
  media on any host with the same ffmpeg major version.

Usage
-----
    python3 scripts/generate_known_corpus.py                    # default output dir
    python3 scripts/generate_known_corpus.py --output /tmp/corpus --tier all
    python3 scripts/generate_known_corpus.py --list             # show fixtures

The generated layout is one directory per Release, ready to drop into an inbound
root:

    <output>/staging/<folder>/01 - <title>.mp3
    <output>/staging/<folder>/cover.jpg           (only for sidecar artwork)
    <output>/expected.json
    <output>/manifest.json                        (what was generated, for evidence)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SPEC_PATH = ROOT / "tests" / "fixtures" / "media-test-metadata.json"
DEFAULT_OUTPUT = Path("/tmp/melodee-known-corpus")

# Sine base frequencies per track index: distinct so tracks are distinguishable
# if anyone listens, and stable so regeneration is byte-stable.
BASE_FREQUENCY_HZ = 220.0

# Each fixture also gets its own frequency offset. Two fixtures that happen to
# share a duration would otherwise generate identical audio, which makes their
# Chromaprint fingerprints identical: with acoustic duplicate detection in place
# a fixture would be classified as a copy of another fixture, and the fixture's
# declared outcome (a policy review, say) would never be exercised. Deriving the
# offset from the slug keeps the corpus byte-stable and the fixtures independent.
FIXTURE_FREQUENCY_STEP_HZ = 6.0
FIXTURE_FREQUENCY_SLOTS = 61


def fixture_frequency_offset(slug: str) -> float:
    """Deterministic, fixture-specific tone offset in ``[0, 61) * 6 Hz``."""
    return FIXTURE_FREQUENCY_STEP_HZ * (sum(slug.encode()) % FIXTURE_FREQUENCY_SLOTS)


class CorpusError(RuntimeError):
    """The corpus cannot be generated (missing tool, bad spec, ffmpeg failure)."""


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        raise CorpusError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{completed.stderr.strip()}"
        )
    return completed


def require_tool(name: str) -> str:
    found = shutil.which(name)
    if not found:
        raise CorpusError(f"{name} is not on PATH; it is required to synthesise the corpus")
    return found


def load_spec(path: Path) -> dict[str, Any]:
    try:
        spec = json.loads(path.read_text())
    except FileNotFoundError as error:
        raise CorpusError(f"spec not found: {path}") from error
    except json.JSONDecodeError as error:
        raise CorpusError(f"spec is not valid JSON: {error}") from error
    if not isinstance(spec, dict) or not spec.get("releases"):
        raise CorpusError("spec must be an object with a non-empty `releases` array")
    slugs = [release.get("slug") for release in spec["releases"]]
    if any(not slug for slug in slugs) or len(slugs) != len(set(slugs)):
        raise CorpusError("every release needs a unique non-empty `slug`")
    return spec


def probe_media(path: Path, ffprobe_binary: str) -> dict[str, Any]:
    completed = run(
        [
            ffprobe_binary,
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            str(path),
        ]
    )
    return json.loads(completed.stdout)


def probe_duration_ms(path: Path, ffprobe_binary: str) -> int:
    data = probe_media(path, ffprobe_binary)
    raw = data.get("format", {}).get("duration")
    if raw is None:
        raise CorpusError(f"ffprobe reported no duration for {path}")
    return int(round(float(raw) * 1000))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_artwork(
    path: Path,
    *,
    width: int,
    height: int,
    colors: list[str],
    ffmpeg: str,
    label: str,
) -> None:
    """Deterministic cover art: a vertical two-colour gradient with a label bar."""
    top = colors[0] if colors else "#101010"
    bottom = colors[1] if len(colors) > 1 else top
    filters = (
        f"gradients=s={width}x{height}:c0={top}:c1={bottom}:x0=0:y0=0:x1=0:y1={height},"
        f"drawbox=x=0:y={int(height * 0.78)}:w={width}:h={int(height * 0.22)}:color=black@0.45:t=fill,"
        f"drawtext=text='{label}':fontcolor=white:fontsize={max(14, width // 14)}"
        f":x=(w-text_w)/2:y=h*0.84"
    )
    run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            filters,
            "-frames:v",
            "1",
            str(path),
        ]
    )


def write_track(
    path: Path,
    *,
    seconds: int,
    frequency: float,
    tags: dict[str, str],
    audio: dict[str, Any],
    ffmpeg: str,
    artwork: Path | None,
) -> None:
    # A plain sine is spectrally constant, and Chromaprint describes spectral
    # *change*: two fixtures with different pitch but identical duration can
    # produce the same fingerprint, which would make the acoustic matcher treat
    # one fixture as a copy of another. Sweeping the pitch at a per-fixture rate
    # makes each fixture acoustically distinct while staying a boring tone.
    sweep_hz = round(frequency * 0.09, 3)
    tone = (
        f"aevalsrc=0.25*sin(2*PI*({frequency}+{sweep_hz}*t)*t):"
        f"d={seconds}:s={audio['sampleRate']}"
    )
    command = [
        ffmpeg,
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        tone,
    ]
    if artwork is not None:
        command += ["-i", str(artwork), "-map", "0:a", "-map", "1:v", "-c:v", "mjpeg"]
    command += [
        "-c:a",
        "libmp3lame",
        "-b:a",
        audio["bitrate"],
        "-ac",
        str(audio["channels"]),
        "-id3v2_version",
        str(audio["id3Version"]),
    ]
    if artwork is not None:
        command += [
            "-metadata:s:v",
            "title=Album cover",
            "-metadata:s:v",
            "comment=Cover (front)",
            "-disposition:v",
            "attached_pic",
        ]
    for key, value in tags.items():
        command += ["-metadata", f"{key}={value}"]
    command.append(str(path))
    run(command)


def truncate(path: Path, percent: int) -> None:
    """Simulate an interrupted transfer: keep `percent` of the bytes."""
    data = path.read_bytes()
    keep = max(1, int(len(data) * percent / 100))
    path.write_bytes(data[:keep])


def track_file_name(track: dict[str, Any]) -> str:
    number = int(track.get("trackNumber", 1))
    disc = track.get("discNumber")
    prefix = f"{disc}-{number:02d}" if disc else f"{number:02d}"
    safe = str(track["title"]).replace("/", "-")
    return f"{prefix} - {safe}.mp3"


def build_release(
    release: dict[str, Any],
    output: Path,
    *,
    ffmpeg: str,
    ffprobe: str,
    audio: dict[str, Any],
    by_slug: dict[str, dict[str, Any]],
    run_id: str,
) -> dict[str, Any]:
    slug = release["slug"]
    # A per-run suffix keeps repeat runs additive: fresh Releases are created
    # instead of colliding with the previous run, so the exercise never needs a
    # destructive catalog reset.
    album = f"{release['album']} [{run_id}]"
    folder = output / "staging" / f"{release['folder']} [{run_id}]"
    folder.mkdir(parents=True, exist_ok=True)

    duplicate_of = release.get("duplicateOf")
    if duplicate_of:
        # A duplicate fixture is a byte-identical copy of another fixture, so the
        # pipeline sees the same audio under a different folder name. That is what
        # exercises audio-identity reuse rather than a second Release.
        source_folder = output / "staging" / by_slug[duplicate_of]["folder"]
        if not source_folder.is_dir():
            raise CorpusError(
                f"{slug}: duplicateOf '{duplicate_of}' must be generated first"
            )
        for item in sorted(source_folder.iterdir()):
            if item.is_file():
                shutil.copyfile(item, folder / item.name)
        tracks = []
        for item in sorted(folder.iterdir()):
            if item.suffix.lower() != ".mp3":
                continue
            tracks.append(
                {
                    "file": item.name,
                    "title": item.stem.split(" - ", 1)[-1],
                    "artist": str(release["releaseArtist"]),
                    "container": "mp3",
                    "codec": "mp3",
                    "sha256": sha256(item),
                    "bytes": item.stat().st_size,
                    "truncated": False,
                    "durationMs": probe_duration_ms(item, ffprobe),
                }
            )
        return {
            "slug": slug,
            "folder": folder.name,
            "purpose": release.get("purpose"),
            "tier": release.get("tier", "core"),
            "album": album,
            "releaseArtist": release["releaseArtist"],
            "displayArtist": release["releaseArtist"],
            "albumArtist": release.get("albumArtist"),
            "year": release["year"],
            "releaseDate": release.get("releaseDate"),
            "genre": release.get("genre"),
            "trackCount": len(tracks),
            "totalDurationMs": sum(track["durationMs"] or 0 for track in tracks),
            "totalBytes": sum(track["bytes"] for track in tracks),
            "artwork": {"present": False, "kind": "duplicate", "sidecar": False},
            "tracks": tracks,
            "expect": release.get("expect", {}),
            "extraTags": release.get("extraTags") or {},
            "duplicateOf": duplicate_of,
            "duplicateFileHashes": [track["sha256"] for track in tracks],
        }

    artwork_spec = release.get("artwork")
    artwork_path: Path | None = None
    artwork_is_sidecar = False
    if artwork_spec:
        wanted = output / "work" / slug / "cover.jpg"
        wanted.parent.mkdir(parents=True, exist_ok=True)
        write_artwork(
            wanted,
            width=int(artwork_spec["width"]),
            height=int(artwork_spec["height"]),
            colors=list(artwork_spec.get("colors", [])),
            ffmpeg=ffmpeg,
            label=album,
        )
        if artwork_spec["kind"] == "sidecar":
            artwork_is_sidecar = True
            artwork_path = folder / str(artwork_spec.get("fileName", "cover.jpg"))
            shutil.copyfile(wanted, artwork_path)
        else:
            artwork_path = wanted

    sidecar = release.get("sidecar")
    if sidecar:
        document_release = dict(sidecar.get("release", {}))
        # The run suffix applies to release-level names everywhere, including a
        # sidecar that overrides the tagged title.
        if "album_title" in document_release:
            document_release["album_title"] = f"{document_release['album_title']} [{run_id}]"
        payload = {"document": {"release": document_release}}
        (folder / str(sidecar.get("fileName", ".blackbeard.provenance.json"))).write_text(
            json.dumps(payload, indent=2) + "\n"
        )

    tracks: list[dict[str, Any]] = []
    for index, track in enumerate(release.get("tracks", []), start=1):
        title = str(track["title"])
        file_name = track_file_name(track)
        path = folder / file_name
        artist = str(track.get("artist") or release["releaseArtist"])
        overrides = release.get("tagOverrides") or {}
        omit = set(overrides.get("omit") or [])
        tags: dict[str, str] = {
            "title": title,
            "artist": artist,
            "album": str(overrides.get("album", album)),
            "date": str(release.get("releaseDate") or release["year"]),
            "track": str(track.get("trackNumber", index)),
            "genre": str(release.get("genre") or ""),
        }
        tagged_album_artist = overrides.get("albumArtist", release.get("albumArtist"))
        if tagged_album_artist:
            tags["album_artist"] = str(tagged_album_artist)
        # Declarative extra native tags. The identity fixtures use this to write a
        # MusicBrainz Artist id, because a provider id is the only tag-level fact
        # that can distinguish two genuinely different artists with one name.
        extra_tags = release.get("extraTags") or {}
        for key, value in extra_tags.items():
            if key in omit:
                tags.pop(key, None)
                continue
            tags[str(key)] = str(value)
        if track.get("discNumber"):
            tags["disc"] = str(track["discNumber"])
        if "album" in omit:
            tags.pop("album", None)
        if "albumArtist" in omit:
            tags.pop("album_artist", None)
        write_track(
            path,
            seconds=int(track["seconds"]),
            frequency=BASE_FREQUENCY_HZ
            + fixture_frequency_offset(str(release.get("slug", release.get("album", ""))))
            + index * 37.0,
            tags=tags,
            audio=audio,
            ffmpeg=ffmpeg,
            artwork=None if artwork_is_sidecar else artwork_path,
        )
        if track.get("truncateToPercent"):
            truncate(path, int(track["truncateToPercent"]))
        entry = {
            "file": file_name,
            "title": title,
            "artist": artist,
            "trackNumber": int(track.get("trackNumber", index)),
            "discNumber": track.get("discNumber"),
            "container": "mp3",
            "codec": "mp3",
            "sha256": sha256(path),
            "bytes": path.stat().st_size,
            "truncated": bool(track.get("truncateToPercent")),
        }
        try:
            entry["durationMs"] = probe_duration_ms(path, ffprobe)
        except CorpusError:
            # A deliberately truncated file may be unprobeable; the oracle records
            # that fact instead of a guessed duration.
            entry["durationMs"] = None
        tracks.append(entry)

    expected = {
        "slug": slug,
        "folder": folder.name,
        "purpose": release.get("purpose"),
        "tier": release.get("tier", "core"),
        "album": album,
        "releaseArtist": release["releaseArtist"],
        "displayArtist": release["releaseArtist"],
        "albumArtist": release.get("albumArtist"),
        "year": release["year"],
        "releaseDate": release.get("releaseDate"),
        "genre": release.get("genre"),
        "trackCount": len(tracks),
        "totalDurationMs": sum(
            track["durationMs"] or 0 for track in tracks if not track["truncated"]
        ),
        "totalBytes": sum(track["bytes"] for track in tracks),
        "artwork": {
            "present": artwork_path is not None,
            "kind": (artwork_spec or {}).get("kind"),
            "sidecar": artwork_is_sidecar,
        },
        "tracks": tracks,
        "extraTags": release.get("extraTags") or {},
        "expect": release.get("expect", {}),
    }
    return expected


def build(
    spec: dict[str, Any],
    output: Path,
    *,
    tier: str,
    run_id: str,
) -> dict[str, Any]:
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)
    ffmpeg = require_tool("ffmpeg")
    ffprobe = require_tool("ffprobe")
    audio = spec["audio"]

    selected = [
        release
        for release in spec["releases"]
        if tier == "all" or release.get("tier", "core") == tier
    ]

    if not selected:
        raise CorpusError(f"tier '{tier}' selected no releases")

    by_slug: dict[str, dict[str, Any]] = {}
    expected_releases: list[dict[str, Any]] = []
    for release in selected:
        expected = build_release(
            release,
            output,
            ffmpeg=ffmpeg,
            ffprobe=ffprobe,
            audio=audio,
            by_slug=by_slug,
            run_id=run_id,
        )
        by_slug[release["slug"]] = expected
        expected_releases.append(expected)

    expected = {
        "schemaVersion": 1,
        "specPath": str(SPEC_PATH.relative_to(ROOT)),
        "tier": tier,
        "runId": run_id,
        "stagingRoot": str(output / "staging"),
        "releases": expected_releases,
    }
    (output / "expected.json").write_text(json.dumps(expected, indent=2) + "\n")

    generated = {
        "generatedAt": subprocess.run(
            ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], capture_output=True, text=True, check=False
        ).stdout.strip(),
        "ffmpegVersion": run([ffmpeg, "-version"]).stdout.splitlines()[0],
        "spec": str(SPEC_PATH.relative_to(ROOT)),
        "tier": tier,
        "releases": [
            {
                "slug": release["slug"],
                "folder": release["folder"],
                "tracks": len(release["tracks"]),
                "bytes": release["totalBytes"],
            }
            for release in expected_releases
        ],
    }
    (output / "manifest.json").write_text(json.dumps(generated, indent=2) + "\n")
    return expected


def base36_token() -> str:
    """Short, sortable, human-readable run token (e.g. `mk3f9q`)."""
    alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
    value = int(time.time()) % (36**5)
    token = ""
    while value:
        value, remainder = divmod(value, 36)
        token = alphabet[remainder] + token
    return (token or "0").rjust(5, "0")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--output", type=Path, default=DEFAULT_OUTPUT, help=f"output directory (default {DEFAULT_OUTPUT})"
    )
    parser.add_argument("--spec", type=Path, default=SPEC_PATH, help="corpus specification JSON")
    parser.add_argument(
        "--tier",
        choices=("core", "faults", "all"),
        default="core",
        help="which fixtures to generate (default: core, the deterministic happy paths)",
    )
    parser.add_argument("--list", action="store_true", help="list fixtures and exit")
    parser.add_argument(
        "--run-id",
        default=None,
        help="suffix appended to album and folder names (default: generated); keeps repeat runs additive",
    )
    args = parser.parse_args()

    try:
        spec = load_spec(args.spec)
        if args.list:
            for release in spec["releases"]:
                print(
                    f"{release.get('tier', 'core'):<7} {release['slug']:<28} "
                    f"{len(release.get('tracks', []))} track(s)  {release.get('purpose', '')}"
                )
            return 0
        run_id = args.run_id or base36_token()
        expected = build(spec, args.output, tier=args.tier, run_id=run_id)
    except CorpusError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    total_tracks = sum(release["trackCount"] for release in expected["releases"])
    total_bytes = sum(release["totalBytes"] for release in expected["releases"])
    print(
        f"corpus: {len(expected['releases'])} release(s), {total_tracks} track(s), "
        f"{total_bytes / 1024:.0f} KiB -> {args.output}"
    )
    print(f"oracle: {args.output / 'expected.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
