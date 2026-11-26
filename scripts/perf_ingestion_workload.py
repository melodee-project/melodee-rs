#!/usr/bin/env python3
"""Exercise real server ingestion on a fixture-owned database and synthetic audio.

Run as a command of perf_representative.py, with a prebuilt release server:
  python3 scripts/perf_representative.py -- python3 scripts/perf_ingestion_workload.py --albums 1
No existing media directory or external server is touched. Results are JSON on stdout;
process timings include worker polling. Generated media and logs are temporary.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit

from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]
SUCCESS = frozenset({"cataloged", "indexing", "indexed"})
FAILURE = frozenset({"failed", "quarantined", "needs_review", "ignored", "deleted"})


def query(url: str, statement: str) -> dict:
    result = subprocess.run(
        ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url,
         "--tuples-only", "--no-align", "--command", statement],
        check=True, capture_output=True, text=True,
    )
    return json.loads(result.stdout)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def generate_corpus(inbound: Path, albums: int) -> int:
    total = 0
    for index in range(albums):
        release = inbound / f"bench-release-{index:04d}"
        release.mkdir()
        codec = "mp3" if index % 2 == 0 else "flac"
        target = release / f"01-track.{codec}"
        # Separate generated sources avoid hardlinks and cover both retained MP3
        # and FLAC-to-MP3 paths. These synthetic tones are not real music.
        command = ["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                   f"sine=frequency={220 + index}:duration=5", "-metadata", f"album=Benchmark {index}",
                   "-metadata", f"album_artist=Benchmark Artist {index}", "-metadata", f"artist=Benchmark Artist {index}",
                   "-metadata", "title=Benchmark Track", "-metadata", "track=1", "-c:a",
                   "libmp3lame" if codec == "mp3" else "flac", str(target)]
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        total += target.stat().st_size
        (release / ".melodee-ready").touch()
    return total


def queue_reimport(root: Path) -> None:
    """Atomically expose each complete, byte-identical copy to discovery."""
    for original in sorted((root / "originals").iterdir()):
        scratch = root / "reimport-scratch" / original.name
        shutil.copytree(original, scratch)
        scratch.rename(root / "inbound" / original.name)


def server_environment(root: Path, database_url: str, concurrency: int) -> dict[str, str]:
    # Never import the developer's server settings or unrelated storage roots.
    env = {key: value for key, value in os.environ.items() if not key.startswith("MELODEE__")}
    env.update({
        "MELODEE__DATABASE__URL": database_url,
        "MELODEE__SERVER__BIND": "127.0.0.1:0",
        "MELODEE__NATIVE__AUTH__JWT_KEY": "benchmark-only-signing-key-67a49e48f0162e773e12",
        "MELODEE__JELLYFIN__TOKEN_PEPPER": "benchmark-only-token-pepper-84f0d9f006fc671a991e",
        "MELODEE__INGEST__ENABLED": "true",
        "MELODEE__INGEST__SOURCE_ROOT": str(root / "inbound"),
        "MELODEE__INGEST__STAGING_ROOT": str(root / "staging"),
        "MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS": "1",
        "MELODEE__INGEST__SETTLING_QUIET_SECONDS": "0",
        "MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS": "0",
        "MELODEE__NORMALIZATION__WORK_AREA": str(root / "work"),
        "MELODEE__STORAGE__ROOT": str(root / "storage"),
        "MELODEE__STORAGE__ASSET_ROOT": str(root / "assets"),
        "MELODEE__INSPECTION__CONCURRENCY": str(concurrency),
        "MELODEE__NORMALIZATION__CONCURRENCY": str(concurrency),
        "MELODEE__VALIDATION__CONCURRENCY": str(concurrency),
        "MELODEE__PUBLICATION__CONCURRENCY": str(concurrency),
        "MELODEE__CATALOG__ENABLED": "true",
        "MELODEE__CATALOG__POLL_INTERVAL_SECONDS": "1",
        "MELODEE__SEARCH__ENABLED": "false",
        "MELODEE__PODCAST__ENABLED": "false",
        "MELODEE__PODCAST__WORKER__ENABLED": "false",
        "MELODEE__POLICY__WORKER__ENABLED": "false",
        "MELODEE__DISCOVERY__WORKER__ENABLED": "false",
        "MELODEE__ENRICHMENT__WORKER__ENABLED": "false",
    })
    return env


STATUS_SQL = """SELECT json_build_object(
 'database', current_database(),
 'states', COALESCE((SELECT json_object_agg(state, n) FROM
   (SELECT state, count(*) n FROM release_work_items GROUP BY state) s), '{}'::json),
 'items', COALESCE((SELECT json_agg(json_build_object(
    'source', w.source_path, 'state', w.state, 'error_code', w.last_error_code,
    'discovered_at', w.discovered_at, 'updated_at', w.updated_at,
    'events', (SELECT COALESCE(json_agg(json_build_object(
       'type', e.event_type, 'from', e.from_state, 'to', e.to_state,
       'time', e.created_at) ORDER BY e.id), '[]'::json)
       FROM release_work_item_events e WHERE e.work_item_id = w.id)) ORDER BY w.source_path)
    FROM release_work_items w), '[]'::json))"""

IDENTITIES_SQL = """SELECT json_build_object('rows', COALESCE(json_agg(
 json_build_object('release_id', r.id, 'release_public_id', r.public_api_id,
                   'track_id', t.id, 'track_public_id', t.public_api_id)
 ORDER BY r.id, t.id), '[]'::json))
 FROM releases r LEFT JOIN tracks t ON t.release_id = r.id
 WHERE r.title LIKE 'Benchmark %'"""


def run(albums: int, concurrency: int, timeout: int, server: Path, env: dict[str, str],
        reimport: bool = False) -> int:
    validate_fixture(env)
    if not 1 <= albums <= 200 or not 1 <= concurrency <= 16 or timeout < 1:
        raise ValueError("albums must be 1..200, concurrency 1..16, and timeout positive")
    server = server.resolve(strict=True)
    if not server.is_file():
        raise ValueError("server must be a prebuilt release executable")
    database_url = env["MELODEE__DATABASE__URL"]
    initial = query(database_url, STATUS_SQL)
    if initial["database"] != env["MELODEE_PERF_OWNED_DATABASE"] or initial["items"]:
        raise ValueError("fixture database must match its ownership marker and contain no work items")
    with tempfile.TemporaryDirectory(prefix="melodee-perf-ingestion-") as temporary:
        root = Path(temporary)
        for name in ("inbound", "staging", "work", "storage", "assets"):
            (root / name).mkdir()
        corpus_bytes = generate_corpus(root / "inbound", albums)
        if reimport:
            shutil.copytree(root / "inbound", root / "originals")
            (root / "reimport-scratch").mkdir()
        started = time.monotonic()
        snapshot = initial
        server_exit_code = None
        first_pass = None
        first_identities = None
        second_started = None
        phase = "initial"
        with (root / "server.log").open("wb") as log:
            process = subprocess.Popen([str(server)], cwd=root, env=server_environment(root, database_url, concurrency),
                                       stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=os.name == "posix")
            try:
                deadline = started + timeout
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        server_exit_code = process.returncode
                        break
                    snapshot = query(database_url, STATUS_SQL)
                    states = snapshot["states"]
                    succeeded = sum(states.get(state, 0) for state in SUCCESS)
                    if sum(states.get(state, 0) for state in FAILURE):
                        break
                    if phase == "initial" and succeeded == albums:
                        first_pass = snapshot
                        if not reimport:
                            break
                        first_identities = query(database_url, IDENTITIES_SQL)
                        second_started = time.monotonic()
                        queue_reimport(root)
                        phase = "reimport"
                    elif phase == "reimport" and succeeded == albums * 2:
                        break
                    time.sleep(1)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    if os.name == "posix":
                        os.killpg(process.pid, signal.SIGKILL)
                    else:
                        process.kill()
                    process.wait()
        elapsed = time.monotonic() - started
        diagnostic = []
        if server_exit_code is not None:
            log_text = (root / "server.log").read_text(errors="replace")
            parsed_url = urlsplit(database_url)
            secrets = (database_url, parsed_url.username, parsed_url.password,
                       "benchmark-only-signing-key-67a49e48f0162e773e12",
                       "benchmark-only-token-pepper-84f0d9f006fc671a991e", str(root))
            for secret in sorted((item for item in secrets if item), key=len, reverse=True):
                if secret:
                    log_text = log_text.replace(secret, "<redacted>")
            diagnostic = log_text.splitlines()[-20:]
        states = snapshot["states"]
        succeeded = sum(states.get(state, 0) for state in SUCCESS)
        expected = albums * (2 if reimport else 1)
        final_identities = query(database_url, IDENTITIES_SQL) if second_started is not None else None
        commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                                text=True, check=True).stdout.strip()
        dirty = bool(subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT,
                                    capture_output=True, text=True, check=True).stdout.strip())
        result = {"profile": "release binary supplied by operator; not independently verified",
                  "binary_sha256": file_sha256(server), "repository_head": commit,
                  "tracked_worktree_dirty": dirty, "platform": platform.platform(),
                  "albums": albums, "tracks": albums,
                  "mix": "alternating generated retained MP3 and FLAC, one five-second tone per release",
                  "corpus_bytes": corpus_bytes, "concurrency_per_stage": concurrency,
                  "seconds": round(elapsed, 3), "success_count": succeeded,
                  "expected_success_count": expected,
                  "redrop_requested": reimport,
                  "reimport_seconds": round(time.monotonic() - second_started, 3) if second_started is not None else None,
                  "initial_states": first_pass["states"] if first_pass is not None else None,
                  "release_track_identifiers_unchanged": (
                      len(first_identities["rows"]) == albums and first_identities == final_identities
                  ) if first_identities is not None and final_identities is not None else None,
                  "initial_identity_rows": len(first_identities["rows"]) if first_identities is not None else None,
                  "initial_release_track_identifiers": first_identities["rows"] if first_identities is not None else None,
                  "final_release_track_identifiers": final_identities["rows"] if final_identities is not None else None,
                  "server_exit_code": server_exit_code,
                  "server_exit_log_tail": diagnostic,
                  "timed_out": server_exit_code is None and succeeded != expected and not any(states.get(state, 0) for state in FAILURE),
                  "states": states, "items": snapshot["items"],
                  "limitations": ["one synthetic five-second track per release; no artwork or CUE",
                                  "same-filesystem acquisition only; no retry or fault injection",
                                  "search, policy, discovery, enrichment and podcast workers disabled",
                                  "no interactive load, streaming, syscall or subprocess counts collected",
                                  "cache condition uncontrolled", "worker polling included in elapsed time",
                                  "redrop creates new work items; ADR-0116 reserves distinct Release IDs and this is not a same-item retry",
                                  "identifier comparison covers only release and track identifiers, not every catalog relationship"]}
        print(json.dumps(result, sort_keys=True))
        return 0 if succeeded == expected else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--albums", type=int, default=1)
    parser.add_argument("--concurrency", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--redrop", "--reimport", dest="reimport", action="store_true",
                        help="redrop identical bytes as new work items; this is not a same-item retry")
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    args = parser.parse_args()
    try:
        return run(args.albums, args.concurrency, args.timeout, args.server, os.environ.copy(), args.reimport)
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Ingestion benchmark failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
