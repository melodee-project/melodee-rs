#!/usr/bin/env python3
"""Capture the Phase 0 media-processing behavioral baseline (Issue 8).

Drives the legacy pipeline through mcli against the synthetic corpus from
``scripts/generate_media_corpus.py``:

1. ``extract``   — per-case metadata extraction via ``mcli tags <file> show``
                   (the exact production AtlMetaTag + MetaTagsProcessor path).
2. ``pipeline``  — per-case inbound->staging run via ``mcli library process``
                   (path-based mode; conversion, artwork, validation, naming,
                   sidecars, and failure behavior are recorded from the
                   resulting staging tree + melodee.json).
3. ``settings``  — seeded conversion/imaging/validation/transcoding defaults
                   read from the disposable database.

Outputs ``compatibility/media/baseline.json`` (machine-readable aggregate)
plus per-case files under ``compatibility/media/cases/``. Volatile fields
(ids, timestamps, uniqueIds, absolute paths, sizes that depend on the host
ffmpeg build) are normalized or flagged.

The disposable database (melodee_phase0) provides settings only; the pipeline
runs are path-based and write only under /tmp/media-baseline.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MEDIA_OUT = ROOT / "compatibility" / "media"
WORK = Path(os.environ.get("MEDIA_BASELINE_WORK", "/tmp/media-baseline"))
CORPUS = WORK / "corpus"
MCLI_DIR = Path(os.environ.get("LEGACY_ROOT", str(ROOT.parent / "melodee"))) / "src" / "Melodee.Cli" / "bin" / "Debug" / "net10.0"

DB = {
    "host": os.environ.get("PHASE0_DB_HOST", "127.0.0.1"),
    "port": os.environ.get("PHASE0_DB_PORT", "5433"),
    "name": os.environ.get("PHASE0_DB_NAME", "melodee_phase0"),
    "user": os.environ.get("PHASE0_DB_USER", "melodee"),
    "password": os.environ.get("PHASE0_DB_PASSWORD", "melodee-dev"),
}

MEDIA_EXTENSIONS = {".mp3", ".flac", ".ogg", ".opus", ".m4a", ".wav"}


def mcli_env() -> dict[str, str]:
    return {
        "HOME": os.environ.get("HOME", "/tmp"),
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "COLUMNS": "4000",
        "security_secretKey": "phase0-fixture-only-security-secret-key-NOT-A-REAL-SECRET-0000",
        "ConnectionStrings__DefaultConnection": (
            f"Host={DB['host']};Port={DB['port']};Database={DB['name']};"
            f"Username={DB['user']};Password={DB['password']};SSL Mode=Disable"
        ),
        "ConnectionStrings__MusicBrainzConnection": "Data Source=/tmp/legacy-oracle/storage/_search-engines/musicbrainz/musicbrainz.ddb;",
        "ConnectionStrings__ArtistSearchEngineConnection": "Data Source=/tmp/legacy-oracle/storage/_search-engines/artistSearchEngine.db;Cache=Shared",
    }


def run_mcli(args: list[str], timeout: int = 600) -> dict:
    result = subprocess.run(
        ["dotnet", "mcli.dll", *args],
        cwd=MCLI_DIR,
        env=mcli_env(),
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return {
        "exitCode": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
    }


def parse_panel_json(stdout: str) -> dict | None:
    # Spectre wraps long output with literal newlines, including inside JSON
    # string literals (illegal JSON); flatten first.
    flattened = stdout.replace("\r", " ").replace("\n", " ")
    start = flattened.find("{")
    if start >= 0:
        try:
            doc, _ = json.JSONDecoder().raw_decode(flattened, start)
            return doc
        except json.JSONDecodeError:
            pass
    parts = []
    for line in stdout.splitlines():
        if "│" in line:
            parts.append(line.split("│", 1)[1].rsplit("│", 1)[0])
    text = "\n".join(parts)
    start = text.find("{")
    if start < 0:
        return None
    try:
        doc, _ = json.JSONDecoder().raw_decode(text, start)
        return doc
    except json.JSONDecodeError:
        return None


def media_files(directory: Path) -> list[Path]:
    return sorted(
        p for p in directory.iterdir() if p.is_file() and p.suffix.lower() in MEDIA_EXTENSIONS
    )


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def summarize_tags(tags: list[dict] | None) -> dict[str, object]:
    summary: dict[str, object] = {}
    for tag in tags or []:
        identifier = tag.get("identifier")
        if identifier:
            summary[identifier] = tag.get("value")
    return summary


def extract_case(case_dir: Path) -> dict:
    extractions = []
    for media in media_files(case_dir)[:2]:
        result = run_mcli(["tags", str(media), "show", "--verbose"])
        doc = parse_panel_json(result["stdout"])
        data = (doc or {}).get("data") or {}
        extractions.append(
            {
                "file": media.name,
                "exitCode": result["exitCode"],
                "exitCodeNote": "mcli tags show always exits 1 (isValid never set) — recorded CLI defect",
                "tags": summarize_tags(data.get("tags")),
                "mediaAudios": summarize_tags(data.get("mediaAudios")),
                "imageCount": len(data.get("images") or []),
                "crcHash": data.get("crcHash"),
            }
        )
    return {"extractions": extractions}


def stage_case(case_dir: Path) -> dict:
    inbound = WORK / "run" / "inbound" / case_dir.name
    staging = WORK / "run" / "staging"
    shutil.rmtree(WORK / "run", ignore_errors=True)
    inbound.mkdir(parents=True)
    staging.mkdir(parents=True)
    shutil.copytree(case_dir, inbound, dirs_exist_ok=True)

    result = run_mcli(["library", "process", "--inbound", str(inbound), "--staging", str(staging)])

    staging_tree = []
    melodee_files = []
    for path in sorted(staging.rglob("*")):
        if path.is_file():
            rel = str(path.relative_to(staging))
            entry = {"path": rel, "size": path.stat().st_size}
            if path.suffix == ".json":
                melodee_files.append(path)
                entry["sha256"] = None
            staging_tree.append(entry)

    albums = []
    for melodee_file in melodee_files:
        album = json.loads(melodee_file.read_text(encoding="utf-8"))
        albums.append(
            {
                "directory": str(melodee_file.parent.relative_to(staging)),
                "status": album.get("status"),
                "statusReasons": album.get("statusReasons"),
                "albumType": album.get("albumType"),
                "validationMessages": [
                    {"message": m.get("message"), "severity": m.get("severity")}
                    for m in album.get("validationMessages") or []
                ],
                "songCount": len(album.get("songs") or []),
                "songs": [
                    {
                        "fileName": (s.get("file") or {}).get("name"),
                        "size": (s.get("file") or {}).get("size"),
                        "title": next(
                            (t.get("value") for t in (s.get("tags") or []) if t.get("identifier") == "Title"),
                            None,
                        ),
                        "songNumber": next(
                            (t.get("value") for t in (s.get("tags") or []) if t.get("identifier") == "TrackNumber"),
                            None,
                        ),
                        "mediaNumber": next(
                            (t.get("value") for t in (s.get("tags") or []) if t.get("identifier") == "DiscNumber"),
                            None,
                        ),
                        "sortOrder": s.get("sortOrder"),
                        "crcHash": s.get("crcHash"),
                    }
                    for s in album.get("songs") or []
                ],
                "images": [
                    {
                        "name": (i.get("fileInfo") or {}).get("name"),
                        "pictureIdentifier": i.get("pictureIdentifier"),
                        "width": i.get("width"),
                        "height": i.get("height"),
                        "wasEmbeddedInSong": i.get("wasEmbeddedInSong"),
                        "originalFilename": i.get("originalFilename"),
                    }
                    for i in album.get("images") or []
                ],
                "displaySummary": album.get("displaySummary"),
            }
        )

    inbound_leftovers = [
        {"path": str(p.relative_to(inbound)), "size": p.stat().st_size}
        for p in sorted(inbound.rglob("*"))
        if p.is_file()
    ]

    # Validation via the production AlbumValidator against each staged album.
    validations = []
    for melodee_file in melodee_files:
        v = run_mcli(["validate", "album", "--file", str(melodee_file), "--json"])
        vdoc = parse_panel_json(v["stdout"]) or {}
        validations.append(
            {
                "album": str(melodee_file.parent.name),
                "exitCode": v["exitCode"],
                "result": vdoc,
            }
        )

    return {
        "processExitCode": result["exitCode"],
        "processSummary": parse_process_summary(result["stdout"]),
        "processErrorTail": (result["stderr"] or "")[-1500:] or None,
        "stagingTree": staging_tree,
        "albums": albums,
        "inboundLeftovers": inbound_leftovers,
        "validations": validations,
    }


def parse_process_summary(stdout: str) -> dict[str, object]:
    summary: dict[str, object] = {}
    for line in stdout.splitlines():
        if "│" not in line:
            continue
        content = line.split("│", 1)[1].rsplit("│", 1)[0].strip()
        for key in ("Elapsed Time", "Directories Processed", "Albums Processed",
                    "Valid Albums", "New Artists", "New Albums", "New Songs",
                    "Processing Errors"):
            if content.startswith(key):
                summary[key] = content[len(key):].strip()
    return summary


def seeded_settings() -> dict[str, object]:
    env = {**os.environ, "PGPASSWORD": DB["password"]}
    result = subprocess.run(
        [
            "psql", "-h", DB["host"], "-p", DB["port"], "-U", DB["user"], "-d", DB["name"],
            "-t", "-A", "-F", "\t", "-c",
            "SELECT \"Key\", \"Value\" FROM \"Settings\" WHERE \"Key\" ~ '^(conversion|imaging|validation|processing|transcoding|magic|lyrics)\\.' ORDER BY \"Key\"",
        ],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    settings = {}
    for line in result.stdout.strip().splitlines():
        key, _, value = line.partition("\t")
        settings[key] = value
    return settings


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", nargs="*", default=None, help="limit to specific case slugs")
    parser.add_argument("--skip-pipeline", action="store_true")
    parser.add_argument("--skip-extract", action="store_true")
    args = parser.parse_args()

    index = json.loads((CORPUS / "case-index.json").read_text(encoding="utf-8"))
    selected = [c for c in index if args.cases is None or c["slug"] in args.cases]
    cases_out = MEDIA_OUT / "cases"
    cases_out.mkdir(parents=True, exist_ok=True)

    baseline_cases = []
    for case in selected:
        slug = case["slug"]
        case_dir = CORPUS / slug
        record: dict[str, object] = {
            "slug": slug,
            "description": case["description"],
            "expected": case["expected"],
            "input": {
                "files": sorted(p.name for p in case_dir.iterdir() if p.is_file()),
                "sizes": {p.name: p.stat().st_size for p in case_dir.iterdir() if p.is_file()},
                "sha256": {
                    p.name: sha256(p)
                    for p in case_dir.iterdir()
                    if p.is_file() and p.suffix.lower() in MEDIA_EXTENSIONS
                },
            },
        }
        if not args.skip_extract:
            record["extraction"] = extract_case(case_dir)
        if not args.skip_pipeline:
            record["pipeline"] = stage_case(case_dir)
        (cases_out / f"{slug}.json").write_text(
            json.dumps(record, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        baseline_cases.append(record)
        status = "?"
        if record.get("pipeline", {}).get("albums"):
            status = str(record["pipeline"]["albums"][0]["status"])
        print(f"{slug}: done (album status: {status})")

    # Aggregate ALL case files on disk (subset runs must not shrink the index).
    all_case_files = sorted(cases_out.glob("*.json"))
    all_cases = [json.loads(p.read_text(encoding="utf-8")) for p in all_case_files]
    baseline = {
        "schemaVersion": 1,
        "legacyBaselineCommit": "4028b58dbd3302ed431e2c5b32d9dde5634db218",
        "capturedAt": "see per-case files; regenerated by scripts/capture_media_baseline.py",
        "environment": {
            "pipeline": "mcli (same Melodee.Common code as the server); path-based inbound->staging",
            "artistSearchEngine": "EMPTY in the fixture environment — ArtistHasSearchEngineResult fails every album; statuses are environment-conditional and recorded as such",
            "seededSettings": seeded_settings(),
        },
        "cases": [{"slug": c["slug"], "expected": c["expected"]} for c in all_cases],
        "caseFiles": [f"cases/{p.name}" for p in all_case_files],
    }
    (MEDIA_OUT / "baseline.json").write_text(
        json.dumps(baseline, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"baseline written: {len(all_cases)} cases -> compatibility/media/baseline.json")


if __name__ == "__main__":
    main()
