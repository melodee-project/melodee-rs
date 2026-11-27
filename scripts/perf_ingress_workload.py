#!/usr/bin/env python3
"""Measure real HTTP admission with owned synthetic media and Jellyfin plays.

Run only as a command of ``perf_representative.py`` using a freshly built
release server. This creates a long synthetic MP3, ingests it through the real
workers, and tests each requested connection limit against concurrent HTTP/1
streams, Jellyfin stopped events, and unrelated health requests. A transaction
locks the synthetic Track to make contention reproducible. All databases and
media roots must belong to this invocation; no production instance is touched.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
from datetime import datetime, timezone
import hashlib
import http.client
import json
import math
import os
from pathlib import Path
import platform
import queue
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

from perf_ingestion_workload import query, server_environment
from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]
FIXTURE_USER = "benchmark-1"
FIXTURE_PASSWORD = "benchmark-only-password"
SOURCE = "ingress-benchmark-release"
READY_TIMEOUT_SECONDS = 90
REQUEST_TIMEOUT_SECONDS = 45


def validate_options(limits: list[int], plays: int, probes: int, hold_seconds: float,
                     media_seconds: int = 480, idle_connections: int = 1) -> None:
    if not limits or len(limits) > 8 or len(set(limits)) != len(limits):
        raise ValueError("provide 1..8 distinct connection limits")
    if any(not 0 <= value <= 128 for value in limits):
        raise ValueError("connection limits must be between 0 and 128")
    if not 1 <= plays <= 64 or not 1 <= probes <= 64 or not 0.25 <= hold_seconds <= 15:
        raise ValueError("plays/probes must be 1..64 and hold seconds 0.25..15")
    if not 30 <= media_seconds <= 900:
        raise ValueError("media seconds must be 30..900")
    if not 0 <= idle_connections <= 16 or any(0 < limit <= idle_connections + 1 for limit in limits):
        raise ValueError("idle connections must be 0..16 and leave capacity for a stream and a request")


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def percentiles(samples: list[float]) -> dict[str, float]:
    if not samples:
        raise ValueError("cannot summarize an empty sample")
    ordered = sorted(samples)
    return {f"p{percentile}": round(ordered[math.ceil(len(ordered) * percentile / 100) - 1], 6)
            for percentile in (50, 95, 99)}


def server_resources(pid: int) -> dict:
    """Linux process measurements when available; no external process helpers."""
    result = {"peak_rss_kib": None, "cpu_seconds": None}
    try:
        status = Path(f"/proc/{pid}/status").read_text()
        rss_line = next(line for line in status.splitlines() if line.startswith("VmHWM:"))
        result["peak_rss_kib"] = int(rss_line.split()[1])
        # The comm field may contain spaces or parentheses; split after the last
        # closing parenthesis and use the documented utime/stime fields.
        stat_fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        ticks = os.sysconf("SC_CLK_TCK")
        result["cpu_seconds"] = round((int(stat_fields[11]) + int(stat_fields[12])) / ticks, 6)
    except (AttributeError, IndexError, OSError, StopIteration, ValueError):
        pass
    return result


def temporary_storage_info(path: Path) -> dict:
    """Describe temporary storage without exposing the generated root path."""
    result = {"filesystem": None, "allocation_unit_bytes": None}
    try:
        result["allocation_unit_bytes"] = os.statvfs(path).f_frsize
    except (AttributeError, OSError):
        pass
    try:
        resolved = str(path.resolve())
        candidates = []
        for line in Path("/proc/self/mountinfo").read_text().splitlines():
            left, right = line.split(" - ", 1)
            mount = re.sub(r"\\([0-7]{3})", lambda match: chr(int(match.group(1), 8)), left.split()[4])
            if resolved == mount or resolved.startswith(mount.rstrip("/") + "/"):
                candidates.append((len(mount), right.split()[0]))
        if candidates:
            result["filesystem"] = max(candidates)[1]
    except (IndexError, OSError, ValueError):
        pass
    return result


def host_info() -> dict:
    result = {"platform": platform.platform(), "logical_cpus": os.cpu_count(),
              "cpu_model": None, "memory_total_kib": None}
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            name, separator, value = line.partition(":")
            if separator and name.strip() == "model name":
                result["cpu_model"] = value.strip()
                break
    except OSError:
        pass
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                result["memory_total_kib"] = int(line.split()[1])
                break
    except (IndexError, OSError, ValueError):
        pass
    return result


def clean_server_environment(root: Path, database_url: str, limit: int, ingest: bool) -> dict[str, str]:
    env = server_environment(root, database_url, 2)
    env.pop("MELODEE_BIND", None)
    env.pop("RUST_LOG", None)
    env.update({
        "MELODEE__SERVER__BIND": "127.0.0.1:0",
        "MELODEE__SERVER__MAX_CONNECTIONS": str(limit),
        "MELODEE__LOGGING__FORMAT": "json",
        "MELODEE__LOGGING__LEVEL": "info",
        "MELODEE__INGEST__ENABLED": "true" if ingest else "false",
        "MELODEE__CATALOG__ENABLED": "true" if ingest else "false",
    })
    return env


def restrict_server_environment(env: dict[str, str]) -> dict[str, str]:
    """Keep configured fixture values and only ordinary process startup variables."""
    allowed = {"PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "SYSTEMROOT", "WINDIR", "TEMP", "TMP"}
    return {key: value for key, value in env.items() if key.startswith("MELODEE__") or key in allowed}


def start_server(binary: Path, root: Path, database_url: str, limit: int, ingest: bool,
                 environment_overrides: dict[str, str | None] | None = None,
                 isolate_host_environment: bool = False):
    log_path = root / f"server-{uuid.uuid4().hex}.log"
    log = log_path.open("wb")
    child_env = clean_server_environment(root, database_url, limit, ingest)
    if isolate_host_environment:
        child_env = restrict_server_environment(child_env)
    for key, value in (environment_overrides or {}).items():
        if value is None:
            child_env.pop(key, None)
        else:
            child_env[key] = value
    bind = re.fullmatch(r"127\.0\.0\.1:([0-9]{1,5})", child_env.get("MELODEE__SERVER__BIND", ""))
    if not bind or int(bind.group(1)) > 65535:
        log.close()
        raise ValueError("benchmark server must bind only to a loopback port")
    requested_port = int(bind.group(1))
    try:
        process = subprocess.Popen([str(binary)], cwd=root,
                                   env=child_env,
                                   stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=os.name == "posix")
    except BaseException:
        log.close()
        raise
    deadline = time.monotonic() + READY_TIMEOUT_SECONDS
    pending = ""
    seen_limit = None
    try:
        with log_path.open(encoding="utf-8", errors="replace") as reader:
            while time.monotonic() < deadline:
                pending += reader.read(16_384)
                lines = pending.split("\n")
                pending = lines.pop()
                if len(pending) > 1_000_000:
                    raise RuntimeError("server produced an oversized startup log line")
                for line in lines:
                    try:
                        event = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    fields = event.get("fields", {})
                    if fields.get("message") == "HTTP connection admission enabled":
                        seen_limit = fields.get("limit")
                    if fields.get("message") == "Melodee Rust bootstrap server listening":
                        if (limit and seen_limit != limit) or (not limit and seen_limit is not None):
                            raise RuntimeError("server did not confirm the requested connection limit")
                        match = re.fullmatch(r"127\.0\.0\.1:([0-9]{1,5})", str(fields.get("address", "")))
                        if not match or not 1 <= int(match.group(1)) <= 65535:
                            raise RuntimeError("server did not report its owned loopback port")
                        if requested_port and int(match.group(1)) != requested_port:
                            raise RuntimeError("server reported an unexpected loopback port")
                        return process, log, int(match.group(1))
                if process.poll() is not None:
                    raise RuntimeError("owned server exited during startup")
                time.sleep(0.05)
        raise RuntimeError("owned server did not report its listener before timeout")
    except BaseException:
        stop_server(process, log)
        raise


def stop_server(process: subprocess.Popen, log) -> None:
    try:
        if process.poll() is None:
            # The private process group also owns fixture FFmpeg children. It
            # is created by this invocation and never contains the caller.
            if os.name == "posix":
                try:
                    if os.getpgid(process.pid) != process.pid or os.getsid(process.pid) != process.pid:
                        raise RuntimeError("owned server process topology changed; refusing group signal")
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            else:
                process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                if os.getpgid(process.pid) != process.pid or os.getsid(process.pid) != process.pid:
                    raise RuntimeError("owned server process topology changed; refusing group kill")
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait(timeout=10)
    finally:
        log.close()


def http_request(port: int, method: str, path: str, token: str | None = None,
                 body: dict | None = None) -> tuple[int, bytes, float]:
    headers = {"Connection": "close"}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if token:
        headers["X-Emby-Token"] = token
    started = time.monotonic()
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=REQUEST_TIMEOUT_SECONDS)
    try:
        connection.request(method, path, body=data, headers=headers)
        response = connection.getresponse()
        payload = response.read(1_000_001)
        if len(payload) > 1_000_000:
            raise RuntimeError("unexpectedly large non-stream response")
        return response.status, payload, time.monotonic() - started
    finally:
        connection.close()


def authenticate(port: int) -> str:
    status, payload, _ = http_request(port, "POST", "/Users/AuthenticateByName",
                                      body={"Username": FIXTURE_USER, "Pw": FIXTURE_PASSWORD})
    if status != 200:
        raise RuntimeError(f"synthetic Jellyfin login returned {status}")
    token = json.loads(payload).get("AccessToken")
    if not isinstance(token, str) or not token:
        raise RuntimeError("synthetic login did not return an access token")
    return token


def observed_request(port: int, method: str, path: str, token: str,
                     body: dict | None = None) -> dict:
    """Retain failed or timed-out request samples without exposing secrets."""
    started = time.monotonic()
    try:
        status, _payload, elapsed = http_request(port, method, path, token, body)
        return {"status": status, "seconds": round(elapsed, 6), "error": None}
    except (OSError, RuntimeError, ValueError, http.client.HTTPException) as error:
        return {"status": None, "seconds": round(time.monotonic() - started, 6),
                "error": type(error).__name__}


class TrackLock:
    """Own one synthetic Track lock and guarantee the psql child is reaped."""

    def __init__(self, database_url: str, track_id: str):
        self.database_url = database_url
        self.track_id = str(uuid.UUID(track_id))
        self.process: subprocess.Popen | None = None

    def __enter__(self):
        self.process = subprocess.Popen(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", self.database_url,
             "--tuples-only", "--no-align", "--quiet"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1,
        )
        try:
            if self.process.stdin is None or self.process.stdout is None:
                raise RuntimeError("owned psql pipes were not created")
            self.process.stdin.write("BEGIN;\n" +
                                     f"SELECT 'owned-track-lock' FROM tracks WHERE id = '{self.track_id}'::uuid FOR UPDATE;\n")
            self.process.stdin.flush()
            messages: queue.Queue[str] = queue.Queue(maxsize=1)
            thread = threading.Thread(target=lambda: messages.put(self.process.stdout.readline()), daemon=True)
            thread.start()
            try:
                line = messages.get(timeout=15).strip()
            except queue.Empty:
                raise RuntimeError("timed out acquiring the owned Track lock") from None
            if line != "owned-track-lock":
                raise RuntimeError("unable to acquire the owned Track lock")
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, _kind, _value, _traceback):
        if self.process is None:
            return
        try:
            if self.process.poll() is None and self.process.stdin:
                try:
                    self.process.stdin.write("ROLLBACK;\n\\q\n")
                    self.process.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        finally:
            if self.process.stdin:
                self.process.stdin.close()
            if self.process.stdout:
                self.process.stdout.close()


def slow_stream(port: int, token: str, track_public_id: str,
                started: threading.Event, stop: threading.Event) -> dict:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=REQUEST_TIMEOUT_SECONDS)
    begin = time.monotonic()
    count = 0
    try:
        connection.connect()
        if connection.sock is None:
            raise RuntimeError("slow stream socket was not created")
        connection.sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
        connection.request("GET", f"/Audio/{track_public_id.replace('-', '')}/stream",
                           headers={"X-Emby-Token": token, "Connection": "close"})
        response = connection.getresponse()
        header_time = time.monotonic() - begin
        if response.status != 200:
            raise RuntimeError(f"slow stream returned {response.status}")
        first_byte = response.read(1)
        if not first_byte:
            raise RuntimeError("slow stream contained no media bytes")
        first_byte_time = time.monotonic() - begin
        count = len(first_byte)
        started.set()
        while not stop.is_set():
            data = response.read(1024)
            if not data:
                raise RuntimeError("slow stream reached EOF before its observation ended")
            count += len(data)
            stop.wait(0.025)
        return {"status": response.status, "time_to_headers_seconds": round(header_time, 6),
                "ttfb_seconds": round(first_byte_time, 6), "bytes": count}
    finally:
        started.set()
        connection.close()


def one_case(port: int, database_url: str, token: str, track_id: str, track_public_id: str,
             case_id: str, limit: int, plays: int, probes: int, hold_seconds: float,
             idle_connections: int = 1) -> dict:
    if not re.fullmatch(r"ingress-[0-9a-f]{32}-[0-7]", case_id):
        raise ValueError("invalid invocation-owned case identifier")
    started = threading.Event()
    stop = threading.Event()
    sessions = [f"{case_id}-{index:04d}" for index in range(plays)]
    workers = plays + probes + 1
    if workers > 129:
        raise ValueError("internal worker bound exceeded")
    with ExitStack() as held, ThreadPoolExecutor(max_workers=workers) as executor:
        for _ in range(idle_connections):
            idle = http.client.HTTPConnection("127.0.0.1", port, timeout=REQUEST_TIMEOUT_SECONDS)
            held.callback(idle.close)
            idle.request("GET", "/System/Info", headers={"X-Emby-Token": token})
            response = idle.getresponse()
            if response.status != 200 or response.will_close or len(response.read(1_000_001)) > 1_000_000:
                raise RuntimeError("unable to establish an idle HTTP/1 keep-alive connection")
        stream = executor.submit(slow_stream, port, token, track_public_id, started, stop)
        try:
            if not started.wait(timeout=REQUEST_TIMEOUT_SECONDS):
                raise RuntimeError("slow stream did not start")
            if stream.done():
                stream.result()
                raise RuntimeError("slow stream ended unexpectedly")
            with TrackLock(database_url, track_id):
                begin = time.monotonic()
                play_futures = [executor.submit(observed_request, port, "POST", "/Sessions/Playing/Stopped",
                                                token, {"ItemId": track_public_id.replace("-", ""),
                                                        "PlaySessionId": session})
                                for session in sessions]
                probe_futures = [executor.submit(observed_request, port, "GET", "/System/Info", token)
                                 for _ in range(probes)]
                time.sleep(hold_seconds)
                plays_done_while_locked = sum(future.done() for future in play_futures)
                probes_done_while_locked = sum(future.done() for future in probe_futures)
            play_results = [future.result(timeout=REQUEST_TIMEOUT_SECONDS + 5) for future in play_futures]
            probe_results = [future.result(timeout=REQUEST_TIMEOUT_SECONDS + 5) for future in probe_futures]
            elapsed = time.monotonic() - begin
        finally:
            stop.set()
            stream_result = stream.result(timeout=REQUEST_TIMEOUT_SECONDS)
    observed = query(database_url, f"SELECT json_build_object('count', count(*), 'distinct', count(DISTINCT client_event_id)) FROM native_track_plays WHERE client_event_id LIKE '{case_id}-%'")
    statuses = [result["status"] for result in play_results]
    health_statuses = [result["status"] for result in probe_results]
    safe_track_id = str(uuid.UUID(track_id))
    popularity = query(database_url, f"SELECT json_build_object('plays',(SELECT count(*) FROM native_track_plays WHERE track_id='{safe_track_id}'::uuid),'track_popularity',(SELECT popularity FROM native_recommendation_candidates WHERE entity_type='track' AND entity_id='{safe_track_id}'::uuid))")
    issues = []
    if any(status != 204 for status in statuses) or observed != {"count": plays, "distinct": plays}:
        issues.append("Jellyfin status/play mismatch")
    if any(status != 200 for status in health_statuses):
        issues.append("interactive probe failure")
    if popularity["plays"] != popularity["track_popularity"]:
        issues.append("track popularity does not equal committed plays")
    return {"connection_limit": limit, "plays": plays, "probes": probes,
            "idle_keepalive_connections": idle_connections,
            "success": not issues, "issues": issues,
            "track_lock_seconds": hold_seconds, "elapsed_seconds": round(elapsed, 6),
            "play_requests_completed_while_track_locked": plays_done_while_locked,
            "interactive_probes_completed_while_track_locked": probes_done_while_locked,
            "jellyfin_statuses": statuses, "persisted_plays": observed,
            "track_recommendation_popularity": popularity,
            "play_samples": play_results, "probe_samples": probe_results,
            "play_latency_percentiles_seconds": (
                percentiles([result["seconds"] for result in play_results])
                if all(result["error"] is None for result in play_results) else None),
            "probe_latency_percentiles_seconds": (
                percentiles([result["seconds"] for result in probe_results])
                if all(result["error"] is None for result in probe_results) else None),
            "slow_stream": stream_result}


def run(env: dict[str, str], binary: Path, limits: list[int], plays: int, probes: int,
        hold_seconds: float, media_seconds: int = 480, idle_connections: int = 1) -> int:
    validate_fixture(env)
    validate_options(limits, plays, probes, hold_seconds, media_seconds, idle_connections)
    binary = binary.resolve(strict=True)
    if not binary.is_file():
        raise ValueError("server must be a prebuilt release binary")
    database_url = env["MELODEE__DATABASE__URL"]
    initial = query(database_url, "SELECT json_build_object('database', current_database(), 'work_items', (SELECT count(*) FROM release_work_items))")
    if initial != {"database": env["MELODEE_PERF_OWNED_DATABASE"], "work_items": 0}:
        raise ValueError("owned fixture must have no existing work items")
    with tempfile.TemporaryDirectory(prefix="melodee-perf-ingress-") as temporary:
        root = Path(temporary)
        storage = temporary_storage_info(root)
        database = query(database_url, "SELECT json_build_object('version',version(),'shared_buffers',current_setting('shared_buffers'),'work_mem',current_setting('work_mem'),'max_connections',current_setting('max_connections'),'artists',(SELECT count(*) FROM artists),'releases',(SELECT count(*) FROM releases),'tracks',(SELECT count(*) FROM tracks))")
        for name in ("inbound", "staging", "work", "storage", "assets"):
            (root / name).mkdir()
        source = root / "inbound" / SOURCE
        source.mkdir()
        subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                        f"sine=frequency=437:duration={media_seconds}", "-metadata", "album=Ingress Benchmark",
                        "-metadata", "album_artist=Ingress Benchmark Artist", "-metadata", "artist=Ingress Benchmark Artist",
                        "-metadata", "title=Ingress Benchmark Track", "-metadata", "track=1",
                        "-c:a", "libmp3lame", "-b:a", "192k", str(source / "01.mp3")],
                       cwd=ROOT, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=180)
        source_bytes = (source / "01.mp3").stat().st_size
        (source / ".melodee-ready").touch()
        query(database_url, "WITH inserted AS (INSERT INTO native_user_roles (user_id,role,created_at) SELECT id,'HasStreamRole',now() FROM native_users WHERE normalized_username='BENCHMARK-1' ON CONFLICT DO NOTHING RETURNING 1) SELECT json_build_object('inserted',count(*)) FROM inserted")
        process, log, _port = start_server(binary, root, database_url, 0, True)
        try:
            deadline = time.monotonic() + 180
            item = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("owned server exited during fixture ingestion")
                item = query(database_url, "SELECT COALESCE((SELECT json_build_object('state',w.state,'track_id',t.id,'public_id',t.public_api_id) FROM release_work_items w LEFT JOIN catalog_imports c ON c.work_item_id=w.id LEFT JOIN tracks t ON t.release_id=c.release_id WHERE w.source_path='ingress-benchmark-release' ORDER BY t.ordinal LIMIT 1), 'null'::json)")
                if item and item.get("state") in {"cataloged", "indexing", "indexed"} and item.get("track_id"):
                    break
                if item and item.get("state") in {"failed", "quarantined", "needs_review", "ignored", "deleted"}:
                    raise RuntimeError(f"fixture ingestion ended in {item['state']}")
                time.sleep(0.5)
            else:
                raise RuntimeError("fixture ingestion timed out")
        finally:
            stop_server(process, log)
        track_id = str(uuid.UUID(item["track_id"]))
        public_id = str(uuid.UUID(item["public_id"]))
        results = []
        run_id = "ingress-" + uuid.uuid4().hex
        for index, limit in enumerate(limits):
            process, log, port = start_server(binary, root, database_url, limit, False)
            try:
                token = authenticate(port)
                result = one_case(port, database_url, token, track_id, public_id,
                                  f"{run_id}-{index}", limit, plays, probes, hold_seconds,
                                  idle_connections)
                result["server_resources"] = server_resources(process.pid)
                results.append(result)
            finally:
                stop_server(process, log)
            if not result["success"]:
                break
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, check=True,
                              capture_output=True, text=True).stdout.strip()
        dirty = bool(subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"],
                                    cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip())
        successful = len(results) == len(limits) and all(result["success"] for result in results)
        print(json.dumps({"timestamp_utc": datetime.now(timezone.utc).isoformat(),
                          "binary_sha256": digest(binary), "repository_head_observed_at_end": head,
                          "binary_source_identity": "not independently established; compare binary hash with build record",
                          "tracked_worktree_dirty_at_end": dirty, "host": host_info(),
                          "database_before_ingestion": database, "temporary_storage": storage,
                          "media": f"one generated {media_seconds}-second MP3 ingested by the server",
                          "source_media_bytes": source_bytes,
                          "slow_client_receive_buffer_requested_bytes": 4096,
                          "success": successful, "results": results,
                          "limitations": ["single process and host, loopback HTTP/1 only",
                                          "synthetic Track lock models contention rather than a catalog workload",
                                          "one synthetic stream; aborts before end and does not measure audible underruns",
                                          "no concurrent ingestion after fixture creation, rebuild or external proxy",
                                          "generated media uses system temporary storage; cache uncontrolled",
                                          "small client and sample counts; not a safe-default certification"]}, sort_keys=True))
    return 0 if successful else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--connection-limit", type=int, action="append", help="repeat for each cap; 0 disables admission")
    parser.add_argument("--plays", type=int, default=16)
    parser.add_argument("--probes", type=int, default=16)
    parser.add_argument("--hold-seconds", type=float, default=3)
    parser.add_argument("--media-seconds", type=int, default=480)
    parser.add_argument("--idle-connections", type=int, default=1)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    args = parser.parse_args()
    try:
        return run(os.environ.copy(), args.server, args.connection_limit or [0, 4, 16, 32, 64],
                   args.plays, args.probes, args.hold_seconds, args.media_seconds, args.idle_connections)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        detail = f": {error}" if isinstance(error, (RuntimeError, ValueError)) else ""
        print(f"Ingress benchmark failed: {type(error).__name__}{detail}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
