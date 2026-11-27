#!/usr/bin/env python3
"""Measure real concurrent genre imports and worker scheduling on an owned fixture.

Run under perf_representative.py. The server, media, and database are disposable;
the prebuilt release executable exercises the regular worker/import paths. The
sampled states and lock waiters are observations, not continuous tracing.
"""
from __future__ import annotations

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from perf_ingestion_workload import FAILURE, STATUS_SQL, SUCCESS, file_sha256, server_environment
from perf_ingress_workload import host_info, server_resources, stop_server, temporary_storage_info
from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]
GENRE = "Melodee Synthetic Concurrent Genre"
POLL_SECONDS = 0.25
MAX_TIMEOUT_SECONDS = 900
MAX_LOG_BYTES = 64 * 1024 * 1024
MESSAGES = {
    "inspection batch completed": "inspection",
    "normalization batch completed": "normalization",
    "validation batch completed": "validation",
    "publication batch completed": "publication",
    "bounded catalog import pass completed": "catalog",
}

SAMPLE_SQL = """SELECT json_build_object(
 'states', COALESCE((SELECT json_object_agg(state,n) FROM
     (SELECT state,count(*) n FROM release_work_items GROUP BY state) q),'{}'::json),
 'leased', COALESCE((SELECT json_object_agg(state,n) FROM
     (SELECT state,count(*) n FROM release_work_items WHERE lease_owner IS NOT NULL GROUP BY state) q),'{}'::json),
 'lock_waiters', (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
     AND pid<>pg_backend_pid() AND wait_event_type='Lock'),
 'advisory_waiters', (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
     AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND lower(wait_event)='advisory'))"""

GENRE_SQL = """SELECT json_build_object(
 'rows', COALESCE((SELECT json_agg(json_build_object('surface',surface,
    'song_count',song_count,'release_count',release_count) ORDER BY surface)
    FROM catalog_genre_aggregates WHERE display_value='Melodee Synthetic Concurrent Genre'), '[]'::json),
 'actual', COALESCE((SELECT json_agg(json_build_object('surface',surface,
    'song_count',songs,'release_count',releases) ORDER BY surface) FROM
      (SELECT term.surface, count(*) songs, count(DISTINCT term.release_id) releases
       FROM catalog_genre_terms term JOIN releases release ON release.id=term.release_id
       WHERE term.display_value='Melodee Synthetic Concurrent Genre' AND release.status='active'
       GROUP BY term.surface) counts), '[]'::json))"""

ENVIRONMENT_SQL = """SELECT json_build_object(
 'postgres_version', current_setting('server_version'),
 'shared_buffers', current_setting('shared_buffers'),
 'work_mem', current_setting('work_mem'),
 'max_connections', current_setting('max_connections'),
 'artists', (SELECT count(*) FROM artists),
 'releases', (SELECT count(*) FROM releases),
 'tracks', (SELECT count(*) FROM tracks))"""


def query(url: str, statement: str) -> dict:
    result = subprocess.run(
        ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url,
         "--tuples-only", "--no-align", "--command", statement],
        check=True, capture_output=True, text=True, timeout=15,
    )
    return json.loads(result.stdout)


def start_server(server: Path, root: Path, database_url: str):
    """Start a private worker process; no HTTP listener/port lookup is needed."""
    child_env = server_environment(root, database_url, 2)
    child_env.pop("MELODEE_BIND", None)
    child_env.pop("RUST_LOG", None)
    child_env.update({"MELODEE__LOGGING__FORMAT": "json", "MELODEE__LOGGING__LEVEL": "info"})
    log = (root / "server.log").open("wb")
    try:
        process = subprocess.Popen([str(server)], cwd=root, env=child_env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=os.name == "posix")
    except BaseException:
        log.close()
        raise
    return process, log


def generate(inbound: Path, releases: int) -> int:
    total = 0
    for index in range(releases):
        directory = inbound / f"genre-benchmark-{index:04d}"
        directory.mkdir()
        extension = "mp3" if index % 2 == 0 else "flac"
        target = directory / f"01-track.{extension}"
        subprocess.run([
            "ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
            f"sine=frequency={220 + index}:duration=5", "-metadata", f"album=Genre Bench {index}",
            "-metadata", f"album_artist=Genre Artist {index}", "-metadata", f"artist=Genre Artist {index}",
            "-metadata", "title=Genre Track", "-metadata", "track=1", "-metadata", f"genre={GENRE}",
            "-c:a", "libmp3lame" if extension == "mp3" else "flac", str(target),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
        total += target.stat().st_size
        (directory / ".melodee-ready").touch()
    return total


def event_waits(items: list[dict]) -> list[float]:
    """Initial Ready-to-first-claim latency; event history includes queue time."""
    waits = []
    for item in items:
        ready = None
        for event in item.get("events", []):
            at = datetime.fromisoformat(event["time"].replace("Z", "+00:00"))
            if event.get("to") == "ready" and ready is None:
                ready = at
            elif ready is not None and event.get("type") == "claimed":
                waits.append(max(0.0, (at - ready).total_seconds()))
                break
    return waits


def percentile(values: list[float], percentile_rank: int) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, (len(ordered) * percentile_rank + 99) // 100 - 1))
    return round(ordered[index], 6)


def summarize_log(path: Path) -> dict[str, dict[str, int]]:
    if path.stat().st_size > MAX_LOG_BYTES:
        raise RuntimeError("owned server log exceeded the benchmark bound")
    result = {name: {"passes": 0, "idle_passes": 0, "claimed": 0} for name in MESSAGES.values()}
    with path.open(encoding="utf-8", errors="replace") as source:
        for line in source:
            try:
                fields = json.loads(line).get("fields", {})
            except (ValueError, AttributeError):
                continue
            stage = MESSAGES.get(fields.get("message"))
            if stage is None:
                continue
            claimed = fields.get("claimed")
            if not isinstance(claimed, int) or claimed < 0:
                continue
            result[stage]["passes"] += 1
            result[stage]["claimed"] += claimed
            result[stage]["idle_passes"] += int(claimed == 0)
    return result


def exact_genre(genre: dict, releases: int) -> bool:
    expected = [{"surface": surface, "song_count": releases, "release_count": releases}
                for surface in ("native", "opensubsonic")]
    return genre.get("rows") == expected and genre.get("actual") == expected


def run(releases: int, timeout: int, server: Path, env: dict[str, str], parent: Path | None) -> int:
    validate_fixture(env)
    if not 2 <= releases <= 200 or not 1 <= timeout <= MAX_TIMEOUT_SECONDS:
        raise ValueError("releases must be 2..200 and timeout 1..900 seconds")
    server = server.resolve(strict=True)
    if not server.is_file():
        raise ValueError("server must be a regular release executable")
    if parent is not None:
        parent = parent.resolve(strict=True)
        if not parent.is_dir():
            raise ValueError("temporary parent must be a directory")
    database_url = env["MELODEE__DATABASE__URL"]
    initial = query(database_url, STATUS_SQL)
    if initial["database"] != env["MELODEE_PERF_OWNED_DATABASE"] or initial["items"]:
        raise ValueError("fixture identity mismatch or existing work items")
    if query(database_url, GENRE_SQL) != {"rows": [], "actual": []}:
        raise ValueError("synthetic genre already exists")
    with tempfile.TemporaryDirectory(prefix="melodee-perf-genre-", dir=parent) as temporary:
        root = Path(temporary)
        for name in ("inbound", "staging", "work", "storage", "assets"):
            (root / name).mkdir()
        corpus_bytes = generate(root / "inbound", releases)
        started = time.monotonic()
        process, log = start_server(server, root, database_url)
        log_path = Path(log.name)
        samples = []
        timed_out = False
        failure = None
        status = initial
        genre = {"rows": [], "actual": []}
        environment = None
        resources = None
        storage = temporary_storage_info(root)
        try:
            deadline = started + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    failure = "server_exited"
                    break
                try:
                    sample = query(database_url, SAMPLE_SQL)
                except subprocess.TimeoutExpired:
                    failure = "sampling_query_timeout"
                    timed_out = True
                    break
                sample["elapsed_seconds"] = round(time.monotonic() - started, 3)
                samples.append(sample)
                states = sample["states"]
                if any(states.get(state, 0) for state in FAILURE):
                    failure = "work_item_failure"
                    break
                if sum(states.get(state, 0) for state in SUCCESS) == releases:
                    break
                time.sleep(POLL_SECONDS)
            else:
                timed_out = True
            elapsed = time.monotonic() - started
            try:
                status = query(database_url, STATUS_SQL)
                genre = query(database_url, GENRE_SQL)
                environment = query(database_url, ENVIRONMENT_SQL)
            except subprocess.TimeoutExpired:
                failure = "final_query_timeout"
                timed_out = True
            # A fixed short idle interval samples idle poll cadence, outside throughput timing.
            if failure is None and not timed_out:
                time.sleep(3)
            resources = server_resources(process.pid)
        finally:
            stop_server(process, log)
        passes = summarize_log(log_path)
        waits = event_waits(status["items"])
        exact = exact_genre(genre, releases)
        success = sum(status["states"].get(state, 0) for state in SUCCESS) == releases
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                              text=True, check=True, timeout=10).stdout.strip()
        dirty = bool(subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"],
                                    cwd=ROOT, capture_output=True, text=True, check=True,
                                    timeout=10).stdout.strip())
        result = {
            "scenario": "concurrent-genres-and-worker-scheduler-v1",
            "server_sha256": file_sha256(server), "server_build_identity": "operator-supplied; not independently verified",
            "harness_head": head, "harness_tracked_tree_dirty": dirty,
            "host": host_info(), "media_storage": storage, "server_resources": resources,
            "database_environment_and_final_counts": environment,
            "configured_concurrency_per_stage": 2, "poll_interval_seconds": POLL_SECONDS,
            "elapsed_seconds": round(elapsed, 3), "releases": releases,
            "releases_per_minute": round(releases * 60 / elapsed, 3) if elapsed and success else None,
            "tracks_per_second": round(releases / elapsed, 3) if elapsed and success else None,
            "corpus_bytes": corpus_bytes, "genre": genre, "genre_exact": exact,
            "states": status["states"], "failure": failure, "timed_out": timed_out,
            "poll_samples": samples, "poll_count": len(samples), "stage_batch_logs": passes,
            "ready_to_first_claim_seconds": {"count": len(waits), "p50": percentile(waits, 50),
                                              "p95": percentile(waits, 95), "max": max(waits, default=None)},
            "limitations": ["one synthetic Track per Release; MP3 and FLAC; shared new genre",
                "state and lock observations are sampled, not continuous lock duration or full CPU utilization",
                "stage batch log counts include idle interval and do not prove a scheduler fairness guarantee",
                "work-item state residency and lease observations can include waiting rather than active work",
                "server CPU/RSS excludes child processes; resource snapshot includes the idle interval",
                "search, policy, discovery, enrichment and podcast workers disabled; no interactive load",
                "cache condition uncontrolled; source generation excluded; startup and polling included",
                "result-ring bound is a code invariant, not a process-lifetime measurement"],
        }
        print(json.dumps(result, sort_keys=True))
        return 0 if success and exact and not timed_out and failure is None else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--releases", type=int, default=200)
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--temp-parent", type=Path)
    args = parser.parse_args()
    try:
        return run(args.releases, args.timeout, args.server, os.environ.copy(), args.temp_parent)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"Genre/scheduler benchmark failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
