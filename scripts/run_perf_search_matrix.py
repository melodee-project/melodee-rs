#!/usr/bin/env python3
"""Run the production outbox batch-size matrix on an invocation-owned fixture.

Only aggregate timing/counter fields are saved. Raw trace output and the
private search tree are removed. Linux pidfds are required for owned session
cleanup. The parent fixture owns and removes the disposable database.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

from build_perf_server import command_output, file_digest, source_fingerprints
from perf_jellyfin_audio_plan import check_parent
from perf_media_io_trace import child_exited, require_owned_session_support, terminate_owned

ROOT = Path(__file__).resolve().parents[1]
MAX_LOG_BYTES = 32 * 1024 * 1024
MAX_SAMPLES = 2048
TRIALS = 8
MARKER = re.compile(r"BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial=(\d+)")
FLOAT = r"([0-9]+(?:\.[0-9]+)?)"
EVENTS = (
    ("index_open_writable", "search index opened", "elapsed_seconds"),
    ("index_open_readonly", "search index opened", "elapsed_seconds"),
    ("writer_init", "search writer initialized", "elapsed_seconds"),
    ("reader_init", "search reader initialized", "elapsed_seconds"),
    ("cached_reader_reload", "cached search reader reloaded", "elapsed_seconds"),
    ("commit", "search writer committed", "commit_seconds"),
    ("commit_reader_reload", "search writer committed", "reload_seconds"),
    ("segments", "search writer committed", "segments"),
)


def parse_log(data: str) -> list[dict]:
    trials: dict[int, dict] = {}
    current: int | None = None
    for line in data.splitlines():
        marker = MARKER.search(line)
        if marker:
            current = int(marker.group(1))
            if current < 1 or current > TRIALS or current in trials:
                raise ValueError("invalid or repeated trial marker")
            trials[current] = {"lifecycle": {name: [] for name, _, _ in EVENTS}}
            continue
        if current is None:
            continue
        if '"workload":"postgres-search-incremental"' in line:
            start = line.find("{")
            record = json.loads(line[start:])
            if record.get("trial") != current or "result" in trials[current]:
                raise ValueError("invalid or repeated trial result")
            trials[current]["result"] = record
        for label, message, field in EVENTS:
            if message not in line:
                continue
            if label == "index_open_writable" and "writable=true" not in line:
                continue
            if label == "index_open_readonly" and "writable=false" not in line:
                continue
            match = re.search(r"(?:^|\s)" + field + "=" + FLOAT, line)
            if not match:
                raise ValueError("lifecycle event lacks required field")
            values = trials[current]["lifecycle"][label]
            if len(values) >= MAX_SAMPLES:
                raise ValueError("lifecycle sample bound exceeded")
            values.append(float(match.group(1)))
    if len(trials) != TRIALS or any("result" not in item for item in trials.values()):
        raise ValueError("incomplete benchmark output")
    expected = [16, 64, 256, 512, 512, 256, 64, 16]
    if [trials[i]["result"].get("batch_size") for i in range(1, TRIALS + 1)] != expected:
        raise ValueError("unexpected batch-size sequence")
    for trial in trials.values():
        batches = trial["result"].get("batches")
        if not isinstance(batches, int) or batches < 1 or batches > MAX_SAMPLES:
            raise ValueError("invalid batch count")
        if any(len(trial["lifecycle"][name]) != batches for name in (
            "index_open_writable", "writer_init", "commit", "commit_reader_reload", "segments"
        )):
            raise ValueError("incomplete lifecycle events")
    return [trials[i] for i in range(1, TRIALS + 1)]


def run(env: dict[str, str], binary: Path, output: Path, timeout: int, provenance: Path) -> None:
    if timeout < 1 or timeout > 3600:
        raise ValueError("timeout out of bounds")
    check_parent(env, os.getppid())
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if (binary.is_symlink() or not binary.is_file() or not binary.name.startswith("perf_outbox-")):
        raise ValueError("invalid selected test binary")
    binary = binary.resolve(strict=True)
    binary_sha = file_digest(binary)
    if provenance.is_symlink() or not provenance.is_file() or provenance.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid build provenance")
    build = json.loads(provenance.read_text(encoding="utf-8"))
    if build.get("test_target") != "search-outbox" or build.get("test_executable_sha256") != binary_sha:
        raise ValueError("binary does not match build provenance")
    built_sources = build.get("source_sha256")
    if (not isinstance(built_sources, dict) or not isinstance(build.get("commit"), str)
            or not all(isinstance(key, str) and isinstance(value, str)
                       for key, value in built_sources.items())):
        raise ValueError("invalid build provenance fields")
    current_sources = source_fingerprints(ROOT)
    changed = [{"path": key, "built_sha256": built_sources.get(key), "current_sha256": current_sources.get(key)}
               for key in sorted(set(built_sources) | set(current_sources)) if built_sources.get(key) != current_sources.get(key)]
    current_head = command_output(["git", "rev-parse", "HEAD"], ROOT)
    require_owned_session_support()
    with tempfile.TemporaryDirectory(prefix="melodee-search-matrix-") as directory:
        root = Path(directory).resolve()
        (root / ".env").touch(exist_ok=False)
        child_env = env.copy()
        for key in ("TMPDIR", "TMP", "TEMP"):
            child_env[key] = str(root)
        with tempfile.TemporaryFile(mode="w+b", dir=root) as log:
            child = subprocess.Popen([str(binary), "--ignored", "--nocapture", "--test-threads=1"],
                cwd=root, env=child_env, stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, start_new_session=True)
            try:
                leader = os.pidfd_open(child.pid)
            except BaseException:
                terminate_owned(child)
                raise
            started = time.monotonic()
            censored = False
            oversized = False
            try:
                while not child_exited(leader):
                    if time.monotonic() - started >= timeout:
                        censored = True
                        break
                    if os.fstat(log.fileno()).st_size > MAX_LOG_BYTES:
                        oversized = True
                        break
                    time.sleep(0.1)
            finally:
                try:
                    terminate_owned(child)
                finally:
                    os.close(leader)
            size = os.fstat(log.fileno()).st_size
            if size > MAX_LOG_BYTES:
                oversized = True
            if censored or oversized:
                record = {"status": "timeout_censored" if censored else "output_limit_censored",
                          "timeout_seconds": timeout, "binary_sha256": binary_sha,
                          "limitations": ["incomplete run, not valid throughput evidence"]}
            elif child.returncode != 0:
                raise RuntimeError("search test failed; raw log removed")
            else:
                log.seek(0)
                trials = parse_log(log.read(MAX_LOG_BYTES + 1).decode("utf-8", errors="strict"))
                record = {"status": "complete", "binary_sha256": binary_sha,
                          "fixture_sha256": env["MELODEE_PERF_FIXTURE_SHA256"],
                          "migration_cli_sha256": env.get("MELODEE_PERF_MIGRATION_CLI_SHA256"),
                          "wrapper_sha256": file_digest(Path(__file__)), "trials": trials,
                          "build_provenance_sha256": file_digest(provenance),
                          "build_commit": build["commit"], "current_head_at_start": current_head,
                          "source_changes_since_build_at_start": changed,
                          "limitations": ["single process and generation; prior rebuild included in RSS",
                              "uncontrolled cache and segment evolution; mixed upserts only",
                              "in-process searches, no concurrent clients, physical media or network latency",
                              "SQLx event counts are not network round trips"]}
            with output.open("x", encoding="utf-8") as destination:
                json.dump(record, destination, indent=2, sort_keys=True)
                destination.write("\n")
            if censored or oversized:
                raise RuntimeError("benchmark run censored")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build-provenance", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run(dict(os.environ), args.test_binary, args.output, args.timeout_seconds, args.build_provenance)
    except (OSError, UnicodeError, ValueError, RuntimeError, InterruptedError) as error:
        print(f"Search matrix failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
