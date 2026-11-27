#!/usr/bin/env python3
"""Measure database competition across real workers, rebuild and HTTP requests.

Run only under perf_representative.py. The server, PostgreSQL database, search
root and generated media belong to this invocation. Capture stdout as JSON;
server logs remain inside the temporary tree and are removed after summarizing.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

from perf_ingestion_workload import FAILURE, SUCCESS, generate_corpus
from perf_ingress_workload import (ROOT, digest, host_info, percentiles, server_resources,
                                  start_server, stop_server, temporary_storage_info)
from run_perf_mutation_benchmarks import validate_fixture
from summarize_perf_metrics import summarize

MAX_SAMPLES = 4096
MAX_LOG_BYTES = 128 * 1024 * 1024
MODES = ("mixed", "ingestion", "rebuild", "idle")
STATUS_SQL = """SELECT json_build_object(
 'database', current_database(),
 'work', COALESCE((SELECT json_object_agg(state,n) FROM
    (SELECT state,count(*) n FROM release_work_items WHERE source_path LIKE 'bench-release-%' GROUP BY state) q), '{}'::json),
 'job', (SELECT status FROM search_rebuild_jobs ORDER BY requested_at DESC LIMIT 1),
 'heartbeats', (SELECT count(*) FROM release_work_item_events WHERE event_type='lease_renewed'),
 'leased', (SELECT count(*) FROM release_work_items WHERE lease_owner IS NOT NULL),
 'lock_waiters', (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                    AND pid<>pg_backend_pid() AND wait_event_type='Lock'),
 'backends', COALESCE((SELECT json_object_agg(kind,n) FROM
    (SELECT COALESCE(wait_event_type,'running') kind,count(*) n FROM pg_stat_activity
      WHERE datname=current_database() AND pid<>pg_backend_pid() GROUP BY 1) q), '{}'::json))"""
ENVIRONMENT_SQL = """SELECT json_build_object('database',current_database(),
 'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
 'tracks',(SELECT count(*) FROM tracks), 'existing_work',(SELECT count(*) FROM release_work_items),
 'existing_jobs',(SELECT count(*) FROM search_rebuild_jobs),
 'postgres_version',current_setting('server_version'), 'shared_buffers',current_setting('shared_buffers'),
 'work_mem',current_setting('work_mem'), 'max_connections',current_setting('max_connections'))"""


def validate_options(mode: str, albums: int, seconds: int, pool_size: int) -> None:
    if mode not in MODES or not 1 <= albums <= 200 or not 20 <= seconds <= 1200 or not 2 <= pool_size <= 64:
        raise ValueError("mode must be supported; albums 1..200, seconds 20..1200, pool size 2..64")


def prepare_root(root: Path) -> None:
    for name in ("inbound", "staging", "work", "storage", "assets", "search"):
        (root / name).mkdir()
    # dotenvy walks ancestors when no local .env exists. An empty private
    # file prevents accidental ingestion of an operator's repository .env.
    (root / ".env").touch(exist_ok=False)


def unused_loopback_port() -> int:
    """Choose a local port; the child must subsequently prove its bind succeeded."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def tool_version(name: str) -> str | None:
    executable = shutil.which(name)
    if executable is None:
        return None
    try:
        result = subprocess.run([executable, "-version"], capture_output=True,
                                text=True, check=False, timeout=10)
        output = result.stdout or result.stderr
        return output.splitlines()[0][:256] if output else None
    except (OSError, subprocess.SubprocessError):
        return None


def log_contains_canary(path: Path, canary: str) -> bool:
    """Scan a private log without retaining it or emitting the canary."""
    needle = canary.encode("utf-8")
    if not 16 <= len(needle) <= 256:
        raise ValueError("isolation canary must be 16..256 UTF-8 bytes")
    suffix = b""
    with path.open("rb") as stream:
        while chunk := stream.read(64 * 1024):
            block = suffix + chunk
            if needle in block:
                return True
            suffix = block[-(len(needle) - 1):]
    return False


def query(url: str, sql: str) -> dict:
    result = subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url,
                             "--tuples-only", "--no-align", "--command", sql],
                            capture_output=True, text=True, check=True, timeout=15)
    value = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise ValueError("unexpected fixture query response")
    return value


def request(port: int, method: str, path: str, token: str | None = None,
            payload: dict | None = None) -> tuple[int | None, bytes, float, str | None]:
    headers = {"Connection": "close"}
    body = None
    if payload is not None:
        body = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    start = time.monotonic()
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=45)
    try:
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        data = response.read(2 * 1024 * 1024 + 1)
        if len(data) > 2 * 1024 * 1024:
            return response.status, b"", time.monotonic() - start, "response_too_large"
        return response.status, data, time.monotonic() - start, None
    except (OSError, http.client.HTTPException) as error:
        return None, b"", time.monotonic() - start, type(error).__name__
    finally:
        connection.close()


def native_login(port: int) -> str:
    status, body, _, error = request(port, "POST", "/api/v1/auth/authenticate",
                                     payload={"userName": "benchmark-1", "password": "benchmark-only-password"})
    if status != 200 or error:
        raise RuntimeError(f"fixture administrator authentication failed (status={status}, error={error})")
    token = json.loads(body).get("token")
    if not isinstance(token, str) or not token:
        raise RuntimeError("fixture login did not yield a token")
    return token


def issue_rebuild(port: int, token: str) -> bool:
    status, body, _, error = request(port, "POST", "/api/admin/v1/search/rebuild", token)
    if status != 202 or error:
        try:
            data = json.loads(body)
            code = data.get("code", data.get("error", {}).get("code"))
            if not isinstance(code, str) or not re.fullmatch(r"[a-z_]{1,64}", code):
                code = None
        except (ValueError, AttributeError):
            code = None
        if status == 409 and code == "rebuild_in_progress":
            return False
        raise RuntimeError(f"server did not accept search rebuild (status={status}, code={code}, error={error})")
    uuid.UUID(json.loads(body)["jobId"])
    return True


def probe_loop(port: int, token: str, path: str, stop: threading.Event, started: float) -> list[dict]:
    samples = []
    while not stop.is_set() and len(samples) < MAX_SAMPLES:
        begin = time.monotonic()
        status, _, elapsed, error = request(port, "GET", path, token)
        samples.append({"started_seconds": round(begin - started, 6),
                        "finished_seconds": round(time.monotonic() - started, 6),
                        "seconds": round(elapsed, 6), "status": status, "error": error})
        stop.wait(0.25)
    return samples


def within(samples: list[dict], begin: float | None, end: float | None) -> list[dict]:
    if begin is None or end is None:
        return []
    return [s for s in samples if s["started_seconds"] >= begin and s["finished_seconds"] <= end]


def summary(samples: list[dict]) -> dict:
    good = [s["seconds"] for s in samples if s["status"] == 200 and s["error"] is None]
    return {"count": len(samples), "errors": len(samples) - len(good),
            "latency_seconds": percentiles(good) if good else None}


def run(mode: str, albums: int, seconds: int, pool_size: int, binary: Path,
        parent: Path, env: dict[str, str], allow_missing_metrics: bool = False,
        output: Path | None = None) -> int:
    validate_fixture(env)
    validate_options(mode, albums, seconds, pool_size)
    if output is not None and (output.exists() or output.is_symlink()):
        raise ValueError("output path already exists")
    binary = binary.resolve(strict=True)
    parent = parent.resolve(strict=True)
    if not binary.is_file() or not parent.is_dir():
        raise ValueError("server executable and temporary parent must exist")
    storage = temporary_storage_info(parent)
    if storage["filesystem"] in (None, "tmpfs", "ramfs", "devtmpfs", "overlay"):
        raise ValueError("temporary parent must be on known disk-backed storage")
    url = env["MELODEE__DATABASE__URL"]
    environment = query(url, ENVIRONMENT_SQL)
    if (environment.pop("database") != env["MELODEE_PERF_OWNED_DATABASE"]
            or environment["existing_work"] or environment["existing_jobs"]):
        raise ValueError("fixture identity mismatch or preexisting work")
    ingest = mode in ("mixed", "ingestion")
    rebuild = mode in ("mixed", "rebuild")
    with tempfile.TemporaryDirectory(prefix="melodee-perf-db-mixed-", dir=parent) as temporary:
        root = Path(temporary)
        prepare_root(root)
        # Expose complete media before server startup so the startup search
        # rebuild and ordinary ingestion workers can overlap from their first
        # turns. It also avoids treating media generation as timed work.
        media_bytes = generate_corpus(root / "inbound", albums) if ingest else 0
        overrides = {"MELODEE__LOGGING__LEVEL": None, "RUST_LOG": "info,melodee::performance=trace",
                     "MELODEE__SERVER__BIND": f"127.0.0.1:{unused_loopback_port()}",
                     "MELODEE__DATABASE__MAX_CONNECTIONS": str(pool_size),
                     "MELODEE__DATABASE__MIN_CONNECTIONS": "1",
                     "MELODEE__SEARCH__ENABLED": "true" if rebuild else "false",
                     "MELODEE__SEARCH__ROOT": str(root / "search")}
        process, log, port = start_server(binary, root, url, 0, ingest, overrides,
                                          isolate_host_environment=True)
        log_path = Path(log.name)
        initial, samples, timed_out, failure = None, [], False, None
        workload_elapsed = None
        rebuild_trigger = None
        server_stopped = False
        first_overlap = last_overlap = None
        stop = threading.Event()
        try:
            token = native_login(port)
            initial = query(url, STATUS_SQL)
            if initial.pop("database") != env["MELODEE_PERF_OWNED_DATABASE"]:
                raise RuntimeError("fixture database identity changed")
            begin = time.monotonic()
            with ThreadPoolExecutor(max_workers=2) as executor:
                admin = executor.submit(probe_loop, port, token, "/api/admin/v1/stats", stop, begin)
                browse = executor.submit(probe_loop, port, token, "/api/v1/artists?page=1&pageSize=50", stop, begin)
                try:
                    if rebuild:
                        # A fresh server can request the first generation before
                        # the listener is ready. Adopt only this fixture's own
                        # live startup job; never enqueue a competing rebuild.
                        if initial["job"] in ("requested", "running", "validated"):
                            rebuild_trigger = "server_startup"
                        elif issue_rebuild(port, token):
                            rebuild_trigger = "admin_request"
                        else:
                            current = query(url, STATUS_SQL)
                            if current.get("job") not in ("requested", "running", "validated"):
                                raise RuntimeError("rebuild conflict without a live owned job")
                            rebuild_trigger = "server_startup_race"
                    while time.monotonic() - begin < seconds and len(samples) < MAX_SAMPLES:
                        if process.poll() is not None:
                            failure = "server_exited"
                            break
                        try:
                            state = query(url, STATUS_SQL)
                        except subprocess.TimeoutExpired:
                            timed_out = True
                            failure = "sampling_query_timeout"
                            break
                        except (OSError, ValueError, subprocess.SubprocessError):
                            failure = "sampling_query_failed"
                            break
                        state.pop("database", None)
                        elapsed = time.monotonic() - begin
                        state["elapsed_seconds"] = round(elapsed, 3)
                        samples.append(state)
                        work = state["work"]
                        failed = sum(work.get(kind, 0) for kind in FAILURE)
                        completed = sum(work.get(kind, 0) for kind in SUCCESS)
                        job_done = state["job"] == "activated"
                        if failed or state["job"] == "failed":
                            failure = "work_or_rebuild_failed"
                            break
                        if ingest and rebuild and sum(work.values()) > completed and state["job"] in ("running", "validated"):
                            first_overlap = elapsed if first_overlap is None else first_overlap
                            last_overlap = elapsed
                        done = (not ingest or completed == albums) and (not rebuild or job_done)
                        if mode != "idle" and done:
                            break
                        time.sleep(0.5)
                    else:
                        timed_out = mode != "idle"
                        if timed_out:
                            failure = "sample_limit" if len(samples) >= MAX_SAMPLES else "deadline"
                finally:
                    workload_elapsed = time.monotonic() - begin
                    # At least one post-work observation allows the periodic
                    # cumulative metric export to close a bounded interval.
                    try:
                        if sys.exc_info()[0] is not None:
                            stop.set()
                            stop_server(process, log)
                            server_stopped = True
                        elif process.poll() is None:
                            time.sleep(11)
                    except BaseException:
                        stop.set()
                        if not server_stopped:
                            stop_server(process, log)
                            server_stopped = True
                        raise
                    finally:
                        stop.set()
                admin_samples = admin.result(timeout=50)
                browse_samples = browse.result(timeout=50)
            resources = server_resources(process.pid)
        finally:
            stop.set()
            if not server_stopped:
                stop_server(process, log)
        if log_path.stat().st_size > MAX_LOG_BYTES:
            raise RuntimeError("owned server log exceeds summarization bound")
        canary = env.get("MELODEE_PERF_ISOLATION_CANARY")
        canary_present = log_contains_canary(log_path, canary) if canary else None
        if canary_present:
            failure = "isolation_canary_found_in_server_log"
        metrics_error = None
        try:
            with log_path.open(encoding="utf-8") as stream:
                metrics = summarize(stream)
        except (OSError, ValueError) as error:
            metrics = []
            metrics_error = type(error).__name__
        overlap_admin = within(admin_samples, first_overlap, last_overlap)
        overlap_browse = within(browse_samples, first_overlap, last_overlap)
        all_errors = summary(admin_samples)["errors"] + summary(browse_samples)["errors"]
        overlap = first_overlap is not None and last_overlap is not None and last_overlap > first_overlap
        workload_success = (failure is None and not timed_out and all_errors == 0 and bool(samples)
                            and len(samples) < MAX_SAMPLES and len(admin_samples) < MAX_SAMPLES
                            and len(browse_samples) < MAX_SAMPLES
                            and (mode != "mixed" or (overlap and bool(overlap_admin) and bool(overlap_browse))))
        required_metrics = {"admin_stats_headline_acquire", "admin_stats_headline_hold"}
        if ingest:
            required_metrics.update(("catalog_import_acquire", "catalog_import_hold"))
        if rebuild:
            required_metrics.update(("search_snapshot_acquire", "search_snapshot_hold"))
        measured_metrics = {row["metric"] for row in metrics if row["observations"] > 0}
        missing_metrics = sorted(required_metrics - measured_metrics)
        success = workload_success and (not missing_metrics or allow_missing_metrics)
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True,
                              check=True, timeout=10).stdout.strip()
        dirty = bool(subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT,
                                    capture_output=True, text=True, check=True, timeout=10).stdout.strip())
        result = {"mode": mode, "timestamp_utc": datetime.now(timezone.utc).isoformat(),
                  "repository_head_observed_at_end": head, "tracked_worktree_dirty_at_end": dirty,
                  "binary_sha256": digest(binary), "binary_source_identity": "not independently established",
                  "host": host_info(), "storage": storage, "database_before_work": environment,
                  "media_tools": {name: tool_version(name) for name in ("ffmpeg", "ffprobe", "fpcalc")},
                  "build_profile": "expected release; binary profile not independently inspected",
                  "fixture_generation": "fixed SQL rules with UUIDv7 identities; no random seed",
                  "cache_conditions": "uncontrolled operating-system and PostgreSQL caches",
                  "pool_max_connections": pool_size, "pool_min_connections": 1,
                  "worker_stage_concurrency": 2, "requested_albums": albums if ingest else 0,
                  "media_bytes": media_bytes, "media_shape": "generated 5-second MP3/FLAC, one Track per Release",
                  "sample_period_seconds": 0.5, "timeout_seconds": seconds, "timed_out": timed_out,
                  "workload_elapsed_seconds": round(workload_elapsed, 6) if workload_elapsed is not None else None,
                  "elapsed_censored": timed_out,
                  "failure": failure, "rebuild_trigger": rebuild_trigger, "database_samples": samples,
                  "isolation_canary_found_in_log": canary_present,
                  "heartbeat_events_delta": samples[-1]["heartbeats"] - initial["heartbeats"] if samples else None,
                  "overlap_seconds": [first_overlap, last_overlap], "admin_all": summary(admin_samples),
                  "browse_all": summary(browse_samples), "admin_overlap": summary(overlap_admin),
                  "browse_overlap": summary(overlap_browse), "admin_samples": admin_samples,
                  "browse_samples": browse_samples, "metrics": metrics, "metrics_error": metrics_error,
                  "server_resources": resources,
                  "workload_success": workload_success, "required_metrics_missing": missing_metrics,
                  "allowed_missing_metrics": allow_missing_metrics, "success": success,
                  "limitations": ["single host and loopback HTTP, generated short media, one Track per Release",
                                  "fixtures omit physical catalog media, realistic ACL and identity evidence",
                                  "cache state uncontrolled; server binary provenance not independently established",
                                  "database and lock samples are discrete, may miss short waits and include sampler connections",
                                  "metric snapshots span startup through shutdown and are not limited to the overlap window",
                                  "pool timings do not independently isolate PostgreSQL CPU, I/O, locks or scheduling",
                                  "RSS and CPU exclude PostgreSQL and subprocesses; no before/after result implied",
                                  "policy, discovery, enrichment and podcast workers disabled"]}
        encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
        if output is None:
            print(encoded, end="")
        else:
            # Exclusive creation prevents overwriting unrelated evidence even if
            # another process creates the path during the workload.
            with output.open("x", encoding="utf-8") as destination:
                destination.write(encoded)
            print(json.dumps({"output": str(output), "success": success}))
    return 0 if success else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=MODES, default="mixed")
    parser.add_argument("--albums", type=int, default=200)
    parser.add_argument("--seconds", type=int, default=900)
    parser.add_argument("--pool-size", type=int, default=8)
    parser.add_argument("--allow-missing-metrics", action="store_true",
                        help="report a baseline lacking this revision's instrumentation without calling it complete")
    parser.add_argument("--output", type=Path, help="write sanitized JSON to a new file; never overwrite")
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--media-parent", type=Path, default=ROOT)
    args = parser.parse_args()
    previous_term = signal.getsignal(signal.SIGTERM)
    previous_int = signal.getsignal(signal.SIGINT)
    def interrupt(signum, _frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if signum == signal.SIGINT:
            raise KeyboardInterrupt
        raise InterruptedError("termination requested")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    try:
        return run(args.mode, args.albums, args.seconds, args.pool_size, args.server,
                   args.media_parent, os.environ.copy(), args.allow_missing_metrics, args.output)
    except RuntimeError as error:
        print(f"Database competition benchmark failed: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, subprocess.SubprocessError, KeyError) as error:
        print(f"Database competition benchmark failed: {type(error).__name__}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Database competition benchmark interrupted; owned cleanup attempted", file=sys.stderr)
        return 130
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)


if __name__ == "__main__":
    raise SystemExit(main())
